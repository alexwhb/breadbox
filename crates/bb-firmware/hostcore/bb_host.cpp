// bb_host.cpp — runtime for host-emulated boards: pipe protocol, lock-step time, interrupts.
#include "bb_host.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

namespace bb {

static int in_fd = 0;
static int out_fd = 1;
static std::vector<uint8_t> outbuf;
static size_t last_frame = (size_t)-1;  // offset of the last buffered frame (serial coalescing)
static uint64_t g_now = 0;
static uint8_t g_board = BOARD_ESP32;
static uint64_t g_seed = 1;
static std::string g_nvs;
static std::vector<uint8_t> pending_irqs;
static bool in_isr = false;
static bool async_on = false;
static bool in_async = false;

struct IsrSlot {
  isr_t fn = nullptr;
  isr_arg_t fn_arg = nullptr;
  void* arg = nullptr;
};
static IsrSlot isrs[64];
static std::vector<void (*)(void)> async_hooks;

void Reply::memcpy_(void* d, const void* s, size_t n) { memcpy(d, s, n); }

Buf& Buf::str(const char* s) { return bytes((const uint8_t*)s, s ? strlen(s) : 0); }

static void write_all(const uint8_t* p, size_t n) {
  while (n) {
    ssize_t k = ::write(out_fd, p, n);
    if (k < 0 && errno == EINTR) continue;
    if (k <= 0) _exit(3);
    p += k;
    n -= (size_t)k;
  }
}

static void read_all(uint8_t* p, size_t n) {
  while (n) {
    ssize_t k = ::read(in_fd, p, n);
    if (k < 0 && errno == EINTR) continue;
    if (k <= 0) _exit(0);  // the simulator went away
    p += k;
    n -= (size_t)k;
  }
}

static void put_u32(std::vector<uint8_t>& v, size_t at, uint32_t x) {
  for (int i = 0; i < 4; i++) v[at + i] = (x >> (8 * i)) & 0xff;
}

static void flush() {
  if (!outbuf.empty()) {
    write_all(outbuf.data(), outbuf.size());
    outbuf.clear();
  }
  last_frame = (size_t)-1;
}

void send(uint8_t type, const Buf& p) {
  // Consecutive serial writes to the same port coalesce into one frame.
  if (type == MSG_SERIAL_WRITE && last_frame != (size_t)-1 && outbuf[last_frame + 4] == MSG_SERIAL_WRITE &&
      outbuf[last_frame + 5] == p.b[0] && outbuf.size() - last_frame < 4096) {
    outbuf.insert(outbuf.end(), p.b.begin() + 1, p.b.end());
    put_u32(outbuf, last_frame, (uint32_t)(outbuf.size() - last_frame - 4));
    return;
  }
  last_frame = outbuf.size();
  outbuf.resize(outbuf.size() + 4);
  put_u32(outbuf, last_frame, (uint32_t)(1 + p.b.size()));
  outbuf.push_back(type);
  outbuf.insert(outbuf.end(), p.b.begin(), p.b.end());
  if (outbuf.size() > 16384) flush();
}

Reply rpc(uint8_t type, const Buf& p) {
  send(type, p);
  last_frame = (size_t)-1;
  flush();
  uint8_t h[4];
  read_all(h, 4);
  uint32_t len = h[0] | (uint32_t)h[1] << 8 | (uint32_t)h[2] << 16 | (uint32_t)h[3] << 24;
  std::vector<uint8_t> body(len);
  read_all(body.data(), len);
  Reply r;
  uint64_t t = 0;
  for (int i = 0; i < 8; i++) t |= (uint64_t)body[i] << (8 * i);
  r.now_us = t;
  uint8_t n = body[8];
  for (uint8_t i = 0; i < n; i++) pending_irqs.push_back(body[9 + i]);
  r.data.assign(body.begin() + 9 + n, body.end());
  g_now = t;
  if (!in_isr && !pending_irqs.empty()) run_pending_irqs();
  return r;
}

uint64_t now_us() { return g_now; }

void run_pending_irqs() {
  in_isr = true;
  while (!pending_irqs.empty()) {
    uint8_t pin = pending_irqs.front();
    pending_irqs.erase(pending_irqs.begin());
    if (pin < 64) {
      IsrSlot& s = isrs[pin];
      if (s.fn) s.fn();
      if (s.fn_arg) s.fn_arg(s.arg);
    }
  }
  in_isr = false;
}

void attach(uint8_t pin, isr_t fn, isr_arg_t fn_arg, void* arg, int mode) {
  if (pin >= 64) return;
  isrs[pin].fn = fn;
  isrs[pin].fn_arg = fn_arg;
  isrs[pin].arg = arg;
  send(MSG_ATTACH_IRQ, Buf().u8(pin).u8((uint8_t)mode));
}

void detach(uint8_t pin) {
  if (pin >= 64) return;
  isrs[pin] = IsrSlot();
  send(MSG_DETACH_IRQ, Buf().u8(pin));
}

void add_async_hook(void (*fn)(void)) { async_hooks.push_back(fn); }

void set_async(bool on) {
  if (on == async_on) return;
  async_on = on;
  send(MSG_ASYNC, Buf().u8(on ? 1 : 0));
}

void service_async() {
  if (in_async || in_isr) return;
  in_async = true;
  for (auto f : async_hooks) f();
  in_async = false;
}

void sleep_until(uint64_t t) {
  // Kind 0 = absolute deadline. The parent replies at the deadline, or earlier to deliver an
  // interrupt or (in async mode) to let BLE callbacks run.
  while (g_now < t) {
    Reply r = rpc(MSG_SLEEP, Buf().u8(0).u64(t));
    if (async_on) service_async();
    if (r.now_us >= t) break;
  }
}

void sleep_for(uint64_t us) {
  // Kind 1 = relative: the parent computes the deadline from its exact clock and returns it.
  Reply r = rpc(MSG_SLEEP, Buf().u8(1).u64(us));
  uint64_t deadline = r.u64();
  if (async_on) service_async();
  while (g_now < deadline) {
    r = rpc(MSG_SLEEP, Buf().u8(0).u64(deadline));
    if (async_on) service_async();
  }
}

bool wait_pin(uint8_t pin, uint8_t level, uint32_t timeout_us) {
  Reply r = rpc(MSG_WAIT_PIN, Buf().u8(pin).u8(level).u32(timeout_us));
  return r.u8() != 0;
}

Reply service(uint8_t channel, const Buf& request) {
  Buf b;
  b.u8(channel);
  b.b.insert(b.b.end(), request.b.begin(), request.b.end());
  return rpc(MSG_SERVICE, b);
}

void log(const char* msg) { send(MSG_LOG, Buf().str(msg)); }

void panic(const char* msg) {
  send(MSG_PANIC, Buf().str(msg));
  flush();
  _exit(2);
}

uint8_t board() { return g_board; }
const char* nvs_path() { return g_nvs.c_str(); }
uint64_t seed() { return g_seed; }

void init(int, char**) {
  // The protocol owns the original stdout; printf() from the sketch goes to stderr.
  out_fd = dup(1);
  dup2(2, 1);
  setvbuf(stdout, nullptr, _IONBF, 0);
  const char* b = getenv("BB_HOST_BOARD");
  if (b && strcmp(b, "pico") == 0) g_board = BOARD_PICO;
  if (const char* s = getenv("BB_HOST_SEED")) g_seed = strtoull(s, nullptr, 10);
  if (const char* n = getenv("BB_HOST_NVS")) g_nvs = n;
  rpc(MSG_HELLO, Buf().u16(1).u8(g_board));
}

}  // namespace bb

void setup();
void loop();

int main(int argc, char** argv) {
  bb::init(argc, argv);
  setup();
  for (;;) {
    loop();
    bb::rpc(bb::MSG_YIELD);
    bb::service_async();
  }
}

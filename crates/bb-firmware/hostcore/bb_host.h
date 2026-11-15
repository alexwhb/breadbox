// bb_host.h — Breadbox host core runtime: the child process side of the HostBoard pipe
// protocol (crates/bb-firmware/src/host/protocol.rs is the other side; keep them in sync).
//
// The sketch runs natively, but it never sees the wall clock: time only moves when the parent
// (the simulator) answers. Every hardware access is a message on stdin/stdout:
//
//   child → parent  [u32 len][u8 type][payload]            (one-way or request)
//   parent → child  [u32 len][u64 now_us][u8 n][n×u8 irq pins][payload]   (reply to a request)
//
// One-way messages (pin writes, serial output, PWM) are buffered and flushed before the next
// request, so ordering is preserved and the parent stamps them with its board clock.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <string>
#include <vector>

namespace bb {

// Message types (child → parent).
enum : uint8_t {
  MSG_HELLO = 0x01,
  MSG_TIME = 0x02,
  MSG_SLEEP = 0x03,
  MSG_PIN_MODE = 0x04,
  MSG_DIGITAL_WRITE = 0x05,
  MSG_DIGITAL_READ = 0x06,
  MSG_ANALOG_READ = 0x07,
  MSG_PWM = 0x08,
  MSG_ATTACH_IRQ = 0x09,
  MSG_DETACH_IRQ = 0x0a,
  MSG_SERIAL_BEGIN = 0x0b,
  MSG_SERIAL_WRITE = 0x0c,
  MSG_SERIAL_READ = 0x0d,
  MSG_SERIAL_AVAILABLE = 0x0e,
  MSG_I2C_WRITE = 0x0f,
  MSG_I2C_READ = 0x10,
  MSG_SPI_TRANSFER = 0x11,
  MSG_SERVICE = 0x12,
  MSG_WAIT_PIN = 0x13,
  MSG_LOG = 0x14,
  MSG_RESTART = 0x15,
  MSG_YIELD = 0x16,
  MSG_ASYNC = 0x17,
  MSG_PANIC = 0x18,
  MSG_COST = 0x19,
};

// Board kinds (HELLO payload).
enum : uint8_t { BOARD_ESP32 = 1, BOARD_PICO = 2 };

// Service channels (MSG_SERVICE), served by bb-radio.
enum : uint8_t { CH_NET = 1, CH_BLE = 2 };

// Growable little-endian payload writer.
struct Buf {
  std::vector<uint8_t> b;
  Buf& u8(uint8_t v) {
    b.push_back(v);
    return *this;
  }
  Buf& u16(uint16_t v) {
    b.push_back(v & 0xff);
    b.push_back(v >> 8);
    return *this;
  }
  Buf& u32(uint32_t v) {
    for (int i = 0; i < 4; i++) b.push_back((v >> (8 * i)) & 0xff);
    return *this;
  }
  Buf& i32(int32_t v) { return u32((uint32_t)v); }
  Buf& u64(uint64_t v) {
    for (int i = 0; i < 8; i++) b.push_back((v >> (8 * i)) & 0xff);
    return *this;
  }
  Buf& ip(uint32_t a) {  // a.b.c.d as bytes a b c d
    for (int i = 0; i < 4; i++) b.push_back((a >> (8 * i)) & 0xff);
    return *this;
  }
  Buf& bytes(const uint8_t* p, size_t n) {
    if (n > 65535) n = 65535;
    u16((uint16_t)n);
    b.insert(b.end(), p, p + n);
    return *this;
  }
  Buf& str(const char* s);
  Buf& str(const std::string& s) { return bytes((const uint8_t*)s.data(), s.size()); }
};

// Reply payload reader.
struct Reply {
  uint64_t now_us = 0;
  std::vector<uint8_t> data;
  size_t pos = 0;
  uint8_t u8() { return pos < data.size() ? data[pos++] : 0; }
  uint16_t u16() {
    uint16_t a = u8();
    return a | (uint16_t)u8() << 8;
  }
  uint32_t u32() {
    uint32_t v = 0;
    for (int i = 0; i < 4; i++) v |= (uint32_t)u8() << (8 * i);
    return v;
  }
  int32_t i32() { return (int32_t)u32(); }
  uint64_t u64() {
    uint64_t v = 0;
    for (int i = 0; i < 8; i++) v |= (uint64_t)u8() << (8 * i);
    return v;
  }
  double f64() {
    uint64_t v = u64();
    double d;
    memcpy_(&d, &v, 8);
    return d;
  }
  uint32_t ip() { return u32(); }
  std::vector<uint8_t> bytes() {
    size_t n = u16();
    if (pos + n > data.size()) n = data.size() - pos;
    std::vector<uint8_t> v(data.begin() + pos, data.begin() + pos + n);
    pos += n;
    return v;
  }
  std::string str() {
    auto v = bytes();
    return std::string(v.begin(), v.end());
  }
  static void memcpy_(void* d, const void* s, size_t n);
};

// Runtime API used by the Arduino layer.
void init(int argc, char** argv);
void send(uint8_t type, const Buf& payload);  // one-way (buffered)
Reply rpc(uint8_t type, const Buf& payload);  // request/reply (flushes first)
inline Reply rpc(uint8_t type) { return rpc(type, Buf()); }
uint64_t now_us();                            // last time the parent reported
void sleep_until(uint64_t t_us);              // blocks in sim time, servicing interrupts/async
void sleep_for(uint64_t us);                  // relative sleep (deadline computed by the parent)
bool wait_pin(uint8_t pin, uint8_t level, uint32_t timeout_us);  // input reaches level?
void service_async();                         // BLE callbacks etc. (called from delay/yield)
void set_async(bool on);                      // ask the parent to wake us periodically
Reply service(uint8_t channel, const Buf& request);
void panic(const char* msg);
void log(const char* msg);
uint8_t board();
// Interrupt plumbing.
typedef void (*isr_t)(void);
typedef void (*isr_arg_t)(void*);
void attach(uint8_t pin, isr_t fn, isr_arg_t fn_arg, void* arg, int mode);
void detach(uint8_t pin);
void run_pending_irqs();
// Hooks other modules register (BLE polling).
void add_async_hook(void (*fn)(void));
// NVS file for Preferences.
const char* nvs_path();
uint64_t seed();

}  // namespace bb

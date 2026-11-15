// WiFi.cpp — WiFi / WiFiClient / WiFiServer / WiFiUDP / SNTP over the simulator's network
// service (channel CH_NET; byte format in crates/bb-radio/src/host.rs).
#include "WiFi.h"

#include <sys/time.h>

#include "bb_host.h"

WiFiClass WiFi;

namespace {
enum : uint8_t {
  WIFI_BEGIN = 0x01,
  WIFI_STATUS = 0x02,
  WIFI_DISCONNECT = 0x03,
  WIFI_SCAN = 0x04,
  WIFI_MAC = 0x05,
  WIFI_CONFIG = 0x06,
  WIFI_HOSTNAME = 0x07,
  DNS_START = 0x10,
  DNS_RESULT = 0x11,
  TCP_CONNECT = 0x20,
  TCP_STATUS = 0x21,
  TCP_SEND = 0x22,
  TCP_RECV = 0x23,
  TCP_CLOSE = 0x24,
  TCP_LISTEN = 0x25,
  TCP_ACCEPT = 0x26,
  TCP_PEER = 0x27,
  UDP_BIND = 0x30,
  UDP_SENDTO = 0x31,
  UDP_RECV = 0x32,
  UDP_CLOSE = 0x33,
};

bb::Reply net(bb::Buf req) { return bb::service(bb::CH_NET, req); }
bb::Buf op(uint8_t o) { return bb::Buf().u8(o); }
}  // namespace

namespace bbnet {

struct Socket {
  int h = -1;
  std::vector<uint8_t> rx;
  size_t pos = 0;
  ~Socket() {
    if (h >= 0) net(op(TCP_CLOSE).i32(h));
  }
};

uint32_t resolve(const char* host, uint32_t timeout_ms) {
  IPAddress lit;
  if (lit.fromString(host)) return (uint32_t)lit;
  bb::Reply r = net(op(DNS_START).str(host));
  int32_t q = r.i32();
  if (q < 0) return 0;
  uint64_t deadline = bb::now_us() + (uint64_t)timeout_ms * 1000;
  for (;;) {
    bb::Reply s = net(op(DNS_RESULT).i32(q));
    uint8_t state = s.u8();
    uint32_t ip = s.ip();
    if (state == 1) return ip;
    if (state == 2) return 0;
    if (bb::now_us() >= deadline) return 0;
    delay(1);
  }
}

}  // namespace bbnet

// ================================================================== WiFiClass

WiFiClass::Status WiFiClass::refresh() {
  bb::Reply r = net(op(WIFI_STATUS));
  Status s;
  s.wl = r.u8();
  s.ip = r.ip();
  s.gw = r.ip();
  s.mask = r.ip();
  s.dns = r.ip();
  s.rssi = r.i32();
  s.channel = r.u8();
  for (auto& b : s.bssid) b = r.u8();
  s.ssid = String(r.str());
  return s;
}

wl_status_t WiFiClass::begin(const char* ssid, const char* pass, int32_t, const uint8_t*, bool connect) {
  pass_ = pass ? pass : "";
  if (!connect) return WL_DISCONNECTED;
  net(op(WIFI_BEGIN).str(ssid ? ssid : "").str(pass ? pass : ""));
  return WL_DISCONNECTED;
}

wl_status_t WiFiClass::status() { return (wl_status_t)refresh().wl; }

bool WiFiClass::disconnect(bool, bool) {
  net(op(WIFI_DISCONNECT));
  return true;
}

bool WiFiClass::reconnect() { return true; }

bool WiFiClass::config(IPAddress ip, IPAddress gw, IPAddress mask, IPAddress, IPAddress) {
  net(op(WIFI_CONFIG).ip((uint32_t)ip).ip((uint32_t)gw).ip((uint32_t)mask));
  return true;
}

bool WiFiClass::setHostname(const char* name) {
  hostname_ = name;
  net(op(WIFI_HOSTNAME).str(name));
  return true;
}

uint8_t WiFiClass::waitForConnectResult(unsigned long timeout_ms) {
  unsigned long t0 = millis();
  uint8_t s = status();
  while ((s == WL_DISCONNECTED || s == WL_IDLE_STATUS) && millis() - t0 < timeout_ms) {
    delay(100);
    s = status();
  }
  return s;
}

IPAddress WiFiClass::localIP() { return IPAddress(refresh().ip); }
IPAddress WiFiClass::gatewayIP() { return IPAddress(refresh().gw); }
IPAddress WiFiClass::subnetMask() { return IPAddress(refresh().mask); }
IPAddress WiFiClass::dnsIP(uint8_t) { return IPAddress(refresh().dns); }
IPAddress WiFiClass::broadcastIP() {
  Status s = refresh();
  return IPAddress(s.ip | ~s.mask);
}

uint8_t* WiFiClass::macAddress(uint8_t* mac) {
  bb::Reply r = net(op(WIFI_MAC));
  for (int i = 0; i < 6; i++) mac[i] = r.u8();
  return mac;
}

String WiFiClass::macAddress() {
  uint8_t m[6];
  macAddress(m);
  char buf[18];
  snprintf(buf, sizeof buf, "%02X:%02X:%02X:%02X:%02X:%02X", m[0], m[1], m[2], m[3], m[4], m[5]);
  return String(buf);
}

String WiFiClass::SSID() { return refresh().ssid; }
int32_t WiFiClass::RSSI() { return refresh().rssi; }
int32_t WiFiClass::channel() { return refresh().channel; }

uint8_t* WiFiClass::BSSID() {
  Status s = refresh();
  memcpy(bssid_, s.bssid, 6);
  return bssid_;
}

String WiFiClass::BSSIDstr() {
  uint8_t* b = BSSID();
  char buf[18];
  snprintf(buf, sizeof buf, "%02X:%02X:%02X:%02X:%02X:%02X", b[0], b[1], b[2], b[3], b[4], b[5]);
  return String(buf);
}

int WiFiClass::hostByName(const char* host, IPAddress& result) {
  uint32_t ip = bbnet::resolve(host, 4000);
  result = IPAddress(ip);
  return ip ? 1 : 0;
}

int16_t WiFiClass::scanNetworks(bool, bool) {
  delay(1200);  // a full channel scan takes about a second on the ESP32
  bb::Reply r = net(op(WIFI_SCAN));
  scan_.clear();
  uint8_t n = r.u8();
  for (uint8_t i = 0; i < n; i++) {
    ScanEntry e;
    e.ssid = String(r.str());
    e.rssi = r.i32();
    e.channel = r.u8();
    e.secure = r.u8() != 0;
    for (auto& b : e.bssid) b = r.u8();
    scan_.push_back(e);
  }
  return (int16_t)scan_.size();
}

String WiFiClass::SSID(uint8_t i) { return i < scan_.size() ? scan_[i].ssid : String(); }
int32_t WiFiClass::RSSI(uint8_t i) { return i < scan_.size() ? scan_[i].rssi : 0; }
wifi_auth_mode_t WiFiClass::encryptionType(uint8_t i) {
  return i < scan_.size() && scan_[i].secure ? WIFI_AUTH_WPA2_PSK : WIFI_AUTH_OPEN;
}
int32_t WiFiClass::channel(uint8_t i) { return i < scan_.size() ? scan_[i].channel : 0; }
String WiFiClass::BSSIDstr(uint8_t i) {
  if (i >= scan_.size()) return String();
  const uint8_t* b = scan_[i].bssid;
  char buf[18];
  snprintf(buf, sizeof buf, "%02X:%02X:%02X:%02X:%02X:%02X", b[0], b[1], b[2], b[3], b[4], b[5]);
  return String(buf);
}

// ================================================================== WiFiClient

WiFiClient::WiFiClient() {}

WiFiClient::WiFiClient(int handle) : sock_(std::make_shared<bbnet::Socket>()) { sock_->h = handle; }

WiFiClient::~WiFiClient() {}

int WiFiClient::connect(IPAddress ip, uint16_t port) { return connect(ip, port, (int32_t)connect_timeout_ms_); }

int WiFiClient::connect(IPAddress ip, uint16_t port, int32_t timeout_ms) {
  stop();
  bb::Reply r = net(op(TCP_CONNECT).ip((uint32_t)ip).u16(port));
  int h = r.i32();
  if (h < 0) return 0;
  sock_ = std::make_shared<bbnet::Socket>();
  sock_->h = h;
  uint64_t deadline = bb::now_us() + (uint64_t)(timeout_ms > 0 ? timeout_ms : 3000) * 1000;
  for (;;) {
    bb::Reply s = net(op(TCP_STATUS).i32(h));
    uint8_t st = s.u8();
    if (st == 1) return 1;
    if (st >= 2 || bb::now_us() >= deadline) {
      stop();
      return 0;
    }
    delay(1);
  }
}

int WiFiClient::connect(const char* host, uint16_t port) { return connect(host, port, (int32_t)connect_timeout_ms_); }

int WiFiClient::connect(const char* host, uint16_t port, int32_t timeout_ms) {
  uint32_t ip = bbnet::resolve(host, 4000);
  if (!ip) return 0;
  return connect(IPAddress(ip), port, timeout_ms);
}

size_t WiFiClient::write(uint8_t c) { return write(&c, 1); }

size_t WiFiClient::write(const uint8_t* buf, size_t size) {
  if (!sock_ || sock_->h < 0 || size == 0) return 0;
  size_t done = 0;
  while (done < size) {
    size_t n = size - done > 4096 ? 4096 : size - done;
    bb::Reply r = net(op(TCP_SEND).i32(sock_->h).bytes(buf + done, n));
    int k = r.i32();
    if (k <= 0) break;
    done += (size_t)k;
  }
  return done;
}

bool WiFiClient::fill() {
  if (!sock_ || sock_->h < 0) return false;
  if (sock_->pos < sock_->rx.size()) return true;
  bb::Reply r = net(op(TCP_RECV).i32(sock_->h).u32(1460));
  sock_->rx = r.bytes();
  sock_->pos = 0;
  return !sock_->rx.empty();
}

int WiFiClient::available() {
  if (!fill()) return 0;
  return (int)(sock_->rx.size() - sock_->pos);
}

int WiFiClient::read() {
  if (!fill()) return -1;
  return sock_->rx[sock_->pos++];
}

int WiFiClient::read(uint8_t* buf, size_t size) {
  size_t k = 0;
  while (k < size && fill()) {
    size_t n = sock_->rx.size() - sock_->pos;
    if (n > size - k) n = size - k;
    memcpy(buf + k, sock_->rx.data() + sock_->pos, n);
    sock_->pos += n;
    k += n;
  }
  return k ? (int)k : -1;
}

int WiFiClient::peek() {
  if (!fill()) return -1;
  return sock_->rx[sock_->pos];
}

void WiFiClient::stop() {
  if (sock_ && sock_->h >= 0) {
    net(op(TCP_CLOSE).i32(sock_->h));
    sock_->h = -1;
  }
  sock_.reset();
}

uint8_t WiFiClient::connected() {
  if (!sock_ || sock_->h < 0) return 0;
  if (sock_->pos < sock_->rx.size()) return 1;
  bb::Reply s = net(op(TCP_STATUS).i32(sock_->h));
  uint8_t st = s.u8();
  uint32_t avail = s.u32();
  return st == 1 || (st == 2 && avail > 0);
}

IPAddress WiFiClient::remoteIP() {
  if (!sock_ || sock_->h < 0) return IPAddress();
  bb::Reply r = net(op(TCP_PEER).i32(sock_->h));
  return IPAddress(r.ip());
}

uint16_t WiFiClient::remotePort() {
  if (!sock_ || sock_->h < 0) return 0;
  bb::Reply r = net(op(TCP_PEER).i32(sock_->h));
  r.ip();
  return r.u16();
}

IPAddress WiFiClient::localIP() {
  if (!sock_ || sock_->h < 0) return IPAddress();
  bb::Reply r = net(op(TCP_PEER).i32(sock_->h));
  r.ip();
  r.u16();
  return IPAddress(r.ip());
}

uint16_t WiFiClient::localPort() {
  if (!sock_ || sock_->h < 0) return 0;
  bb::Reply r = net(op(TCP_PEER).i32(sock_->h));
  r.ip();
  r.u16();
  r.ip();
  return r.u16();
}

// ================================================================== WiFiServer

void WiFiServer::begin(uint16_t port) {
  if (port) port_ = port;
  bb::Reply r = net(op(TCP_LISTEN).u16(port_));
  handle_ = r.i32();
}

bool WiFiServer::hasClient() {
  if (handle_ < 0) return false;
  if (pending_ >= 0) return true;
  bb::Reply r = net(op(TCP_ACCEPT).i32(handle_));
  pending_ = r.i32();
  return pending_ >= 0;
}

WiFiClient WiFiServer::available() {
  if (!hasClient()) return WiFiClient();
  int h = pending_;
  pending_ = -1;
  return WiFiClient(h);
}

void WiFiServer::stop() {
  if (handle_ >= 0) net(op(TCP_CLOSE).i32(handle_));
  handle_ = -1;
}

// ================================================================== WiFiUDP

int WiFiUDP::ensure() {
  if (handle_ < 0) begin(0);
  return handle_;
}

uint8_t WiFiUDP::begin(uint16_t port) {
  stop();
  bb::Reply r = net(op(UDP_BIND).u16(port));
  handle_ = r.i32();
  local_port_ = port;
  return handle_ >= 0 ? 1 : 0;
}

void WiFiUDP::stop() {
  if (handle_ >= 0) net(op(UDP_CLOSE).i32(handle_));
  handle_ = -1;
}

int WiFiUDP::beginPacket(IPAddress ip, uint16_t port) {
  tx_ip_ = ip;
  tx_port_ = port;
  tx_.clear();
  return 1;
}

int WiFiUDP::beginPacket(const char* host, uint16_t port) {
  uint32_t ip = bbnet::resolve(host, 4000);
  if (!ip) return 0;
  return beginPacket(IPAddress(ip), port);
}

int WiFiUDP::beginPacket() { return beginPacket(remote_ip_, remote_port_); }

size_t WiFiUDP::write(uint8_t c) {
  tx_.push_back(c);
  return 1;
}

size_t WiFiUDP::write(const uint8_t* buf, size_t size) {
  tx_.insert(tx_.end(), buf, buf + size);
  return size;
}

int WiFiUDP::endPacket() {
  if (ensure() < 0) return 0;
  bb::Reply r = net(op(UDP_SENDTO).i32(handle_).ip((uint32_t)tx_ip_).u16(tx_port_).bytes(tx_.data(), tx_.size()));
  tx_.clear();
  return r.i32() >= 0 ? 1 : 0;
}

int WiFiUDP::parsePacket() {
  if (handle_ < 0) return 0;
  bb::Reply r = net(op(UDP_RECV).i32(handle_));
  if (!r.u8()) return 0;
  remote_ip_ = IPAddress(r.ip());
  remote_port_ = r.u16();
  rx_ = r.bytes();
  rx_pos_ = 0;
  return (int)rx_.size();
}

int WiFiUDP::available() { return (int)(rx_.size() - rx_pos_); }
int WiFiUDP::read() { return rx_pos_ < rx_.size() ? rx_[rx_pos_++] : -1; }
int WiFiUDP::read(unsigned char* buf, size_t len) {
  size_t n = rx_.size() - rx_pos_;
  if (n > len) n = len;
  memcpy(buf, rx_.data() + rx_pos_, n);
  rx_pos_ += n;
  return (int)n;
}
int WiFiUDP::peek() { return rx_pos_ < rx_.size() ? rx_[rx_pos_] : -1; }

// ================================================================== SNTP

namespace {
std::string ntp_server = "pool.ntp.org";
long gmt_offset = 0;
int dst_offset = 0;
bool synced = false;
uint64_t sync_us = 0;
double sync_unix = 0;

bool ntp_sync(uint32_t timeout_ms) {
  WiFiUDP udp;
  if (!udp.begin(0)) return false;
  uint8_t req[48] = {0};
  req[0] = 0x23;  // LI 0, VN 4, client
  uint64_t deadline = bb::now_us() + (uint64_t)timeout_ms * 1000;
  while (bb::now_us() < deadline) {
    if (!udp.beginPacket(ntp_server.c_str(), 123)) return false;
    udp.write(req, sizeof req);
    udp.endPacket();
    uint64_t retry = bb::now_us() + 1000000;
    while (bb::now_us() < retry && bb::now_us() < deadline) {
      if (udp.parsePacket() >= 48) {
        uint8_t b[48];
        udp.read(b, 48);
        uint32_t secs = (uint32_t)b[40] << 24 | (uint32_t)b[41] << 16 | (uint32_t)b[42] << 8 | b[43];
        uint32_t frac = (uint32_t)b[44] << 24 | (uint32_t)b[45] << 16 | (uint32_t)b[46] << 8 | b[47];
        sync_unix = (double)secs - 2208988800.0 + frac / 4294967296.0;
        sync_us = bb::now_us();
        synced = true;
        return true;
      }
      delay(10);
    }
  }
  return false;
}
}  // namespace

void configTime(long gmtOffset_sec, int daylightOffset_sec, const char* server1, const char*, const char*) {
  gmt_offset = gmtOffset_sec;
  dst_offset = daylightOffset_sec;
  if (server1) ntp_server = server1;
  synced = false;
  if (WiFi.status() == WL_CONNECTED) ntp_sync(2000);
}

void configTzTime(const char*, const char* server1, const char*, const char*) { configTime(0, 0, server1); }

bool getLocalTime(struct tm* info, uint32_t ms) {
  if (!synced && !ntp_sync(ms)) return false;
  double now = sync_unix + (double)(micros() - sync_us) / 1e6 + gmt_offset + dst_offset;
  time_t t = (time_t)now;
  gmtime_r(&t, info);
  return true;
}

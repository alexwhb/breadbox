// WiFi.h — ESP32 Arduino WiFi API (station mode) on Breadbox's virtual network.
#pragma once

#include <memory>
#include <vector>

#include "Arduino.h"
#include "Client.h"
#include "Server.h"
#include "Udp.h"

typedef enum {
  WL_NO_SHIELD = 255,
  WL_IDLE_STATUS = 0,
  WL_NO_SSID_AVAIL = 1,
  WL_SCAN_COMPLETED = 2,
  WL_CONNECTED = 3,
  WL_CONNECT_FAILED = 4,
  WL_CONNECTION_LOST = 5,
  WL_DISCONNECTED = 6
} wl_status_t;

typedef enum { WIFI_OFF = 0, WIFI_STA = 1, WIFI_AP = 2, WIFI_AP_STA = 3 } wifi_mode_t;
#define WIFI_MODE_NULL WIFI_OFF
#define WIFI_MODE_STA WIFI_STA
#define WIFI_MODE_AP WIFI_AP
#define WIFI_MODE_APSTA WIFI_AP_STA

typedef enum { WIFI_AUTH_OPEN = 0, WIFI_AUTH_WEP, WIFI_AUTH_WPA_PSK, WIFI_AUTH_WPA2_PSK, WIFI_AUTH_WPA_WPA2_PSK } wifi_auth_mode_t;

#define WIFI_SCAN_RUNNING (-1)
#define WIFI_SCAN_FAILED (-2)

namespace bbnet {
struct Socket;
uint32_t resolve(const char* host, uint32_t timeout_ms);  // 0 = failed
}  // namespace bbnet

class WiFiClient : public Client {
 public:
  WiFiClient();
  explicit WiFiClient(int handle);
  ~WiFiClient() override;
  int connect(IPAddress ip, uint16_t port) override;
  int connect(IPAddress ip, uint16_t port, int32_t timeout_ms);
  int connect(const char* host, uint16_t port) override;
  int connect(const char* host, uint16_t port, int32_t timeout_ms);
  size_t write(uint8_t c) override;
  size_t write(const uint8_t* buf, size_t size) override;
  using Print::write;
  int available() override;
  int read() override;
  int read(uint8_t* buf, size_t size) override;
  int peek() override;
  void flush() override {}
  void stop() override;
  uint8_t connected() override;
  operator bool() override { return connected(); }
  bool operator==(const WiFiClient& o) const { return sock_ == o.sock_; }
  IPAddress remoteIP();
  uint16_t remotePort();
  IPAddress localIP();
  uint16_t localPort();
  void setNoDelay(bool) {}
  int setTimeout(uint32_t seconds) {
    connect_timeout_ms_ = seconds * 1000;
    Stream::setTimeout(seconds * 1000);
    return 0;
  }

 private:
  bool fill();
  std::shared_ptr<bbnet::Socket> sock_;
  uint32_t connect_timeout_ms_ = 3000;
};

class WiFiServer : public Server {
 public:
  explicit WiFiServer(uint16_t port = 80, uint8_t max_clients = 4) : port_(port) { (void)max_clients; }
  void begin(uint16_t port = 0) override;
  WiFiClient available();
  WiFiClient accept() { return available(); }
  bool hasClient();
  void stop();
  void close() { stop(); }
  void end() { stop(); }
  size_t write(uint8_t) override { return 0; }
  using Print::write;
  operator bool() { return handle_ >= 0; }
  void setNoDelay(bool) {}

 private:
  uint16_t port_;
  int handle_ = -1;
  int pending_ = -1;
};

class WiFiUDP : public UDP {
 public:
  WiFiUDP() {}
  ~WiFiUDP() override { stop(); }
  uint8_t begin(uint16_t port) override;
  uint8_t begin(IPAddress, uint16_t port) { return begin(port); }
  uint8_t beginMulticast(IPAddress, uint16_t port) { return begin(port); }
  void stop() override;
  int beginPacket(IPAddress ip, uint16_t port) override;
  int beginPacket(const char* host, uint16_t port) override;
  int beginPacket();
  int endPacket() override;
  size_t write(uint8_t c) override;
  size_t write(const uint8_t* buf, size_t size) override;
  using Print::write;
  int parsePacket() override;
  int available() override;
  int read() override;
  int read(unsigned char* buf, size_t len) override;
  int read(char* buf, size_t len) override { return read((unsigned char*)buf, len); }
  int peek() override;
  void flush() override { rx_.clear(); }
  IPAddress remoteIP() override { return remote_ip_; }
  uint16_t remotePort() override { return remote_port_; }

 private:
  int ensure();
  int handle_ = -1;
  uint16_t local_port_ = 0;
  IPAddress tx_ip_;
  uint16_t tx_port_ = 0;
  std::vector<uint8_t> tx_;
  std::vector<uint8_t> rx_;
  size_t rx_pos_ = 0;
  IPAddress remote_ip_;
  uint16_t remote_port_ = 0;
};
typedef WiFiUDP WiFiUdp;

class WiFiClass {
 public:
  wl_status_t begin(const char* ssid, const char* passphrase = nullptr, int32_t channel = 0, const uint8_t* bssid = nullptr,
                    bool connect = true);
  wl_status_t begin(const String& ssid, const String& pass) { return begin(ssid.c_str(), pass.c_str()); }
  wl_status_t status();
  bool isConnected() { return status() == WL_CONNECTED; }
  bool disconnect(bool wifioff = false, bool eraseap = false);
  bool reconnect();
  bool mode(wifi_mode_t m) {
    mode_ = m;
    return true;
  }
  wifi_mode_t getMode() { return mode_; }
  bool config(IPAddress local_ip, IPAddress gateway, IPAddress subnet, IPAddress dns1 = IPAddress(), IPAddress dns2 = IPAddress());
  bool setHostname(const char* name);
  const char* getHostname() { return hostname_.c_str(); }
  bool setAutoReconnect(bool) { return true; }
  bool getAutoReconnect() { return true; }
  bool setAutoConnect(bool) { return true; }
  void persistent(bool) {}
  bool setSleep(bool) { return true; }
  void setTxPower(int) {}
  uint8_t waitForConnectResult(unsigned long timeout_ms = 60000);
  IPAddress localIP();
  IPAddress gatewayIP();
  IPAddress subnetMask();
  IPAddress dnsIP(uint8_t = 0);
  IPAddress broadcastIP();
  String macAddress();
  uint8_t* macAddress(uint8_t* mac);
  String SSID();
  String psk() { return pass_; }
  String BSSIDstr();
  uint8_t* BSSID();
  int32_t RSSI();
  int32_t channel();
  int hostByName(const char* host, IPAddress& result);
  int16_t scanNetworks(bool async = false, bool show_hidden = false);
  int16_t scanComplete() { return (int16_t)scan_.size(); }
  void scanDelete() { scan_.clear(); }
  String SSID(uint8_t i);
  int32_t RSSI(uint8_t i);
  wifi_auth_mode_t encryptionType(uint8_t i);
  int32_t channel(uint8_t i);
  String BSSIDstr(uint8_t i);

 private:
  struct Status {
    uint8_t wl = WL_IDLE_STATUS;
    uint32_t ip = 0, gw = 0, mask = 0, dns = 0;
    int32_t rssi = 0;
    uint8_t channel = 0;
    uint8_t bssid[6] = {0};
    String ssid;
  };
  Status refresh();
  struct ScanEntry {
    String ssid;
    int32_t rssi;
    uint8_t channel;
    bool secure;
    uint8_t bssid[6];
  };
  std::vector<ScanEntry> scan_;
  wifi_mode_t mode_ = WIFI_STA;
  String hostname_ = "esp32";
  String pass_;
  uint8_t bssid_[6] = {0};
};

extern WiFiClass WiFi;

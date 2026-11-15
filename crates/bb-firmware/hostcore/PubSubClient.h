// PubSubClient.h — API-compatible with Nick O'Leary's PubSubClient (MQTT 3.1.1 over any
// Client). Real MQTT bytes go over the simulated TCP connection to the broker.
#pragma once

#include <functional>

#include "Arduino.h"
#include "Client.h"
#include "IPAddress.h"

#define MQTT_VERSION_3_1_1 4
#ifndef MQTT_MAX_PACKET_SIZE
#define MQTT_MAX_PACKET_SIZE 256
#endif
#ifndef MQTT_KEEPALIVE
#define MQTT_KEEPALIVE 15
#endif
#ifndef MQTT_SOCKET_TIMEOUT
#define MQTT_SOCKET_TIMEOUT 15
#endif

#define MQTT_CONNECTION_TIMEOUT -4
#define MQTT_CONNECTION_LOST -3
#define MQTT_CONNECT_FAILED -2
#define MQTT_DISCONNECTED -1
#define MQTT_CONNECTED 0
#define MQTT_CONNECT_BAD_PROTOCOL 1
#define MQTT_CONNECT_BAD_CLIENT_ID 2
#define MQTT_CONNECT_UNAVAILABLE 3
#define MQTT_CONNECT_BAD_CREDENTIALS 4
#define MQTT_CONNECT_UNAUTHORIZED 5

#define MQTTCONNECT (1 << 4)
#define MQTTPUBLISH (3 << 4)

#define MQTT_CALLBACK_SIGNATURE std::function<void(char*, uint8_t*, unsigned int)> callback

class PubSubClient : public Print {
 public:
  PubSubClient() {}
  explicit PubSubClient(Client& client) : client_(&client) {}
  PubSubClient(IPAddress ip, uint16_t port, Client& client) : client_(&client) { setServer(ip, port); }
  PubSubClient(const char* domain, uint16_t port, Client& client) : client_(&client) { setServer(domain, port); }
  PubSubClient(const char* domain, uint16_t port, MQTT_CALLBACK_SIGNATURE, Client& client) : client_(&client) {
    setServer(domain, port);
    setCallback(callback);
  }

  PubSubClient& setServer(IPAddress ip, uint16_t port) {
    ip_ = ip;
    domain_.clear();
    port_ = port;
    return *this;
  }
  PubSubClient& setServer(const uint8_t* ip, uint16_t port) { return setServer(IPAddress(ip), port); }
  PubSubClient& setServer(const char* domain, uint16_t port) {
    domain_ = domain ? domain : "";
    port_ = port;
    return *this;
  }
  PubSubClient& setCallback(MQTT_CALLBACK_SIGNATURE) {
    callback_ = callback;
    return *this;
  }
  PubSubClient& setClient(Client& c) {
    client_ = &c;
    return *this;
  }
  PubSubClient& setKeepAlive(uint16_t s) {
    keep_alive_ = s;
    return *this;
  }
  PubSubClient& setSocketTimeout(uint16_t s) {
    socket_timeout_ = s;
    return *this;
  }
  bool setBufferSize(uint16_t size) {
    buffer_size_ = size;
    return true;
  }
  uint16_t getBufferSize() { return buffer_size_; }

  bool connect(const char* id) { return connect(id, nullptr, nullptr, nullptr, 0, false, nullptr, true); }
  bool connect(const char* id, const char* user, const char* pass) { return connect(id, user, pass, nullptr, 0, false, nullptr, true); }
  bool connect(const char* id, const char* willTopic, uint8_t willQos, bool willRetain, const char* willMessage) {
    return connect(id, nullptr, nullptr, willTopic, willQos, willRetain, willMessage, true);
  }
  bool connect(const char* id, const char* user, const char* pass, const char* willTopic, uint8_t willQos, bool willRetain,
               const char* willMessage) {
    return connect(id, user, pass, willTopic, willQos, willRetain, willMessage, true);
  }
  bool connect(const char* id, const char* user, const char* pass, const char* willTopic, uint8_t willQos, bool willRetain,
               const char* willMessage, bool cleanSession);
  void disconnect();
  bool publish(const char* topic, const char* payload) { return publish(topic, (const uint8_t*)payload, payload ? strlen(payload) : 0, false); }
  bool publish(const char* topic, const char* payload, bool retained) {
    return publish(topic, (const uint8_t*)payload, payload ? strlen(payload) : 0, retained);
  }
  bool publish(const char* topic, const uint8_t* payload, unsigned int len) { return publish(topic, payload, len, false); }
  bool publish(const char* topic, const uint8_t* payload, unsigned int len, bool retained);
  bool publish_P(const char* topic, const char* payload, bool retained) { return publish(topic, payload, retained); }
  bool subscribe(const char* topic) { return subscribe(topic, 0); }
  bool subscribe(const char* topic, uint8_t qos);
  bool unsubscribe(const char* topic);
  bool loop();
  bool connected();
  int state() { return state_; }
  size_t write(uint8_t) override { return 0; }
  using Print::write;

 private:
  bool send_packet(const std::vector<uint8_t>& p);
  bool read_packet(std::vector<uint8_t>& out, uint32_t timeout_ms);
  uint16_t next_id();

  Client* client_ = nullptr;
  IPAddress ip_;
  std::string domain_;
  uint16_t port_ = 1883;
  std::function<void(char*, uint8_t*, unsigned int)> callback_;
  uint16_t keep_alive_ = MQTT_KEEPALIVE;
  uint16_t socket_timeout_ = MQTT_SOCKET_TIMEOUT;
  uint16_t buffer_size_ = MQTT_MAX_PACKET_SIZE;
  int state_ = MQTT_DISCONNECTED;
  uint16_t msg_id_ = 0;
  unsigned long last_out_ = 0;
  unsigned long last_in_ = 0;
  bool ping_outstanding_ = false;
  std::vector<uint8_t> rxbuf_;
};

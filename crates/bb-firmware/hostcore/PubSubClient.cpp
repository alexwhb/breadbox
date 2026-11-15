#include "PubSubClient.h"

#include <vector>

namespace {
void put_len(std::vector<uint8_t>& v, size_t len) {
  do {
    uint8_t b = len % 128;
    len /= 128;
    if (len) b |= 0x80;
    v.push_back(b);
  } while (len);
}
void put_str(std::vector<uint8_t>& v, const char* s, size_t n) {
  v.push_back((uint8_t)(n >> 8));
  v.push_back((uint8_t)(n & 0xff));
  v.insert(v.end(), s, s + n);
}
void put_str(std::vector<uint8_t>& v, const char* s) { put_str(v, s, s ? strlen(s) : 0); }
std::vector<uint8_t> frame(uint8_t first, const std::vector<uint8_t>& body) {
  std::vector<uint8_t> p;
  p.push_back(first);
  put_len(p, body.size());
  p.insert(p.end(), body.begin(), body.end());
  return p;
}
}  // namespace

uint16_t PubSubClient::next_id() {
  if (++msg_id_ == 0) msg_id_ = 1;
  return msg_id_;
}

bool PubSubClient::send_packet(const std::vector<uint8_t>& p) {
  if (!client_) return false;
  size_t n = client_->write(p.data(), p.size());
  last_out_ = millis();
  return n == p.size();
}

// Reads one complete packet (blocking up to `timeout_ms` of simulated time).
bool PubSubClient::read_packet(std::vector<uint8_t>& out, uint32_t timeout_ms) {
  unsigned long t0 = millis();
  for (;;) {
    while (client_->available()) {
      int c = client_->read();
      if (c < 0) break;
      rxbuf_.push_back((uint8_t)c);
    }
    if (rxbuf_.size() >= 2) {
      size_t len = 0, mult = 1, i = 1;
      bool complete = false;
      for (; i < rxbuf_.size() && i <= 4; i++) {
        len += (rxbuf_[i] & 0x7f) * mult;
        mult *= 128;
        if (!(rxbuf_[i] & 0x80)) {
          complete = true;
          break;
        }
      }
      if (complete && rxbuf_.size() >= i + 1 + len) {
        out.assign(rxbuf_.begin(), rxbuf_.begin() + (long)(i + 1 + len));
        rxbuf_.erase(rxbuf_.begin(), rxbuf_.begin() + (long)(i + 1 + len));
        last_in_ = millis();
        return true;
      }
    }
    if (timeout_ms == 0 || millis() - t0 >= timeout_ms || !client_->connected()) return false;
    delay(1);
  }
}

bool PubSubClient::connect(const char* id, const char* user, const char* pass, const char* willTopic, uint8_t willQos,
                           bool willRetain, const char* willMessage, bool cleanSession) {
  if (!client_) return false;
  if (connected()) return true;
  int ok = domain_.empty() ? client_->connect(ip_, port_) : client_->connect(domain_.c_str(), port_);
  if (!ok) {
    state_ = MQTT_CONNECT_FAILED;
    return false;
  }
  rxbuf_.clear();
  std::vector<uint8_t> b;
  put_str(b, "MQTT");
  b.push_back(MQTT_VERSION_3_1_1);
  uint8_t flags = cleanSession ? 0x02 : 0;
  if (willTopic) flags |= 0x04 | (uint8_t)((willQos & 3) << 3) | (willRetain ? 0x20 : 0);
  if (user) flags |= 0x80;
  if (pass) flags |= 0x40;
  b.push_back(flags);
  b.push_back((uint8_t)(keep_alive_ >> 8));
  b.push_back((uint8_t)(keep_alive_ & 0xff));
  put_str(b, id);
  if (willTopic) {
    put_str(b, willTopic);
    put_str(b, willMessage);
  }
  if (user) put_str(b, user);
  if (pass) put_str(b, pass);
  if (!send_packet(frame(0x10, b))) {
    state_ = MQTT_CONNECTION_LOST;
    return false;
  }
  std::vector<uint8_t> p;
  if (!read_packet(p, socket_timeout_ * 1000UL)) {
    state_ = MQTT_CONNECTION_TIMEOUT;
    client_->stop();
    return false;
  }
  if (p.size() >= 4 && p[0] == 0x20 && p[3] == 0) {
    state_ = MQTT_CONNECTED;
    ping_outstanding_ = false;
    last_in_ = last_out_ = millis();
    return true;
  }
  state_ = p.size() >= 4 ? p[3] : MQTT_CONNECT_FAILED;
  client_->stop();
  return false;
}

void PubSubClient::disconnect() {
  if (client_ && client_->connected()) send_packet({0xe0, 0x00});
  if (client_) client_->stop();
  state_ = MQTT_DISCONNECTED;
}

bool PubSubClient::publish(const char* topic, const uint8_t* payload, unsigned int len, bool retained) {
  if (!connected()) return false;
  size_t tl = strlen(topic);
  if (tl + len + 7 > buffer_size_) return false;  // PubSubClient's buffer limit
  std::vector<uint8_t> b;
  put_str(b, topic, tl);
  b.insert(b.end(), payload, payload + len);
  return send_packet(frame(0x30 | (retained ? 1 : 0), b));
}

bool PubSubClient::subscribe(const char* topic, uint8_t qos) {
  if (!connected() || qos > 1) return false;
  std::vector<uint8_t> b;
  uint16_t id = next_id();
  b.push_back((uint8_t)(id >> 8));
  b.push_back((uint8_t)(id & 0xff));
  put_str(b, topic);
  b.push_back(qos);
  return send_packet(frame(0x82, b));
}

bool PubSubClient::unsubscribe(const char* topic) {
  if (!connected()) return false;
  std::vector<uint8_t> b;
  uint16_t id = next_id();
  b.push_back((uint8_t)(id >> 8));
  b.push_back((uint8_t)(id & 0xff));
  put_str(b, topic);
  return send_packet(frame(0xa2, b));
}

bool PubSubClient::loop() {
  if (!connected()) return false;
  unsigned long now = millis();
  unsigned long ka = keep_alive_ * 1000UL;
  if (ka && (now - last_in_ > ka || now - last_out_ > ka)) {
    if (ping_outstanding_) {
      state_ = MQTT_CONNECTION_TIMEOUT;
      client_->stop();
      return false;
    }
    send_packet({0xc0, 0x00});
    last_in_ = now;
    ping_outstanding_ = true;
  }
  std::vector<uint8_t> p;
  while (read_packet(p, 0)) {
    uint8_t type = p[0] & 0xf0;
    size_t i = 1;
    while (p[i] & 0x80) i++;
    i++;
    if (type == 0x30) {
      uint16_t tl = (uint16_t)(p[i] << 8 | p[i + 1]);
      std::string topic((const char*)&p[i + 2], tl);
      size_t off = i + 2 + tl;
      uint8_t qos = (p[0] >> 1) & 3;
      uint16_t id = 0;
      if (qos) {
        id = (uint16_t)(p[off] << 8 | p[off + 1]);
        off += 2;
      }
      std::vector<uint8_t> payload(p.begin() + (long)off, p.end());
      payload.push_back(0);
      if (callback_) callback_(&topic[0], payload.data(), (unsigned int)(payload.size() - 1));
      if (qos == 1) send_packet({0x40, 0x02, (uint8_t)(id >> 8), (uint8_t)(id & 0xff)});
    } else if (type == 0xd0) {
      ping_outstanding_ = false;
    } else if (type == 0xc0) {
      send_packet({0xd0, 0x00});
    }
  }
  return connected();
}

bool PubSubClient::connected() {
  if (!client_) return false;
  bool c = client_->connected();
  if (!c) {
    if (state_ == MQTT_CONNECTED) {
      state_ = MQTT_CONNECTION_LOST;
      client_->stop();
    }
    return false;
  }
  return state_ == MQTT_CONNECTED;
}

// mqtt_lite.h — just enough MQTT 3.1.1 to publish from an UNO: builds CONNECT, PUBLISH (QoS 0)
// and PINGREQ packets into a caller-supplied buffer and recognises CONNACK. The bytes go to the
// broker through the ESP-01's TCP link (see wifi_at.h).
#pragma once
#include <Arduino.h>

namespace MqttLite {

// Remaining-length varint; returns bytes written.
inline uint8_t putLength(uint8_t* p, uint16_t len) {
  uint8_t n = 0;
  do {
    uint8_t b = len % 128;
    len /= 128;
    if (len) b |= 0x80;
    p[n++] = b;
  } while (len);
  return n;
}

inline uint16_t putString(uint8_t* p, const char* s) {
  uint16_t n = strlen(s);
  p[0] = n >> 8;
  p[1] = n & 0xff;
  memcpy(p + 2, s, n);
  return n + 2;
}

// CONNECT with a clean session and no credentials. Returns the packet length (0 if `cap` is
// too small).
inline uint16_t connect(uint8_t* buf, uint16_t cap, const char* clientId, uint16_t keepAlive) {
  uint16_t body = 10 + 2 + strlen(clientId);
  if (body + 3 > cap) return 0;
  uint16_t i = 0;
  buf[i++] = 0x10;
  i += putLength(buf + i, body);
  i += putString(buf + i, "MQTT");
  buf[i++] = 4;     // protocol level 3.1.1
  buf[i++] = 0x02;  // clean session
  buf[i++] = keepAlive >> 8;
  buf[i++] = keepAlive & 0xff;
  i += putString(buf + i, clientId);
  return i;
}

// PUBLISH at QoS 0.
inline uint16_t publish(uint8_t* buf, uint16_t cap, const char* topic, const char* payload) {
  uint16_t plen = strlen(payload);
  uint16_t body = 2 + strlen(topic) + plen;
  if (body + 3 > cap) return 0;
  uint16_t i = 0;
  buf[i++] = 0x30;
  i += putLength(buf + i, body);
  i += putString(buf + i, topic);
  memcpy(buf + i, payload, plen);
  return i + plen;
}

inline uint16_t pingreq(uint8_t* buf) {
  buf[0] = 0xC0;
  buf[1] = 0;
  return 2;
}

// CONNACK with return code 0?
inline bool connAccepted(const uint8_t* p, uint16_t n) {
  return n >= 4 && p[0] == 0x20 && p[1] == 2 && p[3] == 0;
}

}  // namespace MqttLite

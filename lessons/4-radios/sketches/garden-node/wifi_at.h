// wifi_at.h — drives an ESP-01 (ESP8266 with Espressif's AT firmware) over a serial port:
// join an access point, open a TCP link to an MQTT broker and publish through it with
// mqtt_lite.h. Blocking and small on purpose: everything fits an UNO's 2 KB of RAM.
#pragma once
#include <Arduino.h>
#include <stdarg.h>
#include "mqtt_lite.h"

// ---------------------------------------------------------------- Serial.printf for AVR
// The ESP cores give Serial a printf(); the AVR core doesn't. PrintfSerial wraps the
// hardware port and adds one so the sketch reads the same on every board.
class PrintfSerial : public Stream {
 public:
  explicit PrintfSerial(HardwareSerial& hw) : hw_(hw) {}
  void begin(unsigned long baud) { hw_.begin(baud); }
  void end() { hw_.end(); }
  int available() override { return hw_.available(); }
  int read() override { return hw_.read(); }
  int peek() override { return hw_.peek(); }
  void flush() override { hw_.flush(); }
  size_t write(uint8_t c) override { return hw_.write(c); }
  using Print::write;
  operator bool() { return true; }
  size_t printf(const char* fmt, ...) {
    char line[96];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(line, sizeof line, fmt, ap);
    va_end(ap);
    return print(line);
  }

 private:
  HardwareSerial& hw_;
};
static PrintfSerial SerialPrintf(Serial);
#define Serial SerialPrintf

// ---------------------------------------------------------------- every(ms)
// True once per `ms` milliseconds at each call site. The schedule advances by `ms` instead of
// restarting from now, so the long-run rate is exact even though loop() takes time itself.
#define every(ms)                                         \
  ([&]() -> bool {                                        \
    static unsigned long next_ = 0;                       \
    unsigned long now_ = millis();                        \
    if ((long)(now_ - next_) < 0) return false;           \
    next_ += (ms);                                        \
    if ((long)(now_ - next_) >= 0) next_ = now_ + (ms);   \
    return true;                                          \
  }())

// ---------------------------------------------------------------- garden-node telemetry
// Lux seen by a GL5528 LDR (R = 350k·lux^-0.7) on top of a 10 kΩ divider, from its ADC value.
inline int luxFromRaw(int raw) {
  if (raw <= 0) return 0;
  if (raw >= 1023) raw = 1022;
  float r = 10000.0f * (1023.0f / raw - 1.0f);
  return (int)lround(pow(350000.0f / r, 1.0f / 0.7f));
}

// ATmega328P on-chip temperature sensor (ADC8, 1.1 V reference), datasheet typical curve.
// Good to a few degrees uncalibrated; the sim reports the bench's air temperature.
inline int chipTempC() {
  ADMUX = _BV(REFS1) | _BV(REFS0) | _BV(MUX3);
  delay(2);
  ADCSRA |= _BV(ADSC);
  while (ADCSRA & _BV(ADSC)) {
  }
  uint16_t raw = ADC;
  float v = (raw + 0.5f) * 1.1f / 1024.0f;
  float t = v < 0.314f ? 25 + (v - 0.314f) / 0.0010286f : 25 + (v - 0.314f) / 0.0011f;
  return (int)lround(t);
}

// ---------------------------------------------------------------- WifiAT
class WifiAT {
 public:
  explicit WifiAT(Stream& port) : port_(port) {}

  // Waits for the module, then joins the access point. True once it has an IP.
  bool join(const char* ssid, const char* pass) {
    waitFor("ready", 2000);  // power-on boot log, then "ready"
    bool alive = false;
    for (uint8_t i = 0; i < 6 && !alive; i++) alive = command(F("AT"), "OK", 600);
    command(F("ATE0"), "OK", 600);  // no echo: SoftwareSerial is half duplex
    command(F("AT+CWMODE=1"), "OK", 1000);
    port_.print(F("AT+CWJAP=\""));
    port_.print(ssid);
    port_.print(F("\",\""));
    port_.print(pass);
    port_.print(F("\"\r\n"));
    joined_ = waitFor("OK", 20000, "FAIL");
    return joined_;
  }

  // Opens the TCP link to the broker and sends MQTT CONNECT.
  bool mqtt(const char* host, uint16_t port, const char* clientId = "garden-node") {
    host_ = host;
    mqttPort_ = port;
    clientId_ = clientId;
    return connectBroker();
  }

  // Publishes a text payload (QoS 0). Reconnects (at most every 5 s) if the link dropped.
  bool publish(const char* topic, const char* payload) {
    if (!up_ && !reconnect()) return false;
    uint8_t pkt[96];
    uint16_t n = MqttLite::publish(pkt, sizeof pkt, topic, payload);
    up_ = n && send(pkt, n);
    return up_;
  }

  // garden-node telemetry: {"m":<moisture %>,"lux":<lux>,"t":<°C>}.
  bool publish(const char* topic, int moisture, int lightRaw) {
    char json[48];
    snprintf(json, sizeof json, "{\"m\":%d,\"lux\":%d,\"t\":%d}", moisture, luxFromRaw(lightRaw), chipTempC());
    return publish(topic, json);
  }

  bool connected() const { return up_; }

 private:
  bool command(const __FlashStringHelper* cmd, const char* expect, unsigned long timeout) {
    port_.print(cmd);
    port_.print(F("\r\n"));
    return waitFor(expect, timeout, "ERROR");
  }

  // Reads until `expect` (true) or `fail` (false) appears, or the timeout passes. Notes link
  // loss ("CLOSED") on the way.
  bool waitFor(const char* expect, unsigned long timeout, const char* fail = nullptr) {
    unsigned long t0 = millis();
    while (millis() - t0 < timeout) {
      while (port_.available()) {
        push(port_.read());
        if (ends("CLOSED")) up_ = false;
        if (ends(expect)) return true;
        if (fail && ends(fail)) return false;
      }
    }
    return false;
  }

  void push(char c) {
    memmove(tail_, tail_ + 1, sizeof tail_ - 2);
    tail_[sizeof tail_ - 2] = c;
    tail_[sizeof tail_ - 1] = 0;
  }

  bool ends(const char* s) const {
    size_t n = strlen(s), have = sizeof tail_ - 1;
    return n <= have && memcmp(tail_ + have - n, s, n) == 0;
  }

  // AT+CIPSEND=<n>, wait for the '>' prompt, write the bytes, wait for SEND OK.
  bool send(const uint8_t* data, uint16_t len) {
    while (port_.available()) {
      push(port_.read());
      if (ends("CLOSED")) up_ = false;
    }
    port_.print(F("AT+CIPSEND="));
    port_.print(len);
    port_.print(F("\r\n"));
    if (!waitFor(">", 2000, "ERROR")) return false;
    port_.write(data, len);
    return waitFor("SEND OK", 4000, "ERROR");
  }

  bool connectBroker() {
    lastTry_ = millis();
    port_.print(F("AT+CIPSTART=\"TCP\",\""));
    port_.print(host_);
    port_.print(F("\","));
    port_.print(mqttPort_);
    port_.print(F("\r\n"));
    if (!waitFor("OK", 10000, "ERROR")) return up_ = false;
    uint8_t pkt[48];
    uint16_t n = MqttLite::connect(pkt, sizeof pkt, clientId_, 60);
    if (!send(pkt, n)) return up_ = false;
    // CONNACK arrives as "+IPD,4:" followed by 4 raw bytes.
    if (!waitFor("+IPD,4:", 5000)) return up_ = false;
    uint8_t ack[4];
    uint8_t got = 0;
    unsigned long t0 = millis();
    while (got < 4 && millis() - t0 < 500) {
      if (port_.available()) ack[got++] = port_.read();
    }
    up_ = MqttLite::connAccepted(ack, got);
    return up_;
  }

  bool reconnect() {
    if (!host_ || millis() - lastTry_ < 5000) return false;
    command(F("AT+CIPCLOSE"), "OK", 500);
    return connectBroker();
  }

  Stream& port_;
  const char* host_ = nullptr;
  uint16_t mqttPort_ = 1883;
  const char* clientId_ = "garden-node";
  bool joined_ = false;
  bool up_ = false;
  unsigned long lastTry_ = 0;
  char tail_[24] = {0};
};

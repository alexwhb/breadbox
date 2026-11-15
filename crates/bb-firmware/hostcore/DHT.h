// DHT.h — Adafruit DHT-library compatible driver for DHT11/DHT22, speaking the real
// single-wire protocol on the pin (the simulator's DHT part answers with timed edges).
#pragma once

#include "Arduino.h"

#define DHT11 11
#define DHT12 12
#define DHT21 21
#define DHT22 22
#define AM2301 21

class DHT {
 public:
  DHT(uint8_t pin, uint8_t type, uint8_t count = 6) : pin_(pin), type_(type) { (void)count; }
  void begin(uint8_t usec = 55) {
    (void)usec;
    pinMode(pin_, INPUT_PULLUP);
    last_ = millis() - 2000;
  }
  float readTemperature(bool S = false, bool force = false);
  float readHumidity(bool force = false);
  float convertCtoF(float c) { return c * 1.8f + 32; }
  float convertFtoC(float f) { return (f - 32) * 0.55555f; }
  float computeHeatIndex(bool isFahrenheit = true) { return computeHeatIndex(readTemperature(isFahrenheit), readHumidity(), isFahrenheit); }
  float computeHeatIndex(float temperature, float percentHumidity, bool isFahrenheit = true);
  bool read(bool force = false);

 private:
  uint8_t pin_, type_;
  uint8_t data_[5] = {0};
  unsigned long last_ = 0;
  bool ok_ = false;
  bool first_ = true;
};

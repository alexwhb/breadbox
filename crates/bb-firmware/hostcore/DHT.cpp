#include "DHT.h"

#include "bb_host.h"

bool DHT::read(bool force) {
  unsigned long now = millis();
  if (!force && !first_ && now - last_ < 2000) return ok_;
  first_ = false;
  last_ = now;
  ok_ = false;
  // Start signal: hold the line low (≥ 18 ms for DHT11, ≥ 1 ms for DHT22), then release.
  pinMode(pin_, OUTPUT);
  digitalWrite(pin_, LOW);
  if (type_ == DHT11)
    delay(20);
  else
    delayMicroseconds(1100);
  pinMode(pin_, INPUT_PULLUP);
  // Response: 80 µs low, 80 µs high, then 40 bits of 50 µs low + 26–28 µs (0) / 70 µs (1) high.
  if (!bb::wait_pin(pin_, LOW, 200)) return false;
  if (!bb::wait_pin(pin_, HIGH, 200)) return false;
  if (!bb::wait_pin(pin_, LOW, 200)) return false;
  uint8_t d[5] = {0};
  for (int i = 0; i < 40; i++) {
    if (!bb::wait_pin(pin_, HIGH, 120)) return false;
    uint64_t t0 = bb::now_us();
    if (!bb::wait_pin(pin_, LOW, 120)) return false;
    if (bb::now_us() - t0 > 48) d[i / 8] |= (uint8_t)(0x80 >> (i % 8));
  }
  if (((d[0] + d[1] + d[2] + d[3]) & 0xff) != d[4]) return false;
  memcpy(data_, d, 5);
  ok_ = true;
  return true;
}

float DHT::readTemperature(bool S, bool force) {
  if (!read(force)) return NAN;
  float t;
  if (type_ == DHT11) {
    t = data_[2] + (data_[3] & 0x0f) * 0.1f;
    if (data_[3] & 0x80) t = -t;
  } else {
    t = (float)(((uint16_t)(data_[2] & 0x7f) << 8) | data_[3]) * 0.1f;
    if (data_[2] & 0x80) t = -t;
  }
  return S ? convertCtoF(t) : t;
}

float DHT::readHumidity(bool force) {
  if (!read(force)) return NAN;
  if (type_ == DHT11) return data_[0] + data_[1] * 0.1f;
  return (float)(((uint16_t)data_[0] << 8) | data_[1]) * 0.1f;
}

float DHT::computeHeatIndex(float t, float h, bool isF) {
  if (!isF) t = convertCtoF(t);
  float hi = 0.5f * (t + 61.0f + ((t - 68.0f) * 1.2f) + (h * 0.094f));
  if (hi > 79) {
    hi = -42.379f + 2.04901523f * t + 10.14333127f * h - 0.22475541f * t * h - 0.00683783f * t * t - 0.05481717f * h * h +
         0.00122874f * t * t * h + 0.00085282f * t * h * h - 0.00000199f * t * t * h * h;
  }
  return isF ? hi : convertFtoC(hi);
}

// Wire.h — I²C master. Each transaction is one message; the simulator's I²C devices answer.
#pragma once

#include <vector>

#include "Arduino.h"

class TwoWire : public Stream {
 public:
  explicit TwoWire(uint8_t bus) : bus_(bus) {}
  bool begin(int sda = -1, int scl = -1, uint32_t frequency = 0) {
    (void)sda;
    (void)scl;
    if (frequency) freq_ = frequency;
    return true;
  }
  bool begin(uint8_t /*slave address*/) { return true; }
  bool end() { return true; }
  bool setPins(int, int) { return true; }
  bool setClock(uint32_t f) {
    freq_ = f;
    return true;
  }
  uint32_t getClock() { return freq_; }
  void setTimeOut(uint16_t) {}
  void beginTransmission(uint16_t address) {
    addr_ = (uint8_t)address;
    tx_.clear();
  }
  void beginTransmission(int address) { beginTransmission((uint16_t)address); }
  uint8_t endTransmission(bool sendStop = true);
  size_t requestFrom(uint16_t address, size_t size, bool sendStop = true);
  uint8_t requestFrom(uint8_t address, uint8_t size, uint8_t sendStop) { return (uint8_t)requestFrom((uint16_t)address, (size_t)size, sendStop != 0); }
  uint8_t requestFrom(int address, int size) { return (uint8_t)requestFrom((uint16_t)address, (size_t)size, true); }
  size_t write(uint8_t c) override {
    tx_.push_back(c);
    return 1;
  }
  size_t write(const uint8_t* b, size_t n) override {
    tx_.insert(tx_.end(), b, b + n);
    return n;
  }
  using Print::write;
  int available() override { return (int)(rx_.size() - rx_pos_); }
  int read() override { return rx_pos_ < rx_.size() ? rx_[rx_pos_++] : -1; }
  int peek() override { return rx_pos_ < rx_.size() ? rx_[rx_pos_] : -1; }
  void flush() override {}
  void onReceive(void (*)(int)) {}
  void onRequest(void (*)(void)) {}

 private:
  uint8_t bus_;
  uint8_t addr_ = 0;
  uint32_t freq_ = 100000;
  std::vector<uint8_t> tx_, rx_;
  size_t rx_pos_ = 0;
};

extern TwoWire Wire;
extern TwoWire Wire1;

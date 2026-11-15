// SPI.h — SPI master; chip select is ordinary GPIO.
#pragma once

#include "Arduino.h"

#define SPI_MODE0 0x00
#define SPI_MODE1 0x01
#define SPI_MODE2 0x02
#define SPI_MODE3 0x03

class SPISettings {
 public:
  SPISettings() {}
  SPISettings(uint32_t clock, uint8_t bitOrder, uint8_t dataMode) : clock_(clock), order_(bitOrder), mode_(dataMode) {}
  uint32_t clock_ = 1000000;
  uint8_t order_ = MSBFIRST;
  uint8_t mode_ = SPI_MODE0;
};

class SPIClass {
 public:
  explicit SPIClass(uint8_t bus = 0) : bus_(bus) {}
  void begin(int8_t sck = -1, int8_t miso = -1, int8_t mosi = -1, int8_t ss = -1) {
    (void)sck;
    (void)miso;
    (void)mosi;
    (void)ss;
  }
  void end() {}
  void beginTransaction(SPISettings s) { settings_ = s; }
  void endTransaction() {}
  void setFrequency(uint32_t f) { settings_.clock_ = f; }
  void setDataMode(uint8_t m) { settings_.mode_ = m; }
  void setBitOrder(uint8_t o) { settings_.order_ = o; }
  uint8_t transfer(uint8_t data);
  uint16_t transfer16(uint16_t data);
  void transfer(void* buf, size_t count);
  void transferBytes(const uint8_t* out, uint8_t* in, uint32_t size);
  void writeBytes(const uint8_t* data, uint32_t size) { transferBytes(data, nullptr, size); }
  void write(uint8_t d) { transfer(d); }

 private:
  uint8_t bus_;
  SPISettings settings_;
};

extern SPIClass SPI;

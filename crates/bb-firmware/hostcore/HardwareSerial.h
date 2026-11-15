// HardwareSerial.h — UARTs. Output goes to the simulator's serial monitor (and to whatever is
// wired to the TX pin); input comes from the serial monitor's send box / wired modules.
#pragma once

#include <vector>

#include "Stream.h"

#define SERIAL_8N1 0x800001c
#define SERIAL_8E1 0x800001e
#define SERIAL_8O1 0x800001f

class HardwareSerial : public Stream {
 public:
  explicit HardwareSerial(uint8_t port) : port_(port) {}
  void begin(unsigned long baud, uint32_t config = SERIAL_8N1, int8_t rxPin = -1, int8_t txPin = -1, bool invert = false,
             unsigned long timeout_ms = 20000UL, uint8_t rxfifo_full_thrhd = 112);
  void end() {}
  int available() override;
  int peek() override;
  int read() override;
  size_t read(uint8_t* buf, size_t n);
  size_t write(uint8_t c) override;
  size_t write(const uint8_t* buf, size_t n) override;
  using Print::write;
  int availableForWrite() override { return 128; }
  void flush() override {}
  void setDebugOutput(bool) {}
  void setRxBufferSize(size_t) {}
  void setTxBufferSize(size_t) {}
  uint32_t baudRate() const { return baud_; }
  operator bool() const { return true; }

 private:
  void fill();
  uint8_t port_;
  uint32_t baud_ = 0;
  std::vector<uint8_t> rx_;
  size_t rx_pos_ = 0;
};

extern HardwareSerial Serial;
extern HardwareSerial Serial1;
extern HardwareSerial Serial2;

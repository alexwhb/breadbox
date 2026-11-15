#include "HardwareSerial.h"

#include "Arduino.h"
#include "bb_host.h"

HardwareSerial Serial(0);
HardwareSerial Serial1(1);
HardwareSerial Serial2(2);

void HardwareSerial::begin(unsigned long baud, uint32_t, int8_t rxPin, int8_t txPin, bool, unsigned long, uint8_t) {
  baud_ = (uint32_t)baud;
  // rx / tx < 0 = the port's default pins (0xff on the wire); ESP32 routes any GPIO
  bb::send(bb::MSG_SERIAL_BEGIN, bb::Buf().u8(port_).u32(baud_).u8((uint8_t)rxPin).u8((uint8_t)txPin));
}

void HardwareSerial::fill() {
  if (rx_pos_ < rx_.size()) return;
  rx_.clear();
  rx_pos_ = 0;
  bb::Reply r = bb::rpc(bb::MSG_SERIAL_READ, bb::Buf().u8(port_).u16(256));
  rx_ = r.bytes();
}

int HardwareSerial::available() {
  fill();
  return (int)(rx_.size() - rx_pos_);
}

int HardwareSerial::peek() {
  fill();
  return rx_pos_ < rx_.size() ? rx_[rx_pos_] : -1;
}

int HardwareSerial::read() {
  fill();
  return rx_pos_ < rx_.size() ? rx_[rx_pos_++] : -1;
}

size_t HardwareSerial::read(uint8_t* buf, size_t n) {
  size_t k = 0;
  while (k < n) {
    int c = read();
    if (c < 0) break;
    buf[k++] = (uint8_t)c;
  }
  return k;
}

size_t HardwareSerial::write(uint8_t c) { return write(&c, 1); }

size_t HardwareSerial::write(const uint8_t* buf, size_t n) {
  bb::Buf b;
  b.u8(port_);
  b.b.insert(b.b.end(), buf, buf + n);
  bb::send(bb::MSG_SERIAL_WRITE, b);
  return n;
}

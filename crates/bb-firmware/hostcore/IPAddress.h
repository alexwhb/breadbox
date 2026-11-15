// IPAddress.h — IPv4 address (bytes in network order).
#pragma once

#include <stdint.h>

#include "Printable.h"
#include "WString.h"

class IPAddress : public Printable {
 public:
  IPAddress() { b_[0] = b_[1] = b_[2] = b_[3] = 0; }
  IPAddress(uint8_t a, uint8_t b, uint8_t c, uint8_t d) {
    b_[0] = a;
    b_[1] = b;
    b_[2] = c;
    b_[3] = d;
  }
  // `a` packed as on ESP32: first octet in the low byte.
  IPAddress(uint32_t a) {
    for (int i = 0; i < 4; i++) b_[i] = (a >> (8 * i)) & 0xff;
  }
  IPAddress(const uint8_t* p) {
    for (int i = 0; i < 4; i++) b_[i] = p[i];
  }
  bool fromString(const char* s);
  bool fromString(const String& s) { return fromString(s.c_str()); }
  operator uint32_t() const { return (uint32_t)b_[0] | (uint32_t)b_[1] << 8 | (uint32_t)b_[2] << 16 | (uint32_t)b_[3] << 24; }
  bool operator==(const IPAddress& o) const { return (uint32_t) * this == (uint32_t)o; }
  bool operator!=(const IPAddress& o) const { return !(*this == o); }
  bool operator==(const uint8_t* p) const { return b_[0] == p[0] && b_[1] == p[1] && b_[2] == p[2] && b_[3] == p[3]; }
  uint8_t operator[](int i) const { return b_[i & 3]; }
  uint8_t& operator[](int i) { return b_[i & 3]; }
  String toString() const;
  size_t printTo(Print& p) const override;

 private:
  uint8_t b_[4];
};

extern const IPAddress INADDR_NONE;

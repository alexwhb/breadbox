// Print.h — Arduino Print.
#pragma once

#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>

#include "Printable.h"
#include "WString.h"

#define DEC 10
#define HEX 16
#define OCT 8
#define BIN 2

class Print {
 public:
  virtual ~Print() {}
  virtual size_t write(uint8_t c) = 0;
  virtual size_t write(const uint8_t* buf, size_t n) {
    size_t k = 0;
    while (n--) k += write(*buf++);
    return k;
  }
  size_t write(const char* s) { return s ? write((const uint8_t*)s, strlen(s)) : 0; }
  size_t write(const char* buf, size_t n) { return write((const uint8_t*)buf, n); }
  // Integer literals (`write(0x00)`) pick these instead of the const char* overload.
  size_t write(int n) { return write((uint8_t)n); }
  size_t write(unsigned int n) { return write((uint8_t)n); }
  size_t write(long n) { return write((uint8_t)n); }
  size_t write(unsigned long n) { return write((uint8_t)n); }
  virtual int availableForWrite() { return 0; }
  virtual void flush() {}

  size_t print(const __FlashStringHelper* f) { return write(reinterpret_cast<const char*>(f)); }
  size_t print(const String& s) { return write((const uint8_t*)s.c_str(), s.length()); }
  size_t print(const char* s) { return write(s); }
  size_t print(char c) { return write((uint8_t)c); }
  size_t print(unsigned char v, int base = DEC) { return print((unsigned long)v, base); }
  size_t print(int v, int base = DEC) { return print((long)v, base); }
  size_t print(unsigned int v, int base = DEC) { return print((unsigned long)v, base); }
  size_t print(long v, int base = DEC);
  size_t print(unsigned long v, int base = DEC);
  size_t print(long long v, int base = DEC);
  size_t print(unsigned long long v, int base = DEC);
  size_t print(double v, int digits = 2);
  size_t print(const Printable& p) { return p.printTo(*this); }

  size_t println() { return write("\r\n"); }
  template <typename T>
  size_t println(const T& v) {
    size_t n = print(v);
    return n + println();
  }
  template <typename T>
  size_t println(const T& v, int fmt) {
    size_t n = print(v, fmt);
    return n + println();
  }
  size_t printf(const char* fmt, ...) __attribute__((format(printf, 2, 3)));

 private:
  size_t printNumber(unsigned long long n, uint8_t base);
};

#include "Print.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

#include <string>

size_t Print::printNumber(unsigned long long n, uint8_t base) {
  if (base < 2) base = 10;
  char buf[66];
  char* p = &buf[sizeof buf - 1];
  *p = 0;
  do {
    int d = (int)(n % base);
    n /= base;
    *--p = (char)(d < 10 ? '0' + d : 'A' + d - 10);
  } while (n);
  return write(p);
}

size_t Print::print(long v, int base) {
  if (base == 10 && v < 0) return write('-') + printNumber((unsigned long long)(-(long long)v), 10);
  return printNumber(base == 10 ? (unsigned long long)v : (unsigned long)v, (uint8_t)base);
}
size_t Print::print(unsigned long v, int base) { return printNumber(v, (uint8_t)base); }
size_t Print::print(long long v, int base) {
  if (base == 10 && v < 0) return write('-') + printNumber((unsigned long long)(-v), 10);
  return printNumber((unsigned long long)v, (uint8_t)base);
}
size_t Print::print(unsigned long long v, int base) { return printNumber(v, (uint8_t)base); }

size_t Print::print(double v, int digits) {
  if (isnan(v)) return write("nan");
  if (isinf(v)) return write("inf");
  if (v > 4294967040.0 || v < -4294967040.0) return write("ovf");
  char buf[64];
  snprintf(buf, sizeof buf, "%.*f", digits < 0 ? 0 : digits, v);
  return write(buf);
}

size_t Print::printf(const char* fmt, ...) {
  char small[256];
  va_list ap;
  va_start(ap, fmt);
  int n = vsnprintf(small, sizeof small, fmt, ap);
  va_end(ap);
  if (n < 0) return 0;
  if ((size_t)n < sizeof small) return write((const uint8_t*)small, (size_t)n);
  std::string big((size_t)n + 1, '\0');
  va_start(ap, fmt);
  vsnprintf(&big[0], big.size(), fmt, ap);
  va_end(ap);
  return write((const uint8_t*)big.data(), (size_t)n);
}

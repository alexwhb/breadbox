#include "IPAddress.h"

#include <stdio.h>
#include <stdlib.h>

#include "Print.h"

const IPAddress INADDR_NONE(0, 0, 0, 0);

bool IPAddress::fromString(const char* s) {
  unsigned v[4];
  char tail;
  if (!s || sscanf(s, "%u.%u.%u.%u%c", &v[0], &v[1], &v[2], &v[3], &tail) != 4) return false;
  for (int i = 0; i < 4; i++) {
    if (v[i] > 255) return false;
    b_[i] = (uint8_t)v[i];
  }
  return true;
}

String IPAddress::toString() const {
  char buf[16];
  snprintf(buf, sizeof buf, "%u.%u.%u.%u", b_[0], b_[1], b_[2], b_[3]);
  return String(buf);
}

size_t IPAddress::printTo(Print& p) const { return p.print(toString()); }

#include "Stream.h"

#include "Arduino.h"

int Stream::timedRead() {
  unsigned long t0 = millis();
  do {
    int c = read();
    if (c >= 0) return c;
    delay(1);
  } while (millis() - t0 < timeout_);
  return -1;
}

int Stream::timedPeek() {
  unsigned long t0 = millis();
  do {
    int c = peek();
    if (c >= 0) return c;
    delay(1);
  } while (millis() - t0 < timeout_);
  return -1;
}

int Stream::peekNextDigit(bool allow_dot) {
  for (;;) {
    int c = timedPeek();
    if (c < 0 || c == '-' || (c >= '0' && c <= '9') || (allow_dot && c == '.')) return c;
    read();
  }
}

bool Stream::find(const char* target) { return find(target, strlen(target)); }

bool Stream::find(const char* target, size_t len) {
  if (len == 0) return true;
  size_t idx = 0;
  int c;
  while ((c = timedRead()) >= 0) {
    if (c == target[idx]) {
      if (++idx == len) return true;
    } else {
      idx = (c == target[0]) ? 1 : 0;
    }
  }
  return false;
}

bool Stream::findUntil(const char* target, const char* term) {
  size_t tl = strlen(target), ul = term ? strlen(term) : 0, ti = 0, ui = 0;
  int c;
  while ((c = timedRead()) >= 0) {
    ti = (c == target[ti]) ? ti + 1 : (c == target[0] ? 1 : 0);
    if (ti == tl) return true;
    if (ul) {
      ui = (c == term[ui]) ? ui + 1 : (c == term[0] ? 1 : 0);
      if (ui == ul) return false;
    }
  }
  return false;
}

long Stream::parseInt() {
  int c = peekNextDigit(false);
  if (c < 0) return 0;
  bool neg = false;
  long v = 0;
  do {
    if (c == '-')
      neg = true;
    else if (c >= '0' && c <= '9')
      v = v * 10 + c - '0';
    read();
    c = timedPeek();
  } while (c >= '0' && c <= '9');
  return neg ? -v : v;
}

float Stream::parseFloat() {
  int c = peekNextDigit(true);
  if (c < 0) return 0;
  bool neg = false, frac = false;
  double v = 0, scale = 1;
  do {
    if (c == '-')
      neg = true;
    else if (c == '.')
      frac = true;
    else if (c >= '0' && c <= '9') {
      v = v * 10 + c - '0';
      if (frac) scale *= 10;
    }
    read();
    c = timedPeek();
  } while ((c >= '0' && c <= '9') || (c == '.' && !frac));
  v /= scale;
  return (float)(neg ? -v : v);
}

size_t Stream::readBytes(char* buf, size_t n) {
  size_t k = 0;
  while (k < n) {
    int c = timedRead();
    if (c < 0) break;
    buf[k++] = (char)c;
  }
  return k;
}

size_t Stream::readBytesUntil(char term, char* buf, size_t n) {
  size_t k = 0;
  while (k < n) {
    int c = timedRead();
    if (c < 0 || c == term) break;
    buf[k++] = (char)c;
  }
  return k;
}

String Stream::readString() {
  String s;
  int c;
  while ((c = timedRead()) >= 0) s += (char)c;
  return s;
}

String Stream::readStringUntil(char term) {
  String s;
  int c;
  while ((c = timedRead()) >= 0 && c != term) s += (char)c;
  return s;
}

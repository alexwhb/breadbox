// Stream.h — Arduino Stream (timed reads, parsing).
#pragma once

#include "Print.h"

class Stream : public Print {
 public:
  virtual int available() = 0;
  virtual int read() = 0;
  virtual int peek() = 0;

  void setTimeout(unsigned long ms) { timeout_ = ms; }
  unsigned long getTimeout() const { return timeout_; }

  bool find(const char* target);
  bool find(const char* target, size_t len);
  bool find(char t) { return find(&t, 1); }
  bool findUntil(const char* target, const char* terminator);
  long parseInt();
  float parseFloat();
  size_t readBytes(char* buf, size_t n);
  size_t readBytes(uint8_t* buf, size_t n) { return readBytes((char*)buf, n); }
  size_t readBytesUntil(char term, char* buf, size_t n);
  String readString();
  String readStringUntil(char term);

 protected:
  int timedRead();
  int timedPeek();
  int peekNextDigit(bool allow_dot);
  unsigned long timeout_ = 1000;
};

// WString.h — Arduino String (backed by std::string).
#pragma once

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <string>
#include <utility>

class __FlashStringHelper;

class String {
 public:
  String() {}
  String(const char* s) : s_(s ? s : "") {}
  String(const char* s, unsigned int len) : s_(s ? std::string(s, len) : std::string()) {}
  String(const std::string& s) : s_(s) {}
  String(const String& o) = default;
  String(String&& o) = default;
  String(const __FlashStringHelper* f) : s_(reinterpret_cast<const char*>(f)) {}
  explicit String(char c) : s_(1, c) {}
  explicit String(unsigned char v, unsigned char base = 10) { s_ = num((unsigned long)v, base); }
  explicit String(int v, unsigned char base = 10) { s_ = base == 10 ? std::to_string(v) : num((unsigned long)(unsigned)v, base); }
  explicit String(unsigned int v, unsigned char base = 10) { s_ = num((unsigned long)v, base); }
  explicit String(long v, unsigned char base = 10) { s_ = base == 10 ? std::to_string(v) : num((unsigned long)v, base); }
  explicit String(unsigned long v, unsigned char base = 10) { s_ = num(v, base); }
  explicit String(long long v, unsigned char base = 10) { s_ = base == 10 ? std::to_string(v) : num((unsigned long)v, base); }
  explicit String(unsigned long long v, unsigned char base = 10) { s_ = num((unsigned long)v, base); }
  explicit String(float v, unsigned int decimals = 2) { s_ = fmt((double)v, decimals); }
  explicit String(double v, unsigned int decimals = 2) { s_ = fmt(v, decimals); }

  String& operator=(const String& o) = default;
  String& operator=(String&& o) = default;
  String& operator=(const char* s) {
    s_ = s ? s : "";
    return *this;
  }

  unsigned int length() const { return (unsigned int)s_.size(); }
  bool isEmpty() const { return s_.empty(); }
  void clear() { s_.clear(); }
  const char* c_str() const { return s_.c_str(); }
  char* begin() { return &s_[0]; }
  char* end() { return &s_[0] + s_.size(); }
  const std::string& str() const { return s_; }
  operator std::string() const { return s_; }
  bool reserve(unsigned int n) {
    s_.reserve(n);
    return true;
  }

  bool concat(const String& o) {
    s_ += o.s_;
    return true;
  }
  bool concat(const char* c) {
    if (c) s_ += c;
    return true;
  }
  bool concat(const char* c, unsigned int n) {
    s_.append(c, n);
    return true;
  }
  bool concat(char c) {
    s_ += c;
    return true;
  }
  bool concat(unsigned char v) { return concat(String(v)); }
  bool concat(int v) { return concat(String(v)); }
  bool concat(unsigned int v) { return concat(String(v)); }
  bool concat(long v) { return concat(String(v)); }
  bool concat(unsigned long v) { return concat(String(v)); }
  bool concat(long long v) { return concat(String(v)); }
  bool concat(unsigned long long v) { return concat(String(v)); }
  bool concat(float v) { return concat(String(v)); }
  bool concat(double v) { return concat(String(v)); }
  bool concat(const __FlashStringHelper* f) { return concat(reinterpret_cast<const char*>(f)); }

  template <typename T>
  String& operator+=(const T& v) {
    concat(v);
    return *this;
  }

  int compareTo(const String& o) const { return s_.compare(o.s_); }
  bool equals(const String& o) const { return s_ == o.s_; }
  bool equals(const char* c) const { return s_ == (c ? c : ""); }
  bool equalsIgnoreCase(const String& o) const {
    if (s_.size() != o.s_.size()) return false;
    for (size_t i = 0; i < s_.size(); i++)
      if (tolower((unsigned char)s_[i]) != tolower((unsigned char)o.s_[i])) return false;
    return true;
  }
  bool operator==(const String& o) const { return s_ == o.s_; }
  bool operator==(const char* c) const { return equals(c); }
  bool operator!=(const String& o) const { return s_ != o.s_; }
  bool operator!=(const char* c) const { return !equals(c); }
  bool operator<(const String& o) const { return s_ < o.s_; }
  bool operator>(const String& o) const { return s_ > o.s_; }
  bool operator<=(const String& o) const { return s_ <= o.s_; }
  bool operator>=(const String& o) const { return s_ >= o.s_; }
  bool startsWith(const String& p) const { return s_.compare(0, p.s_.size(), p.s_) == 0; }
  bool startsWith(const String& p, unsigned int off) const { return off <= s_.size() && s_.compare(off, p.s_.size(), p.s_) == 0; }
  bool endsWith(const String& p) const {
    return s_.size() >= p.s_.size() && s_.compare(s_.size() - p.s_.size(), p.s_.size(), p.s_) == 0;
  }

  char charAt(unsigned int i) const { return i < s_.size() ? s_[i] : 0; }
  void setCharAt(unsigned int i, char c) {
    if (i < s_.size()) s_[i] = c;
  }
  char operator[](unsigned int i) const { return charAt(i); }
  char& operator[](unsigned int i) {
    static char dummy;
    return i < s_.size() ? s_[i] : (dummy = 0);
  }
  void getBytes(unsigned char* buf, unsigned int n, unsigned int idx = 0) const {
    if (!n || !buf) return;
    size_t k = idx < s_.size() ? std::min<size_t>(n - 1, s_.size() - idx) : 0;
    if (k) memcpy(buf, s_.data() + idx, k);
    buf[k] = 0;
  }
  void toCharArray(char* buf, unsigned int n, unsigned int idx = 0) const { getBytes((unsigned char*)buf, n, idx); }

  int indexOf(char c) const { return pos(s_.find(c)); }
  int indexOf(char c, unsigned int from) const { return pos(s_.find(c, from)); }
  int indexOf(const String& s) const { return pos(s_.find(s.s_)); }
  int indexOf(const String& s, unsigned int from) const { return pos(s_.find(s.s_, from)); }
  int lastIndexOf(char c) const { return pos(s_.rfind(c)); }
  int lastIndexOf(char c, unsigned int from) const { return pos(s_.rfind(c, from)); }
  int lastIndexOf(const String& s) const { return pos(s_.rfind(s.s_)); }
  String substring(unsigned int from) const { return from < s_.size() ? String(s_.substr(from)) : String(); }
  String substring(unsigned int from, unsigned int to) const {
    if (from > to) std::swap(from, to);
    if (from >= s_.size()) return String();
    return String(s_.substr(from, std::min<size_t>(to, s_.size()) - from));
  }

  void replace(char a, char b) {
    for (auto& c : s_)
      if (c == a) c = b;
  }
  void replace(const String& a, const String& b) {
    if (a.s_.empty()) return;
    size_t p = 0;
    while ((p = s_.find(a.s_, p)) != std::string::npos) {
      s_.replace(p, a.s_.size(), b.s_);
      p += b.s_.size();
    }
  }
  void remove(unsigned int i) {
    if (i < s_.size()) s_.erase(i);
  }
  void remove(unsigned int i, unsigned int n) {
    if (i < s_.size()) s_.erase(i, n);
  }
  void toLowerCase() {
    for (auto& c : s_) c = (char)tolower((unsigned char)c);
  }
  void toUpperCase() {
    for (auto& c : s_) c = (char)toupper((unsigned char)c);
  }
  void trim() {
    size_t a = s_.find_first_not_of(" \t\r\n\f\v");
    if (a == std::string::npos) {
      s_.clear();
      return;
    }
    size_t b = s_.find_last_not_of(" \t\r\n\f\v");
    s_ = s_.substr(a, b - a + 1);
  }
  long toInt() const { return atol(s_.c_str()); }
  float toFloat() const { return (float)atof(s_.c_str()); }
  double toDouble() const { return atof(s_.c_str()); }

  explicit operator bool() const { return true; }

 private:
  static int pos(size_t p) { return p == std::string::npos ? -1 : (int)p; }
  static std::string num(unsigned long v, unsigned char base) {
    if (base < 2) base = 10;
    char buf[70];
    int i = 69;
    buf[i] = 0;
    do {
      int d = (int)(v % base);
      buf[--i] = (char)(d < 10 ? '0' + d : 'a' + d - 10);
      v /= base;
    } while (v && i > 0);
    return std::string(buf + i);
  }
  static std::string fmt(double v, unsigned int dec) {
    char buf[64];
    snprintf(buf, sizeof buf, "%.*f", (int)dec, v);
    return buf;
  }
  std::string s_;
};

inline String operator+(const String& a, const String& b) {
  String r(a);
  r.concat(b);
  return r;
}
inline String operator+(const String& a, const char* b) {
  String r(a);
  r.concat(b);
  return r;
}
inline String operator+(const char* a, const String& b) {
  String r(a);
  r.concat(b);
  return r;
}
inline String operator+(const String& a, char b) {
  String r(a);
  r.concat(b);
  return r;
}
template <typename T, typename = decltype(String(std::declval<T>()))>
inline String operator+(const String& a, T b) {
  String r(a);
  r.concat(String(b));
  return r;
}
inline bool operator==(const char* a, const String& b) { return b.equals(a); }

typedef String StringSumHelper;

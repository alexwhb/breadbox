#pragma once
#include <stddef.h>
class Print;
// Objects that know how to print themselves (IPAddress, …).
class Printable {
 public:
  virtual ~Printable() {}
  virtual size_t printTo(Print& p) const = 0;
};

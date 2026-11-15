// Preferences.h — ESP32 NVS key/value storage, persisted by the simulator across reboots and
// re-flashes (a file per board).
#pragma once

#include "Arduino.h"

class Preferences {
 public:
  bool begin(const char* name, bool readOnly = false, const char* partition = nullptr);
  void end() { ns_.clear(); }
  bool clear();
  bool remove(const char* key);
  bool isKey(const char* key);
  size_t putChar(const char* key, int8_t v) { return putInt(key, v) ? 1 : 0; }
  size_t putUChar(const char* key, uint8_t v) { return putUInt(key, v) ? 1 : 0; }
  size_t putShort(const char* key, int16_t v) { return putInt(key, v) ? 2 : 0; }
  size_t putUShort(const char* key, uint16_t v) { return putUInt(key, v) ? 2 : 0; }
  size_t putInt(const char* key, int32_t v) { return put(key, String((long)v)) ? 4 : 0; }
  size_t putUInt(const char* key, uint32_t v) { return put(key, String((unsigned long)v)) ? 4 : 0; }
  size_t putLong(const char* key, int32_t v) { return putInt(key, v); }
  size_t putULong(const char* key, uint32_t v) { return putUInt(key, v); }
  size_t putLong64(const char* key, int64_t v) { return put(key, String((long long)v)) ? 8 : 0; }
  size_t putULong64(const char* key, uint64_t v) { return put(key, String((unsigned long long)v)) ? 8 : 0; }
  size_t putFloat(const char* key, float v) { return put(key, String((double)v, 9)) ? 4 : 0; }
  size_t putDouble(const char* key, double v) { return put(key, String(v, 17)) ? 8 : 0; }
  size_t putBool(const char* key, bool v) { return putUInt(key, v ? 1 : 0) ? 1 : 0; }
  size_t putString(const char* key, const char* v) { return put(key, String(v)) ? strlen(v) : 0; }
  size_t putString(const char* key, const String& v) { return put(key, v) ? v.length() : 0; }
  size_t putBytes(const char* key, const void* v, size_t len);
  int8_t getChar(const char* key, int8_t d = 0) { return (int8_t)getInt(key, d); }
  uint8_t getUChar(const char* key, uint8_t d = 0) { return (uint8_t)getUInt(key, d); }
  int16_t getShort(const char* key, int16_t d = 0) { return (int16_t)getInt(key, d); }
  uint16_t getUShort(const char* key, uint16_t d = 0) { return (uint16_t)getUInt(key, d); }
  int32_t getInt(const char* key, int32_t d = 0);
  uint32_t getUInt(const char* key, uint32_t d = 0);
  int32_t getLong(const char* key, int32_t d = 0) { return getInt(key, d); }
  uint32_t getULong(const char* key, uint32_t d = 0) { return getUInt(key, d); }
  int64_t getLong64(const char* key, int64_t d = 0);
  uint64_t getULong64(const char* key, uint64_t d = 0);
  float getFloat(const char* key, float d = NAN);
  double getDouble(const char* key, double d = NAN);
  bool getBool(const char* key, bool d = false) { return getUInt(key, d ? 1 : 0) != 0; }
  String getString(const char* key, const String d = String());
  size_t getString(const char* key, char* value, size_t maxLen);
  size_t getBytesLength(const char* key);
  size_t getBytes(const char* key, void* buf, size_t maxLen);
  size_t freeEntries() { return 500; }

 private:
  bool put(const char* key, const String& value);
  bool get(const char* key, String& out);
  String ns_;
  bool ro_ = false;
};

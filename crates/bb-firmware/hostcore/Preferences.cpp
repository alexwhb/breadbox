#include "Preferences.h"

#include <stdio.h>

#include <map>
#include <string>

#include "bb_host.h"

// File format: one entry per line, "namespace<TAB>key<TAB>hex(value)".
static std::map<std::string, std::string> store;
static bool loaded = false;

static std::string hex(const std::string& s) {
  static const char* d = "0123456789abcdef";
  std::string o;
  for (unsigned char c : s) {
    o += d[c >> 4];
    o += d[c & 15];
  }
  return o;
}

static std::string unhex(const std::string& h) {
  std::string o;
  for (size_t i = 0; i + 1 < h.size(); i += 2) o += (char)strtol(h.substr(i, 2).c_str(), nullptr, 16);
  return o;
}

static void load() {
  if (loaded) return;
  loaded = true;
  FILE* f = fopen(bb::nvs_path(), "r");
  if (!f) return;
  char line[8192];
  while (fgets(line, sizeof line, f)) {
    std::string l(line);
    while (!l.empty() && (l.back() == '\n' || l.back() == '\r')) l.pop_back();
    size_t a = l.find('\t'), b = l.find('\t', a + 1);
    if (a == std::string::npos || b == std::string::npos) continue;
    store[l.substr(0, b)] = unhex(l.substr(b + 1));
  }
  fclose(f);
}

static void save() {
  if (!*bb::nvs_path()) return;
  FILE* f = fopen(bb::nvs_path(), "w");
  if (!f) return;
  for (auto& kv : store) fprintf(f, "%s\t%s\n", kv.first.c_str(), hex(kv.second).c_str());
  fclose(f);
}

static std::string k(const String& ns, const char* key) { return std::string(ns.c_str()) + "\t" + key; }

bool Preferences::begin(const char* name, bool readOnly, const char*) {
  load();
  ns_ = name;
  ro_ = readOnly;
  return ns_.length() > 0 && ns_.length() <= 15;
}

bool Preferences::clear() {
  if (ro_ || !ns_.length()) return false;
  std::string p = std::string(ns_.c_str()) + "\t";
  for (auto it = store.begin(); it != store.end();)
    it = it->first.compare(0, p.size(), p) == 0 ? store.erase(it) : std::next(it);
  save();
  return true;
}

bool Preferences::remove(const char* key) {
  if (ro_ || !ns_.length()) return false;
  bool had = store.erase(k(ns_, key)) > 0;
  save();
  return had;
}

bool Preferences::isKey(const char* key) { return ns_.length() && store.count(k(ns_, key)); }

bool Preferences::put(const char* key, const String& value) {
  if (ro_ || !ns_.length() || !key || strlen(key) > 15) return false;
  store[k(ns_, key)] = value.c_str();
  save();
  return true;
}

bool Preferences::get(const char* key, String& out) {
  if (!ns_.length()) return false;
  auto it = store.find(k(ns_, key));
  if (it == store.end()) return false;
  out = String(it->second);
  return true;
}

int32_t Preferences::getInt(const char* key, int32_t d) {
  String v;
  return get(key, v) ? (int32_t)strtol(v.c_str(), nullptr, 10) : d;
}
uint32_t Preferences::getUInt(const char* key, uint32_t d) {
  String v;
  return get(key, v) ? (uint32_t)strtoul(v.c_str(), nullptr, 10) : d;
}
int64_t Preferences::getLong64(const char* key, int64_t d) {
  String v;
  return get(key, v) ? strtoll(v.c_str(), nullptr, 10) : d;
}
uint64_t Preferences::getULong64(const char* key, uint64_t d) {
  String v;
  return get(key, v) ? strtoull(v.c_str(), nullptr, 10) : d;
}
float Preferences::getFloat(const char* key, float d) {
  String v;
  return get(key, v) ? (float)atof(v.c_str()) : d;
}
double Preferences::getDouble(const char* key, double d) {
  String v;
  return get(key, v) ? atof(v.c_str()) : d;
}
String Preferences::getString(const char* key, const String d) {
  String v;
  return get(key, v) ? v : d;
}
size_t Preferences::getString(const char* key, char* value, size_t maxLen) {
  String v;
  if (!get(key, v) || !value || maxLen == 0) return 0;
  v.toCharArray(value, (unsigned)maxLen);
  return v.length() + 1;
}

size_t Preferences::putBytes(const char* key, const void* v, size_t len) {
  if (ro_ || !ns_.length()) return 0;
  store[k(ns_, key)] = std::string((const char*)v, len);
  save();
  return len;
}

size_t Preferences::getBytesLength(const char* key) {
  auto it = store.find(k(ns_, key));
  return it == store.end() ? 0 : it->second.size();
}

size_t Preferences::getBytes(const char* key, void* buf, size_t maxLen) {
  auto it = store.find(k(ns_, key));
  if (it == store.end() || it->second.size() > maxLen) return 0;
  memcpy(buf, it->second.data(), it->second.size());
  return it->second.size();
}

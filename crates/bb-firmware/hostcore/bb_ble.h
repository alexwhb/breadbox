// bb_ble.h — ESP32 BLE Arduino subset (BLEDevice / BLEServer / BLEService / BLECharacteristic /
// BLEAdvertising / BLE2902). The GATT server lives in the simulator; the virtual phone sees it.
#pragma once

#include <map>
#include <string>
#include <vector>

#include "Arduino.h"

class BLEUUID {
 public:
  BLEUUID() {}
  BLEUUID(const char* s) : s_(s ? s : "") {}
  BLEUUID(const std::string& s) : s_(s) {}
  BLEUUID(uint16_t v) {
    char b[8];
    snprintf(b, sizeof b, "%04x", v);
    s_ = b;
  }
  std::string toString() const { return s_; }
  bool equals(const BLEUUID& o) const;

 private:
  std::string s_;
};

class BLEAddress {
 public:
  explicit BLEAddress(const std::string& s = "a4:c1:38:12:34:b0") : s_(s) {}
  std::string toString() const { return s_; }

 private:
  std::string s_;
};

class BLEDescriptor {
 public:
  explicit BLEDescriptor(const char* uuid, uint16_t len = 100) : uuid_(uuid) { (void)len; }
  explicit BLEDescriptor(BLEUUID uuid, uint16_t len = 100) : uuid_(uuid) { (void)len; }
  virtual ~BLEDescriptor() {}
  void setValue(const uint8_t* data, size_t len) { value_.assign((const char*)data, len); }
  void setValue(const std::string& v) { value_ = v; }
  BLEUUID getUUID() { return uuid_; }

 private:
  BLEUUID uuid_;
  std::string value_;
};

class BLE2902 : public BLEDescriptor {
 public:
  BLE2902() : BLEDescriptor("2902", 2) {}
  void setNotifications(bool) {}
  void setIndications(bool) {}
};

class BLECharacteristic;
class BLEServer;

class BLECharacteristicCallbacks {
 public:
  virtual ~BLECharacteristicCallbacks() {}
  virtual void onRead(BLECharacteristic*) {}
  virtual void onWrite(BLECharacteristic*) {}
  virtual void onNotify(BLECharacteristic*) {}
};

class BLECharacteristic {
 public:
  static const uint32_t PROPERTY_READ = 1 << 0;
  static const uint32_t PROPERTY_WRITE = 1 << 1;
  static const uint32_t PROPERTY_NOTIFY = 1 << 2;
  static const uint32_t PROPERTY_BROADCAST = 1 << 3;
  static const uint32_t PROPERTY_INDICATE = 1 << 4;
  static const uint32_t PROPERTY_WRITE_NR = 1 << 5;

  BLECharacteristic(BLEUUID uuid, uint32_t props) : uuid_(uuid), props_(props) {}
  void setValue(const uint8_t* data, size_t len);
  void setValue(const std::string& v) { setValue((const uint8_t*)v.data(), v.size()); }
  void setValue(const String& v) { setValue((const uint8_t*)v.c_str(), v.length()); }
  void setValue(const char* v) { setValue((const uint8_t*)v, strlen(v)); }
  void setValue(uint16_t& v) { setValue((const uint8_t*)&v, 2); }
  void setValue(uint32_t& v) { setValue((const uint8_t*)&v, 4); }
  void setValue(int& v) { setValue((const uint8_t*)&v, 4); }
  void setValue(float& v) { setValue((const uint8_t*)&v, 4); }
  void setValue(double& v) { setValue((const uint8_t*)&v, 8); }
  std::string getValue() { return value_; }
  uint8_t* getData() { return (uint8_t*)value_.data(); }
  size_t getLength() { return value_.size(); }
  void notify(bool is_notification = true);
  void indicate() { notify(false); }
  void setCallbacks(BLECharacteristicCallbacks* cb) { callbacks_ = cb; }
  void addDescriptor(BLEDescriptor* d) { descriptors_.push_back(d); }
  BLEUUID getUUID() { return uuid_; }
  uint16_t getHandle() { return handle_; }
  uint32_t getProperties() { return props_; }

  // internal
  uint16_t handle_ = 0;
  std::string value_;
  BLECharacteristicCallbacks* callbacks_ = nullptr;

 private:
  BLEUUID uuid_;
  uint32_t props_;
  std::vector<BLEDescriptor*> descriptors_;
};

class BLEService {
 public:
  BLEService(BLEUUID uuid, int index) : uuid_(uuid), index_(index) {}
  BLECharacteristic* createCharacteristic(const char* uuid, uint32_t props) { return createCharacteristic(BLEUUID(uuid), props); }
  BLECharacteristic* createCharacteristic(BLEUUID uuid, uint32_t props);
  BLECharacteristic* getCharacteristic(BLEUUID uuid);
  void start() {}
  void stop() {}
  BLEUUID getUUID() { return uuid_; }

 private:
  BLEUUID uuid_;
  int index_;
  std::vector<BLECharacteristic*> chars_;
};

class BLEServerCallbacks {
 public:
  virtual ~BLEServerCallbacks() {}
  virtual void onConnect(BLEServer*) {}
  virtual void onDisconnect(BLEServer*) {}
};

class BLEAdvertising {
 public:
  void addServiceUUID(BLEUUID) {}
  void addServiceUUID(const char*) {}
  void setScanResponse(bool) {}
  void setMinPreferred(uint16_t) {}
  void setMaxPreferred(uint16_t) {}
  void setAppearance(uint16_t) {}
  void setMinInterval(uint16_t) {}
  void setMaxInterval(uint16_t) {}
  void start();
  void stop();
};

class BLEServer {
 public:
  BLEService* createService(const char* uuid) { return createService(BLEUUID(uuid)); }
  BLEService* createService(BLEUUID uuid, uint32_t numHandles = 15, uint8_t inst_id = 0);
  void setCallbacks(BLEServerCallbacks* cb) { callbacks_ = cb; }
  BLEAdvertising* getAdvertising();
  void startAdvertising();
  uint32_t getConnectedCount();
  void disconnect(uint16_t connId = 0);
  BLEServerCallbacks* callbacks_ = nullptr;
};

class BLEDevice {
 public:
  static void init(const std::string& deviceName);
  static void init(const String& deviceName) { init(std::string(deviceName.c_str())); }
  static void init(const char* deviceName) { init(std::string(deviceName ? deviceName : "")); }
  static BLEServer* createServer();
  static BLEAdvertising* getAdvertising();
  static void startAdvertising();
  static void stopAdvertising();
  static void deinit(bool release_memory = false);
  static void setPower(int) {}
  static void setMTU(uint16_t) {}
  static BLEAddress getAddress() { return BLEAddress(); }
  static std::string toString() { return "BLEDevice"; }
  static bool getInitialized();
};

#define ESP_PWR_LVL_P9 9

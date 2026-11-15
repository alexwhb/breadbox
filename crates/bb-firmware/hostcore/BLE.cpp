#include <algorithm>

#include "bb_ble.h"
#include "bb_host.h"

namespace {
enum : uint8_t { INIT = 1, ADD_SERVICE, ADD_CHAR, SET_VALUE, GET_VALUE, NOTIFY, ADVERTISE, POLL, CONNECTED, DISCONNECT };
bb::Reply ble(bb::Buf b) { return bb::service(bb::CH_BLE, b); }
bb::Buf op(uint8_t o) { return bb::Buf().u8(o); }

bool initialized = false;
BLEServer* server = nullptr;
BLEAdvertising advertising;
std::map<uint16_t, BLECharacteristic*> by_handle;

void poll() {
  if (!initialized) return;
  bb::Reply r = ble(op(POLL));
  uint8_t n = r.u8();
  for (uint8_t i = 0; i < n; i++) {
    uint8_t kind = r.u8();
    if (kind == 1) {
      if (server && server->callbacks_) server->callbacks_->onConnect(server);
    } else if (kind == 2) {
      if (server && server->callbacks_) server->callbacks_->onDisconnect(server);
    } else if (kind == 3) {
      uint16_t h = r.u16();
      std::vector<uint8_t> d = r.bytes();
      auto it = by_handle.find(h);
      if (it != by_handle.end()) {
        it->second->value_.assign(d.begin(), d.end());
        if (it->second->callbacks_) it->second->callbacks_->onWrite(it->second);
      }
    } else if (kind == 4) {
      uint16_t h = r.u16();
      auto it = by_handle.find(h);
      if (it != by_handle.end() && it->second->callbacks_) it->second->callbacks_->onRead(it->second);
    } else if (kind == 5) {
      r.u16();
      r.u8();
    }
  }
}

std::string norm(std::string s) {
  s.erase(std::remove(s.begin(), s.end(), '-'), s.end());
  for (auto& c : s) c = (char)tolower((unsigned char)c);
  if (s.rfind("0x", 0) == 0) s = s.substr(2);
  return s;
}
}  // namespace

bool BLEUUID::equals(const BLEUUID& o) const { return norm(s_) == norm(o.s_); }

void BLEDevice::init(const std::string& name) {
  ble(op(INIT).str(name));
  if (!initialized) {
    initialized = true;
    bb::add_async_hook(poll);
    bb::set_async(true);
  }
}

bool BLEDevice::getInitialized() { return initialized; }

BLEServer* BLEDevice::createServer() {
  if (!server) server = new BLEServer();
  return server;
}

BLEAdvertising* BLEDevice::getAdvertising() { return &advertising; }
void BLEDevice::startAdvertising() { advertising.start(); }
void BLEDevice::stopAdvertising() { advertising.stop(); }
void BLEDevice::deinit(bool) {
  advertising.stop();
  initialized = false;
}

void BLEAdvertising::start() { ble(op(ADVERTISE).u8(1)); }
void BLEAdvertising::stop() { ble(op(ADVERTISE).u8(0)); }

BLEService* BLEServer::createService(BLEUUID uuid, uint32_t, uint8_t) {
  bb::Reply r = ble(op(ADD_SERVICE).str(uuid.toString()));
  return new BLEService(uuid, r.i32());
}

BLEAdvertising* BLEServer::getAdvertising() { return &advertising; }
void BLEServer::startAdvertising() { advertising.start(); }
uint32_t BLEServer::getConnectedCount() { return ble(op(CONNECTED)).u8(); }
void BLEServer::disconnect(uint16_t) { ble(op(DISCONNECT)); }

BLECharacteristic* BLEService::createCharacteristic(BLEUUID uuid, uint32_t props) {
  uint8_t gatt = 0;
  if (props & BLECharacteristic::PROPERTY_READ) gatt |= 0x02;
  if (props & BLECharacteristic::PROPERTY_WRITE_NR) gatt |= 0x04;
  if (props & BLECharacteristic::PROPERTY_WRITE) gatt |= 0x08;
  if (props & BLECharacteristic::PROPERTY_NOTIFY) gatt |= 0x10;
  if (props & BLECharacteristic::PROPERTY_INDICATE) gatt |= 0x20;
  bb::Reply r = ble(op(ADD_CHAR).i32(index_).str(uuid.toString()).u8(gatt));
  BLECharacteristic* c = new BLECharacteristic(uuid, props);
  c->handle_ = (uint16_t)r.i32();
  by_handle[c->handle_] = c;
  chars_.push_back(c);
  return c;
}

BLECharacteristic* BLEService::getCharacteristic(BLEUUID uuid) {
  for (auto c : chars_)
    if (c->getUUID().equals(uuid)) return c;
  return nullptr;
}

void BLECharacteristic::setValue(const uint8_t* data, size_t len) {
  value_.assign((const char*)data, len);
  ble(op(SET_VALUE).u16(handle_).bytes(data, len));
}

void BLECharacteristic::notify(bool) {
  ble(op(NOTIFY).u16(handle_).bytes((const uint8_t*)value_.data(), value_.size()));
  if (callbacks_) callbacks_->onNotify(this);
}

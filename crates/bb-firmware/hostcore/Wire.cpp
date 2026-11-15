#include "Wire.h"

#include "bb_host.h"

TwoWire Wire(0);
TwoWire Wire1(1);

uint8_t TwoWire::endTransmission(bool sendStop) {
  bb::Reply r = bb::rpc(bb::MSG_I2C_WRITE, bb::Buf().u8(bus_).u8(addr_).u32(freq_).u8(sendStop ? 1 : 0).bytes(tx_.data(), tx_.size()));
  tx_.clear();
  return r.u8();  // 0 ok, 2 address NACK, 3 data NACK, 4 other
}

size_t TwoWire::requestFrom(uint16_t address, size_t size, bool sendStop) {
  bb::Reply r = bb::rpc(bb::MSG_I2C_READ, bb::Buf().u8(bus_).u8((uint8_t)address).u32(freq_).u8(sendStop ? 1 : 0).u16((uint16_t)size));
  rx_ = r.bytes();
  rx_pos_ = 0;
  return rx_.size();
}

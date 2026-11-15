#include "SPI.h"

#include "bb_host.h"

SPIClass SPI(0);

void SPIClass::transferBytes(const uint8_t* out, uint8_t* in, uint32_t size) {
  std::vector<uint8_t> tx(size, 0xff);
  if (out) memcpy(tx.data(), out, size);
  bb::Reply r = bb::rpc(bb::MSG_SPI_TRANSFER, bb::Buf().u8(bus_).u32(settings_.clock_).u8(settings_.mode_).bytes(tx.data(), size));
  std::vector<uint8_t> rx = r.bytes();
  if (in)
    for (uint32_t i = 0; i < size; i++) in[i] = i < rx.size() ? rx[i] : 0xff;
}

uint8_t SPIClass::transfer(uint8_t data) {
  uint8_t in;
  transferBytes(&data, &in, 1);
  return in;
}

uint16_t SPIClass::transfer16(uint16_t data) {
  uint8_t out[2] = {(uint8_t)(data >> 8), (uint8_t)(data & 0xff)};
  uint8_t in[2];
  transferBytes(out, in, 2);
  return (uint16_t)(in[0] << 8 | in[1]);
}

void SPIClass::transfer(void* buf, size_t count) { transferBytes((uint8_t*)buf, (uint8_t*)buf, (uint32_t)count); }

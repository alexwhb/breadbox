// ESP32 Wire → BME280 on the bench (API-level I²C bridged to the bench device): chip id,
// calibration words and the datasheet's integer temperature compensation.
#include <Wire.h>

const uint8_t BME = 0x76;
uint16_t digT1;
int16_t digT2, digT3;

uint8_t rd(uint8_t reg, uint8_t* buf, uint8_t n) {
  Wire.beginTransmission(BME);
  Wire.write(reg);
  if (Wire.endTransmission(false) != 0) return 0;
  uint8_t got = Wire.requestFrom(BME, n);
  for (uint8_t i = 0; i < got; i++) buf[i] = Wire.read();
  return got;
}

void setup() {
  Serial.begin(115200);
  Wire.begin(21, 22);
  uint8_t id = 0;
  rd(0xD0, &id, 1);
  Serial.printf("chip_id=0x%02X\n", id);
  uint8_t c[6];
  rd(0x88, c, 6);
  digT1 = c[0] | (c[1] << 8);
  digT2 = (int16_t)(c[2] | (c[3] << 8));
  digT3 = (int16_t)(c[4] | (c[5] << 8));
  Wire.beginTransmission(BME);
  Wire.write(0xF4);
  Wire.write(0x27);  // osrs_t ×1, osrs_p ×1, normal mode
  Serial.printf("ctrl=%d\n", Wire.endTransmission());
  Wire.beginTransmission(0x40);  // nobody there
  Serial.printf("ghost=%d\n", Wire.endTransmission());
}

void loop() {
  uint8_t d[3];
  if (rd(0xFA, d, 3) == 3) {
    int32_t adc = ((int32_t)d[0] << 12) | ((int32_t)d[1] << 4) | (d[2] >> 4);
    int32_t v1 = ((((adc >> 3) - ((int32_t)digT1 << 1))) * digT2) >> 11;
    int32_t v2 = (((((adc >> 4) - (int32_t)digT1) * ((adc >> 4) - (int32_t)digT1)) >> 12) * digT3) >> 14;
    int32_t t = ((v1 + v2) * 5 + 128) >> 8;
    Serial.printf("temp=%ld.%02ld\n", (long)(t / 100), (long)(t % 100));
  }
  delay(500);
}

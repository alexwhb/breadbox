// I²C traffic for the logic analyzer test: every 50 ms read the BME280's chip id (write the
// register pointer 0xD0, repeated START, read one byte), write ctrl_meas 0xF4 = 0x27, and
// probe an address nobody answers (0x40 → address NACK).
#include <Wire.h>

void setup() {
  Serial.begin(115200);
  Wire.begin();
}

void loop() {
  Wire.beginTransmission(0x76);
  Wire.write(0xD0);
  Wire.endTransmission(false);
  Wire.requestFrom(0x76, 1);
  int id = Wire.available() ? Wire.read() : -1;
  Wire.beginTransmission(0x76);
  Wire.write(0xF4);
  Wire.write(0x27);
  Wire.endTransmission();
  Wire.beginTransmission(0x40);
  byte ghost = Wire.endTransmission();
  Serial.print("id=0x");
  Serial.print(id, HEX);
  Serial.print(" ghost=");
  Serial.println(ghost);
  delay(50);
}

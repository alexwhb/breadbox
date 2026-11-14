// I2C master: writes 3 bytes to 0x42, reads 3 back, then probes an absent address 0x50.
#include <Wire.h>

void setup() {
  Serial.begin(115200);
  Wire.begin();
  Wire.beginTransmission(0x42);
  Wire.write(0x10);
  Wire.write(0x20);
  Wire.write(0x30);
  byte err = Wire.endTransmission();
  Serial.print("tx=");
  Serial.println(err);
  byte n = Wire.requestFrom(0x42, 3);
  Serial.print("rx");
  Serial.print(n);
  while (Wire.available()) {
    Serial.print(' ');
    Serial.print(Wire.read(), HEX);
  }
  Serial.println();
  Wire.beginTransmission(0x50);
  Wire.write(1);
  err = Wire.endTransmission();
  Serial.print("absent=");
  Serial.println(err);
}

void loop() {}

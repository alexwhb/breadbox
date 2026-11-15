// Host-core feature test: interrupts, busy waits, pulseIn, I2C, SPI, String, Preferences and
// ESP.restart(). Driven by crates/bb-firmware/tests/host.rs.
#include <Preferences.h>
#include <SPI.h>
#include <Wire.h>

volatile int presses = 0;
void IRAM_ATTR onPress() { presses++; }

Preferences prefs;
int boots = 0;

void setup() {
  Serial.begin(115200);
  prefs.begin("bench");
  boots = prefs.getInt("boots", 0) + 1;
  prefs.putInt("boots", boots);
  Serial.printf("boot %d\n", boots);
  pinMode(15, INPUT_PULLUP);
  attachInterrupt(digitalPinToInterrupt(15), onPress, FALLING);
  Wire.begin();
  Wire.beginTransmission(0x48);
  Wire.write(0x00);
  uint8_t st = Wire.endTransmission();
  Wire.requestFrom(0x48, 2);
  int hi = Wire.read(), lo = Wire.read();
  Serial.printf("i2c st=%d data=%02x%02x\n", st, hi, lo);
  Wire.beginTransmission(0x50);
  Serial.printf("i2c nack=%d\n", Wire.endTransmission());
  SPI.begin();
  Serial.printf("spi %02x\n", SPI.transfer(0xA5));
  unsigned long t0 = millis();
  while (millis() - t0 < 500) {
  }
  Serial.printf("busy waited %lu ms\n", millis() - t0);
  unsigned long w = pulseIn(16, HIGH, 300000);
  Serial.printf("pulse %lu us\n", w);
  String s = String("abc") + 42 + 'x';
  s.toUpperCase();
  Serial.println(s);
  Serial.println(String(3.14159, 3));
  Serial.println(map(512, 0, 1023, 0, 255));
}

void loop() {
  static int last = 0;
  if (presses != last) {
    last = presses;
    Serial.printf("presses=%d\n", presses);
  }
  delay(100);
  if (boots < 2 && millis() > 3000) {
    Serial.println("restarting");
    ESP.restart();
  }
}

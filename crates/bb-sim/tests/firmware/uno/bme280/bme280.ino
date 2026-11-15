// Adafruit BME280 over I2C (0x76).
#include <Wire.h>
#include <Adafruit_BME280.h>
Adafruit_BME280 bme;

void setup() {
  Serial.begin(115200);
  if (!bme.begin(0x76)) {
    Serial.println("bme: not found");
    while (1) delay(10);
  }
  Serial.println("bme: ok");
}

void loop() {
  Serial.print("t=");
  Serial.print(bme.readTemperature(), 2);
  Serial.print(" p=");
  Serial.print(bme.readPressure() / 100.0, 2);
  Serial.print(" h=");
  Serial.println(bme.readHumidity(), 2);
  delay(500);
}

// DallasTemperature over OneWire on D4.
#include <OneWire.h>
#include <DallasTemperature.h>
OneWire ow(4);
DallasTemperature sensors(&ow);

void setup() {
  Serial.begin(115200);
  sensors.begin();
  Serial.print("devices=");
  Serial.println(sensors.getDeviceCount());
}

void loop() {
  sensors.requestTemperatures();
  float t = sensors.getTempCByIndex(0);
  Serial.print("t=");
  Serial.println(t, 2);
  delay(500);
}

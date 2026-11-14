#include <SoftwareSerial.h>
#include "wifi_at.h"

const int SOIL_PIN  = A0;   // capacitive probe J1
const int LDR_PIN   = A1;   // 10k divider + LDR1
const int NIGHT_LED = 9;    // PWM, D1 amber
const int WATER_LED = 8;    // D2 red
const int SOIL_DRY = 620, SOIL_WET = 284;

SoftwareSerial esp(3, 2);  // RX, TX → U2
WifiAT wifi(esp);

void setup() {
  Serial.begin(115200);
  esp.begin(9600);
  pinMode(NIGHT_LED, OUTPUT);
  pinMode(WATER_LED, OUTPUT);
  wifi.join("SIM-AP", "breadbox");
  wifi.mqtt("10.0.0.1", 1883);
}

void loop() {
  int soil  = analogRead(SOIL_PIN);
  int light = analogRead(LDR_PIN);
  int pct   = map(soil, SOIL_DRY, SOIL_WET, 0, 60);
  int pwm   = constrain(255 - light / 4, 0, 255);

  analogWrite(NIGHT_LED, pwm);
  digitalWrite(WATER_LED, pct < 30 ? HIGH : LOW);
  Serial.printf("soil_raw=%d moisture=%d%% light_raw=%d led_pwm=%d\n", soil, pct, light, pwm);
  if (every(1000)) wifi.publish("garden/pot1", pct, light);
  delay(500);
}

// night-light — an LDR divider on A0 measures the room; when it gets dark a 2N2222 on D9
// drives the LED, brighter the darker it is.
//
//   5V — LDR — A0 — 10 kΩ — GND        D9 — 1 kΩ — 2N2222 base; LED + 220 Ω in the collector

const int LDR_PIN = A0;
const int LED_PIN = 9;     // PWM
const int DARK = 300;      // below this the light starts to come on
const int NIGHT = 100;     // full brightness at and below this

void setup() {
  Serial.begin(115200);
  pinMode(LED_PIN, OUTPUT);
}

void loop() {
  int light = analogRead(LDR_PIN);
  int level = 0;
  if (light < DARK) {
    level = map(constrain(light, NIGHT, DARK), DARK, NIGHT, 40, 255);
  }
  analogWrite(LED_PIN, level);
  Serial.print(F("light="));
  Serial.print(light);
  Serial.print(F(" led="));
  Serial.println(level);
  delay(200);
}

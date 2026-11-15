// pico-blink — Raspberry Pi Pico: blink the on-board LED (GP25) at 2 Hz and report the
// voltage on A0 (GP26, e.g. a potentiometer wiper) once per blink.

void setup() {
  Serial.begin(115200);
  pinMode(LED_BUILTIN, OUTPUT);
  analogReadResolution(12);
}

void loop() {
  digitalWrite(LED_BUILTIN, HIGH);
  delay(250);
  digitalWrite(LED_BUILTIN, LOW);
  delay(250);
  int raw = analogRead(A0);
  Serial.printf("A0 raw=%d %.3f V\n", raw, raw * 3.3f / 4095.0f);
}

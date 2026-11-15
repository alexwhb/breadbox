// tone() on D8 into a passive buzzer: 440 Hz then 880 Hz.
void setup() {
  Serial.begin(115200);
}

void loop() {
  tone(8, 440);
  Serial.println("tone=440");
  delay(300);
  tone(8, 880);
  Serial.println("tone=880");
  delay(300);
}

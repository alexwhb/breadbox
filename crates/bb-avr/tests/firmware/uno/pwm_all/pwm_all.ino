// analogWrite on every PWM pin of the UNO with distinct duties.
void setup() {
  analogWrite(3, 32);    // OC2B, phase-correct 490 Hz
  analogWrite(5, 64);    // OC0B, fast PWM 976.5625 Hz
  analogWrite(6, 128);   // OC0A, fast PWM
  analogWrite(9, 192);   // OC1A, phase-correct
  analogWrite(10, 200);  // OC1B, phase-correct
  analogWrite(11, 250);  // OC2A, phase-correct
}

void loop() {}

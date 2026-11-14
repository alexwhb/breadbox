// tone() on D8: 440 Hz for 500 ms, silence, then 1 kHz continuous.
void setup() {
  tone(8, 440, 500);
  delay(1000);
  tone(8, 1000);
}

void loop() {}

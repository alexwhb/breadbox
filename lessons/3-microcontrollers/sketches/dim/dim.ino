// dim — analogWrite(9, 64): D9 switches at ~490 Hz and is high 64/255 ≈ 25 % of the time,
// so the LED glows at about a quarter of full brightness.
const int LED = 9;  // a PWM pin (~)

void setup() {
  pinMode(LED, OUTPUT);
  analogWrite(LED, 64);
}

void loop() {
}

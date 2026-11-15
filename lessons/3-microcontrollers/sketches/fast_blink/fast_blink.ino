// fast_blink — an LED on D8, on for 100 ms and off for 100 ms: 5 blinks a second.
const int LED = 8;

void setup() {
  pinMode(LED, OUTPUT);
}

void loop() {
  digitalWrite(LED, HIGH);
  delay(100);
  digitalWrite(LED, LOW);
  delay(100);
}

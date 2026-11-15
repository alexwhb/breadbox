// LED on D13 (and the board's L LED) toggles every 500 ms.
void setup() {
  pinMode(13, OUTPUT);
  Serial.begin(115200);
  Serial.println("blink");
}

void loop() {
  digitalWrite(13, HIGH);
  delay(500);
  digitalWrite(13, LOW);
  delay(500);
}

// HC-SR04: TRIG on D7, ECHO on D8.
void setup() {
  Serial.begin(115200);
  pinMode(7, OUTPUT);
  pinMode(8, INPUT);
}

void loop() {
  digitalWrite(7, LOW);
  delayMicroseconds(2);
  digitalWrite(7, HIGH);
  delayMicroseconds(10);
  digitalWrite(7, LOW);
  unsigned long us = pulseIn(8, HIGH, 40000UL);
  Serial.print("us=");
  Serial.print(us);
  Serial.print(" cm=");
  Serial.println(us / 58.0, 1);
  delay(60);
}

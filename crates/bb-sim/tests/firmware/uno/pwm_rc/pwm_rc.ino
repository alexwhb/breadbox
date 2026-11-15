// analogWrite on D9 into an RC low-pass; A0 reads the filtered voltage.
void setup() {
  Serial.begin(115200);
  pinMode(9, OUTPUT);
  analogWrite(9, 64);
}

void loop() {
  Serial.print("a0=");
  Serial.println(analogRead(A0));
  delay(100);
}

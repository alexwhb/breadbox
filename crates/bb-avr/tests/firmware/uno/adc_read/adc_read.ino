// Reads A0..A5 and prints the codes, plus how long one analogRead takes.
void setup() {
  Serial.begin(115200);
}

void loop() {
  int v[6];
  unsigned long t0 = micros();
  v[0] = analogRead(A0);
  unsigned long t1 = micros();
  for (int i = 1; i < 6; i++) v[i] = analogRead(A0 + i);
  for (int i = 0; i < 6; i++) {
    Serial.print('A');
    Serial.print(i);
    Serial.print('=');
    Serial.print(v[i]);
    Serial.print(' ');
  }
  Serial.print("dt=");
  Serial.println(t1 - t0);
  delay(50);
}

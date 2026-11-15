// serial_pot — reads the knob on A0 five times a second and prints it to the Serial Monitor.
void setup() {
  Serial.begin(9600);
  Serial.println("ready");
}

void loop() {
  int value = analogRead(A0);
  Serial.print("A0 = ");
  Serial.println(value);
  delay(200);
}

// A deliberately busy sketch for the performance test: float math, integer hashing, PWM,
// digital I/O, millis/micros, ADC reads and serial output, all in a tight loop.
float acc = 0.0f;
unsigned long hash = 2166136261UL;

void setup() {
  Serial.begin(115200);
  pinMode(13, OUTPUT);
  pinMode(7, OUTPUT);
  analogWrite(9, 100);
}

void loop() {
  for (int i = 0; i < 50; i++) {
    acc = acc * 0.999f + sinf(i * 0.1f) * 0.5f;
    hash = (hash ^ (unsigned long)i) * 16777619UL;
    digitalWrite(7, i & 1);
  }
  int a = analogRead(A0);
  analogWrite(9, a >> 2);
  digitalWrite(13, (millis() / 250) & 1);
  static unsigned long last = 0;
  if (millis() - last >= 100) {
    last = millis();
    Serial.print("acc=");
    Serial.print(acc, 3);
    Serial.print(" h=");
    Serial.print(hash, HEX);
    Serial.print(" us=");
    Serial.println(micros());
  }
}

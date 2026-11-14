// Every second (by millis), toggle D7 and report millis()/micros() sampled at the toggle.
void setup() {
  Serial.begin(115200);
  pinMode(7, OUTPUT);
}

unsigned long next = 1000;
void loop() {
  if (millis() >= next) {
    PIND = _BV(7);                 // toggle D7 (PD7): marks the sampling instant
    unsigned long u = micros();
    unsigned long m = millis();
    Serial.print("m=");
    Serial.print(m);
    Serial.print(" u=");
    Serial.println(u);
    next += 1000;
  }
}

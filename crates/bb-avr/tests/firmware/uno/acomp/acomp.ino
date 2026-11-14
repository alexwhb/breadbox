// Analog comparator AIN0 (D6) vs AIN1 (D7): counts rising output edges in the ISR and
// reports ACO on request (any byte on Serial).
volatile unsigned int rises = 0;
ISR(ANALOG_COMP_vect) { rises++; }

void setup() {
  Serial.begin(115200);
  DIDR1 = _BV(AIN1D) | _BV(AIN0D);
  ACSR = _BV(ACI);
  ACSR = _BV(ACIE) | _BV(ACIS1) | _BV(ACIS0);   // interrupt on rising output edge
}

void loop() {
  if (Serial.available()) {
    Serial.read();
    Serial.print("aco=");
    Serial.print((ACSR & _BV(ACO)) ? 1 : 0);
    Serial.print(" rises=");
    Serial.println(rises);
  }
}

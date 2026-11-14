// Register-level ADC checks: internal 1.1 V reference, temperature sensor, bandgap channel,
// left adjust, external AREF, and free-running conversion rate.
volatile unsigned long conversions = 0;
ISR(ADC_vect) { conversions++; }

static uint16_t convert(uint8_t admux) {
  ADMUX = admux;
  ADCSRA = _BV(ADEN) | _BV(ADSC) | _BV(ADPS2) | _BV(ADPS1) | _BV(ADPS0);
  while (ADCSRA & _BV(ADSC)) {}
  uint8_t lo = ADCL;
  uint8_t hi = ADCH;
  return (hi << 8) | lo;
}

void setup() {
  Serial.begin(115200);
  delay(5);
  Serial.print("int1v1_a0=");
  Serial.println(convert(_BV(REFS1) | _BV(REFS0) | 0));          // A0 vs 1.1 V
  Serial.print("temp=");
  Serial.println(convert(_BV(REFS1) | _BV(REFS0) | 8));          // temperature sensor
  Serial.print("bandgap=");
  Serial.println(convert(_BV(REFS0) | 14));                      // 1.1 V vs AVcc
  Serial.print("gnd=");
  Serial.println(convert(_BV(REFS0) | 15));
  Serial.print("aref_a1=");
  Serial.println(convert(1));                                    // A1 vs AREF
  ADMUX = _BV(REFS0) | _BV(ADLAR) | 2;                           // left adjust, A2
  ADCSRA = _BV(ADEN) | _BV(ADSC) | 7;
  while (ADCSRA & _BV(ADSC)) {}
  Serial.print("adlar_h=");
  Serial.println(ADCH);
  // Free running for 1 s with interrupt counting.
  ADMUX = _BV(REFS0) | 3;
  ADCSRB = 0;
  conversions = 0;
  ADCSRA = _BV(ADEN) | _BV(ADATE) | _BV(ADIE) | _BV(ADSC) | 7;
  unsigned long start = millis();
  while (millis() - start < 1000) {}
  ADCSRA = 0;
  Serial.print("free=");
  Serial.println(conversions);
  // Auto-trigger on Timer0 overflow (every 1.024 ms) for 500 ms.
  conversions = 0;
  ADCSRB = _BV(ADTS2);                                           // Timer0 overflow
  ADCSRA = _BV(ADEN) | _BV(ADATE) | _BV(ADIE) | 7;
  start = millis();
  while (millis() - start < 500) {}
  ADCSRA = 0;
  Serial.print("t0ovf=");
  Serial.println(conversions);
}

void loop() {}

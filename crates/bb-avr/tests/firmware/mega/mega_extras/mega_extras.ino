// Mega 2560 extras: Timer5 PWM on D44/D45/D46, INT3 via attachInterrupt(D18), differential
// ADC (ADC1 - ADC0, gain 1 and 10), ADC8-15 via MUX5, and Serial3.
volatile unsigned int rises = 0;
void on_rise() { rises++; }

static int16_t diff_read(uint8_t mux5, uint8_t mux) {
  ADCSRB = mux5 ? _BV(MUX5) : 0;
  ADMUX = _BV(REFS0) | mux;
  ADCSRA = _BV(ADEN) | _BV(ADSC) | 7;
  while (ADCSRA & _BV(ADSC)) {}
  int16_t v = ADC;
  if (v & 0x200) v -= 0x400;   // 10-bit two's complement
  return v;
}

void setup() {
  Serial.begin(115200);
  Serial3.begin(57600);
  analogWrite(44, 64);
  analogWrite(45, 128);
  analogWrite(46, 192);
  attachInterrupt(digitalPinToInterrupt(18), on_rise, RISING);
  analogRead(A0);  // first conversion
  Serial.print("d10=");
  Serial.print(diff_read(0, 0x10));   // ADC0 - ADC1, x1
  Serial.print(" d01x10=");
  Serial.print(diff_read(0, 0x09));   // ADC1 - ADC0, x10
  Serial.print(" a9=");
  Serial.println(analogRead(A9));
  Serial3.println("serial3");
}

void loop() {
  static unsigned int last = 0;
  if (rises != last) {
    last = rises;
    Serial.print("rises=");
    Serial.println(last);
  }
}

// Timer1 waveform modes on OC1A (D9) / OC1B (D10), one after another, plus input capture with
// the noise canceler on ICP1 (D8). Phases are announced on Serial.
volatile uint16_t caps[4];
volatile uint8_t ncap = 0;
ISR(TIMER1_CAPT_vect) {
  if (ncap < 4) caps[ncap++] = ICR1;
}

void setup() {
  Serial.begin(115200);
  pinMode(9, OUTPUT);
  pinMode(10, OUTPUT);
  // Phase A: mode 14 fast PWM, TOP=ICR1=39999, /8 → 50 Hz; OCR1A=2999 → 1.5 ms high.
  TCCR1A = 0; TCCR1B = 0; TCNT1 = 0;
  ICR1 = 39999;
  OCR1A = 2999;
  OCR1B = 9999;
  TCCR1A = _BV(COM1A1) | _BV(COM1B1) | _BV(WGM11);
  TCCR1B = _BV(WGM13) | _BV(WGM12) | _BV(CS11);
  Serial.println("A");
  delay(100);
  // Phase B: mode 8 phase+frequency correct, TOP=ICR1=1000, /1; OCR1A=250 (25 %), OCR1B=750 inverting.
  TCCR1B = 0; TCNT1 = 0;
  ICR1 = 1000;
  OCR1A = 250;
  OCR1B = 750;
  TCCR1A = _BV(COM1A1) | _BV(COM1B1) | _BV(COM1B0);
  TCCR1B = _BV(WGM13) | _BV(CS10);
  Serial.println("B");
  delay(100);
  // Phase C: mode 4 CTC (TOP=OCR1A=7999, /1), toggle OC1A → 1 kHz.
  TCCR1B = 0; TCNT1 = 0;
  OCR1A = 7999;
  TCCR1A = _BV(COM1A0);
  TCCR1B = _BV(WGM12) | _BV(CS10);
  Serial.println("C");
  delay(100);
  // Phase D: normal mode /8, input capture on rising edges with noise canceler.
  TCCR1A = 0;
  TCCR1B = 0;
  TCNT1 = 0;
  TIFR1 = _BV(ICF1);
  TIMSK1 = _BV(ICIE1);
  TCCR1B = _BV(ICNC1) | _BV(ICES1) | _BV(CS11);
  Serial.println("D");
  while (ncap < 4) {}
  TIMSK1 = 0;
  Serial.print("cap");
  for (uint8_t i = 1; i < 4; i++) {
    Serial.print(' ');
    Serial.print((uint16_t)(caps[i] - caps[i - 1]));
  }
  Serial.println();
}

void loop() {}

// Register-level peripheral checks, one phase at a time with a serial handshake:
// EE_READY interrupt, Timer0 external clock (T0), FOC2A, comparator → Timer1 input capture
// (ACIC), ADC noise-reduction sleep, INT0 FALLING/CHANGE, GTCCR TSM, CLKPR, watchdog
// interrupt+reset mode.
#include <avr/sleep.h>
#include <avr/wdt.h>
#include <EEPROM.h>

volatile uint8_t ee_irqs = 0;
ISR(EE_READY_vect) {
  ee_irqs++;
  EECR &= ~_BV(EERIE);
}
volatile uint16_t adc_val = 0;
volatile bool adc_done = false;
ISR(ADC_vect) {
  adc_val = ADC;
  adc_done = true;
}
volatile uint16_t caps[3];
volatile uint8_t ncap = 0;
ISR(TIMER1_CAPT_vect) {
  if (ncap < 3) caps[ncap++] = ICR1;
}
volatile uint8_t int0_count = 0;
void int0_isr() { int0_count++; }
volatile bool wdt_fired = false;
ISR(WDT_vect) { wdt_fired = true; }

static void wait_go() {
  while (!Serial.available()) {}
  Serial.read();
}

void setup() {
  Serial.begin(115200);
  Serial.println("start");

  // 1. EEPROM write with the EE_READY interrupt.
  EEAR = 5;
  EEDR = 0x5A;
  cli();
  EECR = _BV(EEMPE);
  EECR |= _BV(EEPE);
  sei();
  EECR |= _BV(EERIE);
  delay(10);
  Serial.print("ee_irqs=");
  Serial.print(ee_irqs);
  Serial.print(" ee5=");
  Serial.println(EEPROM.read(5), HEX);

  // 2. Timer0 clocked from T0 (D4) rising edges.
  TIMSK0 = 0;
  TCCR0A = 0;
  TCCR0B = 0;
  TCNT0 = 0;
  TCCR0B = _BV(CS02) | _BV(CS01) | _BV(CS00);
  Serial.println("T0?");
  Serial.flush();
  wait_go();
  Serial.print("t0=");
  Serial.println(TCNT0);
  TCCR0A = _BV(WGM01) | _BV(WGM00);
  TCCR0B = _BV(CS01) | _BV(CS00);
  TIMSK0 = _BV(TOIE0);

  // 3. Force output compare on Timer2 channel A (D11) in normal mode, timer stopped.
  pinMode(11, OUTPUT);
  TCCR2A = _BV(COM2A0);
  TCCR2B = 0;
  TCCR2B = _BV(FOC2A);
  delayMicroseconds(50);
  TCCR2B = _BV(FOC2A);
  TCCR2A = _BV(WGM20);
  TCCR2B = _BV(CS22);
  Serial.println("foc");

  // 4. Analog comparator output edges captured by Timer1 (ACIC), clk/8.
  TCCR1A = 0;
  TCCR1B = _BV(ICES1) | _BV(CS11);
  TIFR1 = _BV(ICF1);
  TIMSK1 = _BV(ICIE1);
  DIDR1 = _BV(AIN1D) | _BV(AIN0D);
  ACSR = _BV(ACIC);
  Serial.println("AC?");
  Serial.flush();
  wait_go();
  while (ncap < 3) {}
  Serial.print("ac ");
  Serial.print((uint16_t)(caps[1] - caps[0]));
  Serial.print(' ');
  Serial.println((uint16_t)(caps[2] - caps[1]));
  TIMSK1 = 0;
  ACSR = _BV(ACD);
  TCCR1A = _BV(WGM10);
  TCCR1B = _BV(CS11) | _BV(CS10);

  // 5. ADC noise-reduction sleep: entering sleep starts the conversion, its interrupt wakes.
  Serial.flush();
  ADMUX = _BV(REFS0) | 2;
  ADCSRA = _BV(ADEN) | _BV(ADIE) | 7;
  set_sleep_mode(SLEEP_MODE_ADC);
  adc_done = false;
  sleep_enable();
  sei();
  sleep_cpu();
  sleep_disable();
  ADCSRA = _BV(ADEN) | 7;
  Serial.print("adcnr=");
  Serial.print(adc_val);
  Serial.print(' ');
  Serial.println(adc_done);

  // 6. INT0 FALLING, then CHANGE.
  pinMode(2, INPUT);
  attachInterrupt(digitalPinToInterrupt(2), int0_isr, FALLING);
  Serial.println("INT?");
  Serial.flush();
  wait_go();
  delay(5);
  Serial.print("fall=");
  Serial.println(int0_count);
  int0_count = 0;
  attachInterrupt(digitalPinToInterrupt(2), int0_isr, CHANGE);
  Serial.println("CHG?");
  Serial.flush();
  wait_go();
  delay(5);
  Serial.print("chg=");
  Serial.println(int0_count);
  detachInterrupt(digitalPinToInterrupt(2));

  // 7. GTCCR: TSM + PSRSYNC halts Timer0/1; releasing it restarts them.
  GTCCR = _BV(TSM) | _BV(PSRSYNC);
  TCNT1 = 0;
  delayMicroseconds(100);
  uint16_t a = TCNT1;
  GTCCR = 0;
  delayMicroseconds(100);
  uint16_t b = TCNT1;
  Serial.print("tsm ");
  Serial.print(a);
  Serial.print(' ');
  Serial.println(b);

  // 8. CLKPR /2: delay(100) lasts 200 ms of real time (marked on D7).
  Serial.println("CLK");
  Serial.flush();
  pinMode(7, OUTPUT);
  PORTD |= _BV(7);
  cli();
  CLKPR = _BV(CLKPCE);
  CLKPR = _BV(CLKPS0);
  sei();
  delay(100);
  PORTD &= ~_BV(7);
  cli();
  CLKPR = _BV(CLKPCE);
  CLKPR = 0;
  sei();
  Serial.println("clkdone");

  // 9. Watchdog interrupt + reset mode (16 ms): first timeout interrupts, second resets.
  Serial.flush();
  cli();
  wdt_reset();
  WDTCSR = _BV(WDCE) | _BV(WDE);
  WDTCSR = _BV(WDIE) | _BV(WDE);
  sei();
  while (!wdt_fired) {}
  Serial.println("wdt irq");
  Serial.flush();
  while (1) {}
}

void loop() {}

// attachInterrupt on D2 (INT0, RISING) toggles D4 via the Arduino handler; a PCINT0 ISR on D8
// (PB0) toggles D5; a naked PCINT1 ISR on A0 (PC0) sets D6 with minimal, exactly known latency.
#include <avr/interrupt.h>

volatile unsigned int int0_count = 0;
void on_rise() {
  PIND = _BV(4);
  int0_count++;
}

ISR(PCINT0_vect) {
  PIND = _BV(5);
}

ISR(PCINT1_vect, ISR_NAKED) {
  asm volatile("sbi %0, 6\n\t"
               "reti" ::"I"(_SFR_IO_ADDR(PORTD)));
}

void setup() {
  Serial.begin(115200);
  pinMode(4, OUTPUT);
  pinMode(5, OUTPUT);
  pinMode(6, OUTPUT);
  pinMode(2, INPUT);
  pinMode(8, INPUT);
  pinMode(A0, INPUT);
  attachInterrupt(digitalPinToInterrupt(2), on_rise, RISING);
  PCMSK0 = _BV(PCINT0);
  PCMSK1 = _BV(PCINT8);
  PCIFR = _BV(PCIF0) | _BV(PCIF1);
  PCICR = _BV(PCIE0) | _BV(PCIE1);
  Serial.println("ready");
}

void loop() {
  static unsigned int last = 0;
  if (int0_count != last) {
    last = int0_count;
    Serial.print("int0=");
    Serial.println(last);
  }
}

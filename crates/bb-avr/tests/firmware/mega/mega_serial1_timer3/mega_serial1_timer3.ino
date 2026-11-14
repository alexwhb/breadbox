// Mega 2560: Serial1 at 9600, Timer3 CTC toggling OC3A (D5, PE3) at 1 kHz with a compare ISR
// counting half-periods; the count after ~1 s goes out on Serial (USART0).
volatile unsigned long t3 = 0;
ISR(TIMER3_COMPA_vect) { t3++; }

void setup() {
  Serial.begin(115200);
  Serial1.begin(9600);
  Serial1.println("mega serial1");
  pinMode(5, OUTPUT);
  TCCR3A = _BV(COM3A0);                 // toggle OC3A on compare match
  TCCR3B = _BV(WGM32) | _BV(CS31);      // CTC, TOP=OCR3A, clk/8
  OCR3A = 999;                          // 2 kHz match rate → 1 kHz square wave
  TIMSK3 = _BV(OCIE3A);
  delay(1000);
  noInterrupts();
  unsigned long n = t3;
  interrupts();
  Serial.print("t3=");
  Serial.println(n);
  Serial1.print("done ");
  Serial1.println(n);
}

void loop() {}

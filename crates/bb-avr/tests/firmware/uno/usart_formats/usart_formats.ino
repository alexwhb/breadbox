// Frame formats: 7E2 at 9600 baud, then 8O1 with U2X off at 19200; echoes received bytes and
// reports parity / frame errors seen by the receiver.
void setup() {
  Serial.begin(9600, SERIAL_7E2);
  Serial.print("7E2");
  Serial.flush();
  Serial.end();
  // 8O1, U2X off: UBRR = 16e6/(16*19200) - 1 = 51 (19230.8 baud)
  UBRR0H = 0;
  UBRR0L = 51;
  UCSR0A = 0;
  UCSR0C = _BV(UPM01) | _BV(UPM00) | _BV(UCSZ01) | _BV(UCSZ00);
  UCSR0B = _BV(RXEN0) | _BV(TXEN0);
  const char *msg = "8O1";
  for (const char *p = msg; *p; p++) {
    while (!(UCSR0A & _BV(UDRE0))) {}
    UDR0 = *p;
  }
}

void loop() {
  if (UCSR0A & _BV(RXC0)) {
    uint8_t st = UCSR0A;
    uint8_t c = UDR0;
    while (!(UCSR0A & _BV(UDRE0))) {}
    UDR0 = (st & _BV(UPE0)) ? '!' : (st & _BV(FE0)) ? '#' : c;
  }
}

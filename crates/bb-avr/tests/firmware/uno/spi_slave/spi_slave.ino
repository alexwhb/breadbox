// SPI slave (mode 0): preloads 0x77, then answers each byte with (received + 1).
volatile byte last = 0;
volatile bool fresh = false;

ISR(SPI_STC_vect) {
  byte b = SPDR;
  SPDR = b + 1;
  last = b;
  fresh = true;
}

void setup() {
  Serial.begin(115200);
  pinMode(MISO, OUTPUT);
  SPCR = _BV(SPE) | _BV(SPIE);
  SPDR = 0x77;
}

void loop() {
  if (fresh) {
    fresh = false;
    Serial.print("slave got ");
    Serial.println(last, HEX);
  }
}

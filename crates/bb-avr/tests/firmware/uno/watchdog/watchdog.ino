// Watchdog: interrupt mode first (count 16 ms ticks for ~500 ms), then reset mode with 250 ms
// timeout; feeds it 5 times then stops feeding.
#include <avr/wdt.h>

volatile unsigned int wdt_ticks = 0;
ISR(WDT_vect) { wdt_ticks++; }

void setup() {
  Serial.begin(115200);
  Serial.println("boot");
  cli();
  wdt_reset();
  WDTCSR = _BV(WDCE) | _BV(WDE);
  WDTCSR = _BV(WDIE);               // interrupt mode, 16 ms
  sei();
  delay(500);
  cli();
  wdt_reset();
  WDTCSR = _BV(WDCE) | _BV(WDE);
  WDTCSR = 0;
  sei();
  Serial.print("wdt_irqs=");
  Serial.println(wdt_ticks);
  wdt_enable(WDTO_250MS);
}

int fed = 0;
void loop() {
  if (fed < 5) {
    wdt_reset();
    delay(100);
    fed++;
    Serial.println("fed");
    PIND = _BV(7);  // mark last feed on D7
  }
}

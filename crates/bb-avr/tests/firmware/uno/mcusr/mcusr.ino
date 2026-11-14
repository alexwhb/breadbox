// Captures MCUSR before anything else runs (for bare-chip reset tests) and prints it.
#include <avr/wdt.h>
uint8_t mcusr_at_boot __attribute__((section(".noinit")));

void early(void) __attribute__((naked, used, section(".init3")));
void early(void) {
  mcusr_at_boot = MCUSR;
  MCUSR = 0;
  wdt_disable();
}

void setup() {
  Serial.begin(115200);
  Serial.print("mcusr=");
  Serial.println(mcusr_at_boot);
}

void loop() {}

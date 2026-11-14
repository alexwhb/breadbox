// Sleeps in power-down until D2 is pulled low (INT0 level interrupt), then reports millis().
#include <avr/sleep.h>

void wake() {}

void setup() {
  Serial.begin(115200);
  pinMode(2, INPUT_PULLUP);
  Serial.print("sleeping at ");
  Serial.println(millis());
  Serial.flush();
  set_sleep_mode(SLEEP_MODE_PWR_DOWN);
  noInterrupts();
  sleep_enable();
  attachInterrupt(digitalPinToInterrupt(2), wake, LOW);
  interrupts();
  sleep_cpu();
  sleep_disable();
  detachInterrupt(digitalPinToInterrupt(2));
  Serial.print("woke at ");
  Serial.println(millis());
  // Idle sleep: Timer0 keeps running, so millis advances and each overflow wakes the core.
  set_sleep_mode(SLEEP_MODE_IDLE);
  unsigned long t0 = millis();
  unsigned int wakes = 0;
  while (millis() - t0 < 100) {
    sleep_mode();
    wakes++;
  }
  Serial.print("idle wakes=");
  Serial.println(wakes);
}

void loop() {}

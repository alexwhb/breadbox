// Servo on D9: 0°, 90°, 180°, one second each.
#include <Servo.h>
Servo s;

void setup() {
  s.attach(9);
  s.write(0);
  delay(1000);
  s.write(90);
  delay(1000);
  s.write(180);
}

void loop() {}

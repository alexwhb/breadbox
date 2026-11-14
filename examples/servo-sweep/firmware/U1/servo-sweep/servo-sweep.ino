// servo-sweep — an SG90 on D9 sweeps 0° → 180° → 0°, one degree every 15 ms.
#include <Servo.h>

Servo servo;

void setup() {
  servo.attach(9);
}

void loop() {
  for (int pos = 0; pos <= 180; pos++) {
    servo.write(pos);
    delay(15);
  }
  for (int pos = 180; pos >= 0; pos--) {
    servo.write(pos);
    delay(15);
  }
}

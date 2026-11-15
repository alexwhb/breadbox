// Servo library sweeping 0..180 in 10 degree steps on D9.
#include <Servo.h>
Servo s;

void setup() {
  Serial.begin(115200);
  s.attach(9);
}

void loop() {
  for (int a = 0; a <= 180; a += 30) {
    s.write(a);
    Serial.print("angle=");
    Serial.println(a);
    delay(400);
  }
}

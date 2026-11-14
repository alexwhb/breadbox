// First boot writes a record to EEPROM (timing the writes); later boots read it back.
#include <EEPROM.h>

void setup() {
  Serial.begin(115200);
  if (EEPROM.read(0) != 0x42) {
    unsigned long t0 = micros();
    EEPROM.write(0, 0x42);
    EEPROM.write(1, 'B');
    EEPROM.write(2, 'B');
    EEPROM.write(3, 7);
    unsigned long t1 = micros();
    EEPROM.put(10, 3.25f);
    Serial.print("written dt=");
    Serial.println(t1 - t0);
  } else {
    float f;
    EEPROM.get(10, f);
    Serial.print("read ");
    Serial.print((char)EEPROM.read(1));
    Serial.print((char)EEPROM.read(2));
    Serial.print(' ');
    Serial.print(EEPROM.read(3));
    Serial.print(' ');
    Serial.println(f, 2);
  }
}

void loop() {}

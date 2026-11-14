// SoftwareSerial on D10 (RX) / D11 (TX) at 9600: greets, then echoes what it receives to the
// hardware serial port (and back out of the soft TX upper-cased).
#include <SoftwareSerial.h>
SoftwareSerial soft(10, 11);

void setup() {
  Serial.begin(115200);
  soft.begin(9600);
  soft.println("hello soft");
}

void loop() {
  while (soft.available()) {
    char c = soft.read();
    Serial.write(c);
    if (c >= 'a' && c <= 'z') c -= 32;
    soft.write(c);
  }
}

// SPI master (mode 0, 1 MHz): sends 0xA5 0x3C while SS (D10) is low and prints what came back.
#include <SPI.h>

void setup() {
  Serial.begin(115200);
  SPI.begin();
  SPI.beginTransaction(SPISettings(1000000, MSBFIRST, SPI_MODE0));
  digitalWrite(SS, LOW);
  byte a = SPI.transfer(0xA5);
  byte b = SPI.transfer(0x3C);
  digitalWrite(SS, HIGH);
  SPI.endTransaction();
  Serial.print("spi ");
  Serial.print(a, HEX);
  Serial.print(' ');
  Serial.println(b, HEX);
}

void loop() {}

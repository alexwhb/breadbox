// Hardware SPI traffic for the logic analyzer test: a 74HC595 on the SPI pins (MOSI D11 → DS,
// SCK D13 → SHCP, D10 → STCP as chip select) with its serial out Q7' wired back to MISO D12.
// Every 50 ms one CS frame sends 0x5A, 0xC3; the second transfer reads back the first byte as
// it leaves the shift register. Prints what MISO returned.
#include <SPI.h>

void setup() {
  Serial.begin(115200);
  pinMode(10, OUTPUT);
  digitalWrite(10, HIGH);
  SPI.begin();
}

void loop() {
  SPI.beginTransaction(SPISettings(1000000, MSBFIRST, SPI_MODE0));
  digitalWrite(10, LOW);
  byte a = SPI.transfer(0x5A);
  byte b = SPI.transfer(0xC3);
  digitalWrite(10, HIGH);
  SPI.endTransaction();
  Serial.print("rx=");
  Serial.print(a, HEX);
  Serial.print(",");
  Serial.println(b, HEX);
  delay(50);
}

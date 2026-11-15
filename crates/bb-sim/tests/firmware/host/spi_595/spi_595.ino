// ESP32 SPI → 74HC595: SPI.transfer shifts the byte in, a GPIO pulse on the latch shows it.
#include <SPI.h>

const int LATCH = 5;
uint8_t value = 0xA5;

void setup() {
  Serial.begin(115200);
  pinMode(LATCH, OUTPUT);
  digitalWrite(LATCH, LOW);
  SPI.begin();
}

void loop() {
  SPI.transfer(value);
  digitalWrite(LATCH, HIGH);
  digitalWrite(LATCH, LOW);
  Serial.printf("sent=0x%02X\n", value);
  value = (uint8_t)(value + 1);
  delay(200);
}

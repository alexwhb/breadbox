// LiquidCrystal on the full 8-bit bus: RS=12 E=11 D0..D7 = 2..9. A custom character (CGRAM),
// a ROM character above 0x7F (°) and a counter on line 2.
#include <LiquidCrystal.h>
LiquidCrystal lcd(12, 11, 2, 3, 4, 5, 6, 7, 8, 9);

byte bell[8] = {0b00100, 0b01110, 0b01110, 0b01110, 0b11111, 0b00000, 0b00100, 0b00000};

void setup() {
  lcd.begin(16, 2);
  lcd.createChar(1, bell);
  // createChar leaves the address counter in CGRAM (as on the real chip): back to DDRAM
  lcd.setCursor(0, 0);
  lcd.print("8-bit bus OK");
  lcd.setCursor(0, 1);
  lcd.write((uint8_t)1);
}

void loop() {
  lcd.setCursor(2, 1);
  lcd.print("n=");
  lcd.print(millis() / 100);
  lcd.write((uint8_t)0xDF);
  delay(100);
}

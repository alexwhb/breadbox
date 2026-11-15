// LiquidCrystal, 4-bit: RS=12 E=11 D4..D7=5,4,3,2.
#include <LiquidCrystal.h>
LiquidCrystal lcd(12, 11, 5, 4, 3, 2);

void setup() {
  lcd.begin(16, 2);
  lcd.print("Hello, Breadbox!");
}

void loop() {
  lcd.setCursor(0, 1);
  lcd.print("t=");
  lcd.print(millis() / 1000);
  lcd.print("s ");
  lcd.write((uint8_t)0xDF);
  delay(200);
}

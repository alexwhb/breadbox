// LiquidCrystal_I2C on a PCF8574 backpack at 0x27.
#include <Wire.h>
#include <LiquidCrystal_I2C.h>
LiquidCrystal_I2C lcd(0x27, 16, 2);

void setup() {
  Serial.begin(115200);
  lcd.init();
  lcd.backlight();
  lcd.setCursor(0, 0);
  lcd.print("I2C backpack");
  lcd.setCursor(0, 1);
  lcd.print("works!");
  Serial.println("lcd: done");
}

void loop() {}

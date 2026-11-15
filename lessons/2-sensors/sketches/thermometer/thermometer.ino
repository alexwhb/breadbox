// thermometer — a TMP36 on A0 and a 16×2 character LCD (HD44780, 4-bit mode).
//
//   TMP36: Vout = 0.5 V + 10 mV/°C        LCD: RS D12, E D11, D4–D7 on D5, D4, D3, D2
#include <LiquidCrystal.h>

LiquidCrystal lcd(12, 11, 5, 4, 3, 2);

float minC = 1000, maxC = -1000;

float readCelsius() {
  long sum = 0;
  for (int i = 0; i < 8; i++) sum += analogRead(A0);  // average out ADC noise
  float volts = sum / 8.0 * 5.0 / 1024.0;
  return (volts - 0.5) * 100.0;
}

void setup() {
  Serial.begin(115200);
  lcd.begin(16, 2);
  lcd.print(F("Thermometer"));
  delay(500);
}

void loop() {
  float c = readCelsius();
  minC = min(minC, c);
  maxC = max(maxC, c);
  lcd.clear();
  lcd.print(F("Temp: "));
  lcd.print(c, 1);
  lcd.print((char)223);  // degree sign in the HD44780 ROM
  lcd.print('C');
  lcd.setCursor(0, 1);
  lcd.print(F("lo "));
  lcd.print(minC, 1);
  lcd.print(F(" hi "));
  lcd.print(maxC, 1);
  Serial.print(c, 1);
  Serial.println(F(" C"));
  delay(1000);
}

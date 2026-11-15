// Adafruit SSD1306 128x64 over I2C (0x3C).
#include <Wire.h>
#include <Adafruit_GFX.h>
#include <Adafruit_SSD1306.h>
Adafruit_SSD1306 d(128, 64, &Wire, -1);

void setup() {
  Serial.begin(115200);
  if (!d.begin(SSD1306_SWITCHCAPVCC, 0x3C)) {
    Serial.println("oled: alloc failed");
    while (1) {}
  }
  d.clearDisplay();
  d.setTextSize(2);
  d.setTextColor(SSD1306_WHITE);
  d.setCursor(0, 0);
  d.println("Hi!");
  d.fillRect(100, 40, 20, 20, SSD1306_WHITE);
  d.display();
  Serial.println("oled: done");
}

void loop() {}

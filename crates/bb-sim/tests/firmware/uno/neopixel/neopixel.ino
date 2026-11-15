// Adafruit NeoPixel: 8 pixels on D6.
#include <Adafruit_NeoPixel.h>
Adafruit_NeoPixel strip(8, 6, NEO_GRB + NEO_KHZ800);

void setup() {
  Serial.begin(115200);
  strip.begin();
  strip.setPixelColor(0, strip.Color(255, 0, 0));
  strip.setPixelColor(1, strip.Color(0, 255, 0));
  strip.setPixelColor(7, strip.Color(0, 0, 64));
  strip.show();
  Serial.println("ws: shown");
}

void loop() {}

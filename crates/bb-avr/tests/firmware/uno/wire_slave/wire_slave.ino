// I2C slave at 0x42: prints what it receives, answers requests with "OK!".
#include <Wire.h>

volatile byte got[8];
volatile byte ngot = 0;
volatile bool fresh = false;

void onRx(int n) {
  ngot = 0;
  while (Wire.available() && ngot < 8) got[ngot++] = Wire.read();
  fresh = true;
}

void onReq() {
  Wire.write("OK!");
}

void setup() {
  Serial.begin(115200);
  Wire.begin(0x42);
  Wire.onReceive(onRx);
  Wire.onRequest(onReq);
}

void loop() {
  if (fresh) {
    fresh = false;
    Serial.print("got");
    for (byte i = 0; i < ngot; i++) {
      Serial.print(' ');
      Serial.print(got[i], HEX);
    }
    Serial.println();
  }
}

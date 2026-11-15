// Full-step drive of a 28BYJ-48 through a ULN2003 on D8..D11: 64 steps forward, then back.
const uint8_t pins[4] = {8, 9, 10, 11};
void coil(uint8_t k) {
  for (uint8_t i = 0; i < 4; i++) digitalWrite(pins[i], i == k || i == ((k + 1) & 3));
}

void setup() {
  Serial.begin(115200);
  for (uint8_t i = 0; i < 4; i++) pinMode(pins[i], OUTPUT);
  for (int s = 0; s < 64; s++) {
    coil(s & 3);
    delay(4);
  }
  Serial.println("steps: forward done");
}

void loop() {}

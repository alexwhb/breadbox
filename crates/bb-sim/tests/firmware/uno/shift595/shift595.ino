// 74HC595: latch D8, data D11, clock D12.
void setup() {
  pinMode(8, OUTPUT);
  pinMode(11, OUTPUT);
  pinMode(12, OUTPUT);
  Serial.begin(115200);
}

void loop() {
  const uint8_t vals[] = {0xA5, 0x3C};
  for (uint8_t i = 0; i < 2; i++) {
    digitalWrite(8, LOW);
    shiftOut(11, 12, MSBFIRST, vals[i]);
    digitalWrite(8, HIGH);
    Serial.print("out=");
    Serial.println(vals[i]);
    delay(200);
  }
}

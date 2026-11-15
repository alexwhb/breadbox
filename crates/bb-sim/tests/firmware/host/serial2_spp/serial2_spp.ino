// ESP32 Serial2 (RX2 = GPIO16, TX2 = GPIO17) wired to an HC-05: the sketch talks to the
// virtual phone's Bluetooth terminal through a real UART waveform on the pins.
void setup() {
  Serial.begin(115200);
  Serial2.begin(9600);
}

String line;
unsigned long last = 0;

void loop() {
  if (millis() - last >= 500) {
    last = millis();
    Serial2.print("hello\n");
  }
  while (Serial2.available()) {
    char c = (char)Serial2.read();
    if (c == '\n') {
      Serial.printf("got: %s\n", line.c_str());
      line = "";
    } else {
      line += c;
    }
  }
  delay(5);
}

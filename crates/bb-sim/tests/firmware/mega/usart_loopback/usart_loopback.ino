// Mega 2560 hardware USART1 and USART2 looped back on the bench: TX1 (D18) → RX1 (D19) and
// TX2 (D16) → RX2 (D17). Every 100 ms each port sends a numbered line; whatever comes back is
// printed to the USB serial (USART0) as "got1: …" / "got2: …".
String line1, line2;
unsigned long last = 0;
int n = 0;

static void pump(HardwareSerial& port, String& line, const char* tag) {
  while (port.available()) {
    char c = (char)port.read();
    if (c == '\n') {
      Serial.print(tag);
      Serial.print(": ");
      Serial.println(line);
      line = "";
    } else {
      line += c;
    }
  }
}

void setup() {
  Serial.begin(115200);
  Serial1.begin(9600);
  Serial2.begin(38400);
}

void loop() {
  if (millis() - last >= 100) {
    last = millis();
    n++;
    Serial1.print("one ");
    Serial1.print(n);
    Serial1.print('\n');
    Serial2.print("two ");
    Serial2.print(n);
    Serial2.print('\n');
  }
  pump(Serial1, line1, "got1");
  pump(Serial2, line2, "got2");
}

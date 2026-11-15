// Serial1 and Serial2 looped back on the same board: each UART's TX wired to its own RX on the
// bench. Every 200 ms each port sends a numbered line; whatever comes back on its RX is
// printed to the USB serial as "got1: …" / "got2: …".
//
// ESP32: Serial1's default pins (GPIO9/10) drive the flash chip and are not on the DevKit
// header, so it is routed to RX = GPIO26 (D26), TX = GPIO25 (D25) like real sketches do;
// Serial2 stays on RX2 / TX2. Pico: Serial1 = UART0 (GP1 / GP0), Serial2 = UART1 (GP9 / GP8).
void setup() {
  Serial.begin(115200);
#if defined(ARDUINO_ARCH_ESP32)
  Serial1.begin(9600, SERIAL_8N1, 26, 25);
#else
  Serial1.begin(9600);
#endif
  Serial2.begin(19200);
}

String line1, line2;
unsigned long last = 0;
int n = 0;

static void pump(HardwareSerial& port, String& line, const char* tag) {
  while (port.available()) {
    char c = (char)port.read();
    if (c == '\n') {
      Serial.printf("%s: %s\n", tag, line.c_str());
      line = "";
    } else {
      line += c;
    }
  }
}

void loop() {
  if (millis() - last >= 200) {
    last = millis();
    n++;
    Serial1.printf("one %d\n", n);
    Serial2.printf("two %d\n", n);
  }
  pump(Serial1, line1, "got1");
  pump(Serial2, line2, "got2");
  delay(2);
}

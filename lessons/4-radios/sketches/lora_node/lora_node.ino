// ESP32 + RYLR896 on Serial2 (RX2 = GPIO16 ← TXD, TX2 = GPIO17 → RXD): a LoRa sensor node.
// Sends "t=21.5,h=40" every 2 s to the gateway and prints every downlink it receives.
String line;
unsigned long last = 0;

void at(const char* cmd) {
  Serial2.print(cmd);
  Serial2.print("\r\n");
  delay(120);
}

void setup() {
  Serial.begin(115200);
  Serial2.begin(115200);
  delay(300);
  at("AT+ADDRESS=7");
  at("AT+NETWORKID=0");
  at("AT+PARAMETER=7,7,1,4");  // SF7 · 125 kHz · 4/5 · preamble 4: short airtime
  Serial.println("lora node ready");
}

void loop() {
  if (millis() - last >= 2000) {
    last = millis();
    Serial2.print("AT+SEND=0,11,t=21.5,h=40\r\n");
  }
  while (Serial2.available()) {
    char c = (char)Serial2.read();
    if (c == '\n') {
      line.trim();
      if (line.startsWith("+RCV=")) Serial.printf("rx: %s\n", line.c_str());
      else if (line.length()) Serial.printf("lora: %s\n", line.c_str());
      line = "";
    } else {
      line += c;
    }
  }
  delay(5);
}

// Hardware USART0 looped back on the bench: D1 (TX) wired to D0 (RX). Every 100 ms the sketch
// sends "ping N"; every "ping N" line it receives back is answered with "pong N" (which also
// comes back, and is ignored), so the serial monitor reads ping 1, pong 1, ping 2, pong 2, …
String line;
unsigned long last = 0;
int n = 0;

void setup() {
  Serial.begin(57600);
}

void loop() {
  if (millis() - last >= 100) {
    last = millis();
    n++;
    Serial.print("ping ");
    Serial.println(n);
  }
  while (Serial.available()) {
    char c = (char)Serial.read();
    if (c == '\r') continue;
    if (c == '\n') {
      if (line.startsWith("ping ")) {
        Serial.print("pong ");
        Serial.println(line.substring(5));
      }
      line = "";
    } else {
      line += c;
    }
  }
}

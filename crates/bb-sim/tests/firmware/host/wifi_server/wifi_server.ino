// ESP32 WiFiServer: a tiny HTTP server on port 80. Every request line is echoed to the USB
// serial ("served GET /path HTTP/1.0") and answered with a plain-text page that counts hits.
#include <WiFi.h>

WiFiServer server(80);
int hits = 0;

void setup() {
  Serial.begin(115200);
  WiFi.begin("SIM-AP", "breadbox");
  while (WiFi.status() != WL_CONNECTED) delay(100);
  server.begin();
  Serial.print("ip=");
  Serial.println(WiFi.localIP());
}

void loop() {
  WiFiClient client = server.available();
  if (!client) {
    delay(5);
    return;
  }
  String line = client.readStringUntil('\n');
  line.trim();
  // the rest of the request header, up to the blank line
  while (client.connected()) {
    String h = client.readStringUntil('\n');
    if (h.length() <= 1) break;
  }
  hits++;
  client.print("HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n");
  client.printf("hello from esp32, hit %d\n", hits);
  client.stop();
  Serial.printf("served %s\n", line.c_str());
}

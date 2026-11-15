// esp32-weather — an ESP32 DevKit reads a DHT22 every 2 s and publishes it over Wi-Fi to the
// bench's MQTT broker. Send "led on" / "led off" to weather/cmd to drive the on-board LED.
//
//   DHT22: VCC 3V3, DATA → GPIO4 (10 kΩ pull-up), GND
#include <WiFi.h>
#include <PubSubClient.h>
#include <DHT.h>

const char* SSID = "SIM-AP";
const char* PASS = "breadbox";
const char* BROKER = "broker.sim";
const int DHT_PIN = 4;

DHT dht(DHT_PIN, DHT22);
WiFiClient net;
PubSubClient mqtt(net);
unsigned long lastPublish = 0;

void connectWiFi() {
  Serial.print("Wi-Fi: joining ");
  Serial.println(SSID);
  WiFi.mode(WIFI_STA);
  WiFi.begin(SSID, PASS);
  while (WiFi.status() != WL_CONNECTED) {
    delay(250);
    Serial.print('.');
  }
  Serial.println();
  Serial.print("Wi-Fi: connected, IP ");
  Serial.print(WiFi.localIP());
  Serial.printf(", RSSI %d dBm\n", (int)WiFi.RSSI());
}

void onMessage(char* topic, byte* payload, unsigned int len) {
  String msg;
  for (unsigned int i = 0; i < len; i++) msg += (char)payload[i];
  Serial.printf("MQTT: %s = %s\n", topic, msg.c_str());
  if (msg == "led on") digitalWrite(LED_BUILTIN, HIGH);
  if (msg == "led off") digitalWrite(LED_BUILTIN, LOW);
}

void connectMqtt() {
  while (!mqtt.connected()) {
    Serial.printf("MQTT: connecting to %s\n", BROKER);
    if (mqtt.connect("esp32-weather", "weather/status", 0, true, "offline")) {
      mqtt.publish("weather/status", "online", true);
      mqtt.subscribe("weather/cmd");
      Serial.println("MQTT: connected");
    } else {
      Serial.printf("MQTT: failed (state %d), retrying in 2 s\n", mqtt.state());
      delay(2000);
    }
  }
}

void setup() {
  Serial.begin(115200);
  pinMode(LED_BUILTIN, OUTPUT);
  dht.begin();
  connectWiFi();
  mqtt.setServer(BROKER, 1883);
  mqtt.setCallback(onMessage);
  connectMqtt();
}

void loop() {
  if (!mqtt.connected()) connectMqtt();
  mqtt.loop();
  if (millis() - lastPublish >= 2000) {
    lastPublish = millis();
    float t = dht.readTemperature();
    float h = dht.readHumidity();
    if (isnan(t) || isnan(h)) {
      Serial.println("DHT22: read failed");
      return;
    }
    char json[64];
    snprintf(json, sizeof json, "{\"t\":%.1f,\"h\":%.1f}", t, h);
    mqtt.publish("weather/esp32", json);
    Serial.printf("t=%.1f C h=%.1f %% -> weather/esp32\n", t, h);
  }
  delay(10);
}

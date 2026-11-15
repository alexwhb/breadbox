// ESP32 BLE server (Nordic UART style) + HTTP client + UDP, for bb-radio's host-radio test.
#include <BLE2902.h>
#include <BLEDevice.h>
#include <BLEServer.h>
#include <HTTPClient.h>
#include <WiFi.h>
#include <WiFiUdp.h>

#define SERVICE_UUID "6e400001-b5a3-f393-e0a9-e50e24dcca9e"
#define RX_UUID "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
#define TX_UUID "6e400003-b5a3-f393-e0a9-e50e24dcca9e"

BLECharacteristic* tx = nullptr;
bool connected = false;
int counter = 0;

class ServerCb : public BLEServerCallbacks {
  void onConnect(BLEServer*) override {
    connected = true;
    Serial.println("ble: connected");
  }
  void onDisconnect(BLEServer* s) override {
    connected = false;
    Serial.println("ble: disconnected");
    s->startAdvertising();
  }
};

class RxCb : public BLECharacteristicCallbacks {
  void onWrite(BLECharacteristic* c) override {
    std::string v = c->getValue();
    Serial.printf("ble rx: %s\n", v.c_str());
    if (v == "reset") counter = 0;
  }
};

void setup() {
  Serial.begin(115200);
  BLEDevice::init("esp32-uart");
  BLEServer* server = BLEDevice::createServer();
  server->setCallbacks(new ServerCb());
  BLEService* svc = server->createService(SERVICE_UUID);
  tx = svc->createCharacteristic(TX_UUID, BLECharacteristic::PROPERTY_NOTIFY | BLECharacteristic::PROPERTY_READ);
  tx->addDescriptor(new BLE2902());
  BLECharacteristic* rx = svc->createCharacteristic(RX_UUID, BLECharacteristic::PROPERTY_WRITE);
  rx->setCallbacks(new RxCb());
  svc->start();
  server->getAdvertising()->addServiceUUID(SERVICE_UUID);
  server->getAdvertising()->start();
  Serial.println("ble: advertising");

  WiFi.begin("SIM-AP", "breadbox");
  while (WiFi.status() != WL_CONNECTED) delay(100);
  Serial.printf("wifi: %s\n", WiFi.localIP().toString().c_str());
  HTTPClient http;
  http.begin("http://http.sim/get?board=esp32");
  int code = http.GET();
  String body = http.getString();
  Serial.printf("http: %d %s\n", code, body.indexOf("\"board\": \"esp32\"") >= 0 ? "args ok" : body.c_str());
  http.end();
  WiFiUDP udp;
  udp.begin(4210);
  udp.beginPacket("echo.sim", 7);
  udp.print("udp-ping");
  udp.endPacket();
  unsigned long t0 = millis();
  while (millis() - t0 < 1000 && !udp.parsePacket()) delay(5);
  char buf[16] = {0};
  udp.read(buf, sizeof buf - 1);
  Serial.printf("udp: %s from %s\n", buf, udp.remoteIP().toString().c_str());
  IPAddress ip;
  Serial.printf("dns: %d %s\n", WiFi.hostByName("broker.sim", ip), ip.toString().c_str());
}

void loop() {
  if (connected) {
    char msg[24];
    snprintf(msg, sizeof msg, "count=%d", counter++);
    tx->setValue(msg);
    tx->notify();
  }
  delay(500);
}

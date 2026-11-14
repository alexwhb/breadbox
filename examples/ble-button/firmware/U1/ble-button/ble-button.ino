// ble-button — an UNO with an HM-10 BLE module. Press the button and the phone gets a
// notification; type "on" / "off" in the phone's Bluetooth terminal to switch the LED.
//
//   HM-10 TXD → D10 (SoftwareSerial RX)    HM-10 RXD ← D11 (TX, through a divider: 3.3 V input)
//   button D4 → GND (internal pull-up)     LED on D13
#include <SoftwareSerial.h>

const int BUTTON = 4;
const int LED = 13;

SoftwareSerial ble(10, 11);  // RX, TX → HM-10

bool connected = false;
int presses = 0;
char line[24];
uint8_t len = 0;

// Sends an AT command (HM-10 commands have no line ending) and prints the answer.
void at(const char* cmd) {
  ble.print(cmd);
  unsigned long t0 = millis();
  String reply;
  while (millis() - t0 < 300) {
    while (ble.available()) reply += (char)ble.read();
  }
  Serial.print(cmd);
  Serial.print(F(" -> "));
  Serial.println(reply);
}

void handle(const char* text) {
  if (strcmp(text, "OK+CONN") == 0) {
    connected = true;
    Serial.println(F("phone connected"));
  } else if (strcmp(text, "OK+LOST") == 0) {
    connected = false;
    Serial.println(F("phone disconnected"));
  } else if (strcmp(text, "on") == 0) {
    digitalWrite(LED, HIGH);
    ble.print(F("led on\n"));
  } else if (strcmp(text, "off") == 0) {
    digitalWrite(LED, LOW);
    ble.print(F("led off\n"));
  } else if (strcmp(text, "?") == 0) {
    ble.print(F("presses="));
    ble.print(presses);
    ble.print('\n');
  }
}

void setup() {
  Serial.begin(115200);
  ble.begin(9600);
  pinMode(BUTTON, INPUT_PULLUP);
  pinMode(LED, OUTPUT);
  delay(400);  // HM-10 boots in ~300 ms
  at("AT");
  at("AT+NAMEbb-button");
  at("AT+NOTI1");  // report OK+CONN / OK+LOST
  Serial.println(F("ble-button ready"));
}

void loop() {
  // Button with a simple 20 ms debounce.
  static bool last = HIGH;
  static unsigned long changed = 0;
  bool now = digitalRead(BUTTON);
  if (now != last && millis() - changed > 20) {
    changed = millis();
    last = now;
    if (now == LOW) {
      presses++;
      Serial.print(F("pressed #"));
      Serial.println(presses);
      if (connected) {
        ble.print(F("pressed #"));
        ble.print(presses);
        ble.print('\n');
      }
    }
  }
  // Lines from the module: connection status or phone commands.
  while (ble.available()) {
    char c = ble.read();
    if (c == '\n' || c == '\r') {
      line[len] = 0;
      if (len) handle(line);
      len = 0;
    } else if (len < sizeof line - 1) {
      line[len++] = c;
      line[len] = 0;
      // Status messages arrive without a line ending.
      if (strcmp(line, "OK+CONN") == 0 || strcmp(line, "OK+LOST") == 0) {
        handle(line);
        len = 0;
      }
    }
  }
}

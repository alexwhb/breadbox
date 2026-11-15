// Push button on D2 to GND with INPUT_PULLUP; FALLING interrupts are counted.
volatile unsigned int presses = 0;
void onFall() { presses++; }

void setup() {
  Serial.begin(115200);
  pinMode(2, INPUT_PULLUP);
  attachInterrupt(digitalPinToInterrupt(2), onFall, FALLING);
  Serial.println("ready");
}

void loop() {
  static unsigned int last = 0;
  static int lastLevel = -1;
  noInterrupts();
  unsigned int p = presses;
  interrupts();
  if (p != last) {
    last = p;
    Serial.print("presses=");
    Serial.println(p);
  }
  int lv = digitalRead(2);
  if (lv != lastLevel) {
    lastLevel = lv;
    Serial.print("level=");
    Serial.println(lv);
  }
  delay(2);
}

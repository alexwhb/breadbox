// Prints a greeting at 9600 and 115200 baud, then a counter.
void setup() {
  Serial.begin(9600);
  Serial.println("Hello, Breadbox!");
  Serial.flush();
  Serial.end();
  Serial.begin(115200);
  Serial.println("Fast lane 115200");
}

int n = 0;
void loop() {
  Serial.print("n=");
  Serial.println(n++);
  delay(100);
}

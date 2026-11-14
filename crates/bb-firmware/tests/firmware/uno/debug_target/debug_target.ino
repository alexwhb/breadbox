// Debugger fixture: globals of several basic types, a helper function with locals and
// parameters, and a loop body with a well-known line for breakpoints.
volatile int counter = 0;
unsigned long total = 0;
float ratio = 0.5f;
bool flag = false;
char letter = 'A';
int history[4] = {0, 0, 0, 0};
const char *greeting = "hi";

int square_plus(int x, int bias) {
  int sq = x * x;
  int result = sq + bias;
  return result;
}

void setup() {
  Serial.begin(115200);
  pinMode(13, OUTPUT);
}

void loop() {
  int step = counter * 3;
  long big = (long)step * 1000L;
  counter++;
  int r = square_plus(step, 7);
  total += r;
  history[counter & 3] = step;
  flag = !flag;
  letter = 'A' + (counter % 26);
  ratio = ratio * 0.5f + step;
  digitalWrite(13, flag);
  if (big < 0) Serial.println(greeting);
}

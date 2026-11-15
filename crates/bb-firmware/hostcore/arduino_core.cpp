// arduino_core.cpp — time, GPIO, ADC, PWM/LEDC, interrupts, misc for host-emulated boards.
#include <unistd.h>

#include <vector>

#include "Arduino.h"
#include "bb_host.h"

EspClass ESP;

static uint8_t adc_bits = 12;
static uint8_t pwm_bits = 8;
static uint32_t pwm_freq = 1000;
static uint32_t pwm_range = 0;  // Pico analogWriteRange (0 = use bits)

struct Tone {
  uint8_t pin = 255;
  uint64_t end_us = 0;
};
static Tone tone_state;

static void check_tone() {
  if (tone_state.pin != 255 && tone_state.end_us && bb::now_us() >= tone_state.end_us) noTone(tone_state.pin);
}

unsigned long micros() {
  bb::Reply r = bb::rpc(bb::MSG_TIME);
  check_tone();
  return (unsigned long)r.now_us;
}

unsigned long millis() { return (unsigned long)(micros() / 1000ULL); }

void delay(unsigned long ms) {
  if (ms == 0) {
    yield();
    return;
  }
  bb::sleep_for((uint64_t)ms * 1000ULL);
  check_tone();
}

void delayMicroseconds(unsigned int us) {
  if (us) bb::sleep_for(us);
}

void yield() {
  bb::rpc(bb::MSG_YIELD);
  bb::service_async();
  check_tone();
}

void vTaskDelay(TickType_t ticks) { delay(ticks); }

void pinMode(uint8_t pin, uint8_t mode) { bb::send(bb::MSG_PIN_MODE, bb::Buf().u8(pin).u8(mode)); }

void digitalWrite(uint8_t pin, uint8_t val) { bb::send(bb::MSG_DIGITAL_WRITE, bb::Buf().u8(pin).u8(val ? 1 : 0)); }

int digitalRead(uint8_t pin) { return bb::rpc(bb::MSG_DIGITAL_READ, bb::Buf().u8(pin)).u8() ? HIGH : LOW; }

static double read_volts(uint8_t pin) { return bb::rpc(bb::MSG_ANALOG_READ, bb::Buf().u8(pin)).f64(); }

uint16_t analogRead(uint8_t pin) {
  double v = read_volts(pin);
  double full = (double)((1u << adc_bits) - 1);
  double x = v / 3.3 * full;
  if (x < 0) x = 0;
  if (x > full) x = full;
  return (uint16_t)lround(x);
}

uint32_t analogReadMilliVolts(uint8_t pin) {
  double v = read_volts(pin);
  if (v < 0) v = 0;
  if (v > 3.3) v = 3.3;
  return (uint32_t)lround(v * 1000.0);
}

void analogReadResolution(uint8_t bits) {
  if (bits >= 1 && bits <= 16) adc_bits = bits;
}
void analogSetAttenuation(int) {}
void analogSetPinAttenuation(uint8_t, int) {}

static void pwm(uint8_t pin, uint32_t duty, uint32_t full, uint32_t freq) {
  bb::send(bb::MSG_PWM, bb::Buf().u8(pin).u32(duty).u32(full).u32(freq));
}

void analogWrite(uint8_t pin, int value) {
  uint32_t full = pwm_range ? pwm_range : ((1u << pwm_bits) - 1);
  if (value < 0) value = 0;
  if ((uint32_t)value > full) value = (int)full;
  pwm(pin, (uint32_t)value, full, pwm_freq);
}
void analogWriteResolution(uint8_t bits) {
  if (bits >= 1 && bits <= 16) {
    pwm_bits = bits;
    pwm_range = 0;
  }
}
void analogWriteResolution(uint8_t, uint8_t bits) { analogWriteResolution(bits); }
void analogWriteFrequency(uint32_t f) { pwm_freq = f; }
void analogWriteFrequency(uint8_t, uint32_t f) { pwm_freq = f; }
void analogWriteFreq(uint32_t f) { pwm_freq = f; }
void analogWriteRange(uint32_t r) { pwm_range = r; }
void dacWrite(uint8_t pin, uint8_t value) { pwm(pin, value, 255, 0); }

// ------------------------------------------------------------------ LEDC
struct LedcChan {
  uint32_t freq = 5000;
  uint8_t bits = 8;
  uint32_t duty = 0;
  std::vector<uint8_t> pins;
};
static LedcChan ledc[16];
static int8_t pin_chan[64];  // pin → channel (3.x API), -1 none
static bool pin_chan_init = false;

static void init_pin_chan() {
  if (!pin_chan_init) {
    for (auto& p : pin_chan) p = -1;
    pin_chan_init = true;
  }
}

static void ledc_apply(uint8_t ch) {
  LedcChan& c = ledc[ch & 15];
  uint32_t full = (1u << c.bits) - 1;
  for (uint8_t p : c.pins) pwm(p, c.duty > full ? full : c.duty, full, c.freq);
}

uint32_t ledcSetup(uint8_t ch, uint32_t freq, uint8_t bits) {
  LedcChan& c = ledc[ch & 15];
  c.freq = freq;
  c.bits = bits ? (bits > 20 ? 20 : bits) : 8;
  return freq;
}
void ledcAttachPin(uint8_t pin, uint8_t ch) {
  ledc[ch & 15].pins.push_back(pin);
  ledc_apply(ch);
}
void ledcDetachPin(uint8_t pin) {
  for (auto& c : ledc) c.pins.erase(std::remove(c.pins.begin(), c.pins.end(), pin), c.pins.end());
  digitalWrite(pin, LOW);
}
bool ledcAttachChannel(uint8_t pin, uint32_t freq, uint8_t bits, uint8_t ch) {
  init_pin_chan();
  ledcSetup(ch, freq, bits);
  if (pin < 64) pin_chan[pin] = (int8_t)(ch & 15);
  ledcAttachPin(pin, ch);
  return true;
}
bool ledcAttach(uint8_t pin, uint32_t freq, uint8_t bits) {
  init_pin_chan();
  for (uint8_t ch = 0; ch < 16; ch++) {
    bool used = !ledc[ch].pins.empty();
    for (auto c : pin_chan)
      if (c == ch) used = true;
    if (!used) return ledcAttachChannel(pin, freq, bits, ch);
  }
  return false;
}
bool ledcDetach(uint8_t pin) {
  init_pin_chan();
  ledcDetachPin(pin);
  if (pin < 64) pin_chan[pin] = -1;
  return true;
}
static uint8_t resolve(uint8_t chan_or_pin) {
  init_pin_chan();
  if (chan_or_pin < 64 && pin_chan[chan_or_pin] >= 0) return (uint8_t)pin_chan[chan_or_pin];
  return chan_or_pin & 15;
}
void ledcWrite(uint8_t chan_or_pin, uint32_t duty) {
  uint8_t ch = resolve(chan_or_pin);
  ledc[ch].duty = duty;
  ledc_apply(ch);
}
uint32_t ledcRead(uint8_t chan_or_pin) { return ledc[resolve(chan_or_pin)].duty; }
uint32_t ledcWriteTone(uint8_t chan_or_pin, uint32_t freq) {
  uint8_t ch = resolve(chan_or_pin);
  ledc[ch].freq = freq;
  ledc[ch].duty = freq ? (1u << ledc[ch].bits) / 2 : 0;
  ledc_apply(ch);
  return freq;
}
uint32_t ledcReadFreq(uint8_t chan_or_pin) { return ledc[resolve(chan_or_pin)].freq; }

void tone(uint8_t pin, unsigned int frequency, unsigned long duration) {
  pwm(pin, frequency ? 1 : 0, 2, frequency);
  tone_state.pin = pin;
  tone_state.end_us = duration ? bb::now_us() + duration * 1000ULL : 0;
}

void noTone(uint8_t pin) {
  pwm(pin, 0, 2, 0);
  if (tone_state.pin == pin) tone_state = Tone();
}

// ------------------------------------------------------------------ interrupts
void attachInterrupt(uint8_t pin, void (*isr)(void), int mode) { bb::attach(pin, isr, nullptr, nullptr, mode); }
void attachInterruptArg(uint8_t pin, void (*isr)(void*), void* arg, int mode) { bb::attach(pin, nullptr, isr, arg, mode); }
void detachInterrupt(uint8_t pin) { bb::detach(pin); }

// ------------------------------------------------------------------ pulses, shifting
unsigned long pulseIn(uint8_t pin, uint8_t state, unsigned long timeout) {
  uint64_t t0 = micros();
  auto left = [&]() -> uint32_t {
    uint64_t el = bb::now_us() - t0;
    return el >= timeout ? 0 : (uint32_t)(timeout - el);
  };
  if (!bb::wait_pin(pin, !state, left())) return 0;
  if (!bb::wait_pin(pin, state, left())) return 0;
  uint64_t start = bb::now_us();
  if (!bb::wait_pin(pin, !state, left())) return 0;
  return (unsigned long)(bb::now_us() - start);
}

unsigned long pulseInLong(uint8_t pin, uint8_t state, unsigned long timeout) { return pulseIn(pin, state, timeout); }

void shiftOut(uint8_t dataPin, uint8_t clockPin, uint8_t bitOrder, uint8_t val) {
  for (uint8_t i = 0; i < 8; i++) {
    digitalWrite(dataPin, bitOrder == LSBFIRST ? (val >> i) & 1 : (val >> (7 - i)) & 1);
    digitalWrite(clockPin, HIGH);
    digitalWrite(clockPin, LOW);
  }
}

uint8_t shiftIn(uint8_t dataPin, uint8_t clockPin, uint8_t bitOrder) {
  uint8_t v = 0;
  for (uint8_t i = 0; i < 8; i++) {
    digitalWrite(clockPin, HIGH);
    if (bitOrder == LSBFIRST)
      v |= digitalRead(dataPin) << i;
    else
      v |= digitalRead(dataPin) << (7 - i);
    digitalWrite(clockPin, LOW);
  }
  return v;
}

// ------------------------------------------------------------------ random, math
static uint64_t rng_state = 0;
static uint32_t rng_next() {
  if (!rng_state) rng_state = bb::seed() * 0x9E3779B97F4A7C15ULL + 1;
  rng_state ^= rng_state >> 12;
  rng_state ^= rng_state << 25;
  rng_state ^= rng_state >> 27;
  return (uint32_t)((rng_state * 2685821657736338717ULL) >> 32);
}

uint32_t esp_random() { return rng_next(); }
void randomSeed(unsigned long seed) {
  if (seed) rng_state = seed * 0x9E3779B97F4A7C15ULL + 1;
}
long random(long howbig) { return howbig <= 0 ? 0 : (long)(rng_next() % (uint32_t)howbig); }
long random(long lo, long hi) { return lo >= hi ? lo : lo + random(hi - lo); }

long map(long x, long in_min, long in_max, long out_min, long out_max) {
  if (in_max == in_min) return out_min;
  return (x - in_min) * (out_max - out_min) / (in_max - in_min) + out_min;
}

uint16_t makeWord(uint16_t w) { return w; }
uint16_t makeWord(uint8_t h, uint8_t l) { return (uint16_t)(h << 8 | l); }

// ------------------------------------------------------------------ ESP
void EspClass::restart() {
  bb::send(bb::MSG_RESTART, bb::Buf());
  bb::rpc(bb::MSG_YIELD);
  _exit(0);
}
const char* EspClass::getChipModel() { return bb::board() == bb::BOARD_PICO ? "RP2040" : "ESP32-D0WDQ6"; }
uint32_t EspClass::getCpuFreqMHz() { return bb::board() == bb::BOARD_PICO ? 133 : 240; }
uint64_t EspClass::getEfuseMac() { return 0x10b0b07fcf5cULL; }
uint32_t EspClass::getCycleCount() { return (uint32_t)(micros() * getCpuFreqMHz()); }

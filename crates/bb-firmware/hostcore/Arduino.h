// Arduino.h — Breadbox's Arduino-compatible core for API-level boards (ESP32 DevKit, Pico).
// Sketches compile natively against this header and run in lock-step with simulated time.
#pragma once

#include <ctype.h>
#include <math.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <algorithm>
#include <cmath>
#include <functional>
#include <string>

#ifndef ARDUINO
#define ARDUINO 10819
#endif

typedef uint8_t byte;
typedef bool boolean;
typedef unsigned int word;

#define HIGH 0x1
#define LOW 0x0

#define INPUT 0x01
#define OUTPUT 0x03
#define PULLUP 0x04
#define INPUT_PULLUP 0x05
#define PULLDOWN 0x08
#define INPUT_PULLDOWN 0x09
#define OPEN_DRAIN 0x10
#define OUTPUT_OPEN_DRAIN 0x13
#define ANALOG 0xC0

#define RISING 0x01
#define FALLING 0x02
#define CHANGE 0x03
#define ONLOW 0x04
#define ONHIGH 0x05

#define LSBFIRST 0
#define MSBFIRST 1

#define PI 3.1415926535897932384626433832795
#define HALF_PI 1.5707963267948966192313216916398
#define TWO_PI 6.283185307179586476925286766559
#define DEG_TO_RAD 0.017453292519943295769236907684886
#define RAD_TO_DEG 57.295779513082320876798154814105
#define EULER 2.718281828459045235360287471352

#define SERIAL 0x0
#define DISPLAY 0x1

#define NOT_A_PIN 255
#define NOT_AN_INTERRUPT -1

template <class T, class L>
inline auto min(const T& a, const L& b) -> decltype((b < a) ? b : a) {
  return (b < a) ? b : a;
}
template <class T, class L>
inline auto max(const T& a, const L& b) -> decltype((b < a) ? b : a) {
  return (a < b) ? b : a;
}
#define _min(a, b) ((a) < (b) ? (a) : (b))
#define _max(a, b) ((a) > (b) ? (a) : (b))
#define constrain(amt, low, high) ((amt) < (low) ? (low) : ((amt) > (high) ? (high) : (amt)))
#define radians(deg) ((deg) * DEG_TO_RAD)
#define degrees(rad) ((rad) * RAD_TO_DEG)
#define sq(x) ((x) * (x))

#define lowByte(w) ((uint8_t)((w) & 0xff))
#define highByte(w) ((uint8_t)((w) >> 8))
#define bitRead(value, bit) (((value) >> (bit)) & 0x01)
#define bitSet(value, bit) ((value) |= (1UL << (bit)))
#define bitClear(value, bit) ((value) &= ~(1UL << (bit)))
#define bitToggle(value, bit) ((value) ^= (1UL << (bit)))
#define bitWrite(value, bit, bitvalue) ((bitvalue) ? bitSet(value, bit) : bitClear(value, bit))
#define bit(b) (1UL << (b))
#define _BV(b) (1UL << (b))

// Flash strings are ordinary strings on these boards.
class __FlashStringHelper;
#define F(string_literal) (reinterpret_cast<const __FlashStringHelper*>(string_literal))
#define FPSTR(p) (reinterpret_cast<const __FlashStringHelper*>(p))
#define PROGMEM
#define PGM_P const char*
#define PSTR(s) (s)
#define pgm_read_byte(addr) (*(const unsigned char*)(addr))
#define pgm_read_word(addr) (*(const unsigned short*)(addr))
#define pgm_read_dword(addr) (*(const unsigned long*)(addr))
#define pgm_read_float(addr) (*(const float*)(addr))
#define pgm_read_ptr(addr) (*(void* const*)(addr))
#define strcpy_P strcpy
#define strncpy_P strncpy
#define strcmp_P strcmp
#define strlen_P strlen
#define memcpy_P memcpy
#define sprintf_P sprintf
#define snprintf_P snprintf
#define printf_P printf

#define IRAM_ATTR
#define ARDUINO_ISR_ATTR
#define digitalPinToInterrupt(p) (p)
#define digitalPinToGPIONumber(p) (p)

#include "WString.h"
#include "Printable.h"
#include "Print.h"
#include "Stream.h"
#include "IPAddress.h"
#include "HardwareSerial.h"

// ------------------------------------------------------------------ board pins
#if defined(BB_HOST_PICO)
#define ARDUINO_ARCH_RP2040 1
static const uint8_t LED_BUILTIN = 25;
#define PIN_LED 25
static const uint8_t A0 = 26, A1 = 27, A2 = 28, A3 = 29;
static const uint8_t SDA = 4, SCL = 5;
static const uint8_t MISO = 16, SS = 17, SCK = 18, MOSI = 19;
static const uint8_t TX = 0, RX = 1;
#else
#ifndef ESP32
#define ESP32 1
#endif
#define ARDUINO_ARCH_ESP32 1
static const uint8_t LED_BUILTIN = 2;
#define BUILTIN_LED LED_BUILTIN
static const uint8_t A0 = 36, A3 = 39, A4 = 32, A5 = 33, A6 = 34, A7 = 35, A10 = 4, A11 = 0, A12 = 2, A13 = 15, A14 = 13,
                     A15 = 12, A16 = 14, A17 = 27, A18 = 25, A19 = 26;
static const uint8_t T0 = 4, T1 = 0, T2 = 2, T3 = 15, T4 = 13, T5 = 12, T6 = 14, T7 = 27, T8 = 33, T9 = 32;
static const uint8_t DAC1 = 25, DAC2 = 26;
static const uint8_t SDA = 21, SCL = 22;
static const uint8_t SS = 5, MOSI = 23, MISO = 19, SCK = 18;
static const uint8_t TX = 1, RX = 3;
#endif

// ------------------------------------------------------------------ core API
unsigned long millis(void);
unsigned long micros(void);
void delay(unsigned long ms);
void delayMicroseconds(unsigned int us);
void yield(void);

void pinMode(uint8_t pin, uint8_t mode);
void digitalWrite(uint8_t pin, uint8_t val);
int digitalRead(uint8_t pin);

uint16_t analogRead(uint8_t pin);
uint32_t analogReadMilliVolts(uint8_t pin);
void analogReadResolution(uint8_t bits);
void analogSetAttenuation(int attenuation);
void analogSetPinAttenuation(uint8_t pin, int attenuation);
void analogWrite(uint8_t pin, int value);
void analogWriteResolution(uint8_t bits);
void analogWriteResolution(uint8_t pin, uint8_t bits);
void analogWriteFrequency(uint32_t freq);
void analogWriteFrequency(uint8_t pin, uint32_t freq);
void analogWriteFreq(uint32_t freq);
void analogWriteRange(uint32_t range);
void dacWrite(uint8_t pin, uint8_t value);

void attachInterrupt(uint8_t pin, void (*isr)(void), int mode);
void attachInterruptArg(uint8_t pin, void (*isr)(void*), void* arg, int mode);
void detachInterrupt(uint8_t pin);
inline void interrupts() {}
inline void noInterrupts() {}

unsigned long pulseIn(uint8_t pin, uint8_t state, unsigned long timeout = 1000000L);
unsigned long pulseInLong(uint8_t pin, uint8_t state, unsigned long timeout = 1000000L);
void shiftOut(uint8_t dataPin, uint8_t clockPin, uint8_t bitOrder, uint8_t val);
uint8_t shiftIn(uint8_t dataPin, uint8_t clockPin, uint8_t bitOrder);
void tone(uint8_t pin, unsigned int frequency, unsigned long duration = 0);
void noTone(uint8_t pin);

long random(long howbig);
long random(long howsmall, long howbig);
void randomSeed(unsigned long seed);
uint32_t esp_random(void);
long map(long x, long in_min, long in_max, long out_min, long out_max);
uint16_t makeWord(uint16_t w);
uint16_t makeWord(uint8_t h, uint8_t l);

// ESP32 LEDC (core 2.x channel API and core 3.x pin API).
uint32_t ledcSetup(uint8_t channel, uint32_t freq, uint8_t resolution_bits);
void ledcAttachPin(uint8_t pin, uint8_t channel);
void ledcDetachPin(uint8_t pin);
bool ledcAttach(uint8_t pin, uint32_t freq, uint8_t resolution);
bool ledcAttachChannel(uint8_t pin, uint32_t freq, uint8_t resolution, uint8_t channel);
bool ledcDetach(uint8_t pin);
void ledcWrite(uint8_t chan_or_pin, uint32_t duty);
uint32_t ledcRead(uint8_t chan_or_pin);
uint32_t ledcWriteTone(uint8_t chan_or_pin, uint32_t freq);
uint32_t ledcReadFreq(uint8_t chan_or_pin);

// FreeRTOS-ish shims used by ESP32 sketches.
typedef uint32_t TickType_t;
#define portTICK_PERIOD_MS 1
#define pdMS_TO_TICKS(ms) ((TickType_t)(ms))
#define portMAX_DELAY 0xffffffffUL
void vTaskDelay(TickType_t ticks);

// SNTP (ESP32 core): configTime + getLocalTime use real NTP packets to the sim's server.
void configTime(long gmtOffset_sec, int daylightOffset_sec, const char* server1, const char* server2 = nullptr,
                const char* server3 = nullptr);
void configTzTime(const char* tz, const char* server1, const char* server2 = nullptr, const char* server3 = nullptr);
bool getLocalTime(struct tm* info, uint32_t ms = 5000);

class EspClass {
 public:
  void restart();
  uint32_t getFreeHeap() { return 248 * 1024; }
  uint32_t getHeapSize() { return 320 * 1024; }
  uint32_t getMinFreeHeap() { return 200 * 1024; }
  const char* getChipModel();
  uint8_t getChipRevision() { return 3; }
  uint8_t getChipCores() { return 2; }
  uint32_t getCpuFreqMHz();
  uint64_t getEfuseMac();
  const char* getSdkVersion() { return "breadbox-host-1"; }
  uint32_t getFlashChipSize() { return 4 * 1024 * 1024; }
  uint32_t getCycleCount();
};
extern EspClass ESP;
#define rp2040 ESP

void setup(void);
void loop(void);

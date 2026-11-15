// HTTPClient.h — subset of the ESP32 HTTPClient (HTTP/1.1, Content-Length bodies).
#pragma once

#include <vector>

#include "Arduino.h"
#include "WiFi.h"

#define HTTPC_ERROR_CONNECTION_REFUSED (-1)
#define HTTPC_ERROR_SEND_HEADER_FAILED (-2)
#define HTTPC_ERROR_SEND_PAYLOAD_FAILED (-3)
#define HTTPC_ERROR_NOT_CONNECTED (-4)
#define HTTPC_ERROR_CONNECTION_LOST (-5)
#define HTTPC_ERROR_NO_STREAM (-6)
#define HTTPC_ERROR_NO_HTTP_SERVER (-7)
#define HTTPC_ERROR_READ_TIMEOUT (-11)

enum t_http_codes {
  HTTP_CODE_OK = 200,
  HTTP_CODE_CREATED = 201,
  HTTP_CODE_NO_CONTENT = 204,
  HTTP_CODE_MOVED_PERMANENTLY = 301,
  HTTP_CODE_FOUND = 302,
  HTTP_CODE_BAD_REQUEST = 400,
  HTTP_CODE_UNAUTHORIZED = 401,
  HTTP_CODE_FORBIDDEN = 403,
  HTTP_CODE_NOT_FOUND = 404,
  HTTP_CODE_INTERNAL_SERVER_ERROR = 500,
};

class HTTPClient {
 public:
  bool begin(const String& url);
  bool begin(WiFiClient& client, const String& url);
  bool begin(const String& host, uint16_t port, const String& uri = "/");
  bool begin(WiFiClient& client, const String& host, uint16_t port, const String& uri = "/");
  void end();
  void addHeader(const String& name, const String& value);
  void setTimeout(uint16_t ms) { timeout_ms_ = ms; }
  void setConnectTimeout(int32_t ms) { connect_timeout_ms_ = ms; }
  void setUserAgent(const String& ua) { user_agent_ = ua; }
  void setReuse(bool) {}
  int GET() { return sendRequest("GET"); }
  int POST(const String& payload) { return sendRequest("POST", (const uint8_t*)payload.c_str(), payload.length()); }
  int POST(const uint8_t* payload, size_t size) { return sendRequest("POST", payload, size); }
  int PUT(const String& payload) { return sendRequest("PUT", (const uint8_t*)payload.c_str(), payload.length()); }
  int PATCH(const String& payload) { return sendRequest("PATCH", (const uint8_t*)payload.c_str(), payload.length()); }
  int sendRequest(const char* method, const uint8_t* payload = nullptr, size_t size = 0);
  int sendRequest(const char* method, const String& payload) {
    return sendRequest(method, (const uint8_t*)payload.c_str(), payload.length());
  }
  String getString() { return body_; }
  int getSize() { return (int)body_.length(); }
  String header(const char* name);
  bool connected() { return client_ && client_->connected(); }
  WiFiClient& getStream() { return *client_; }
  static String errorToString(int error);

 private:
  WiFiClient own_;
  WiFiClient* client_ = nullptr;
  String host_;
  uint16_t port_ = 80;
  String uri_ = "/";
  std::vector<std::pair<String, String>> headers_;
  std::vector<std::pair<String, String>> resp_headers_;
  String body_;
  String user_agent_ = "ESP32HTTPClient";
  uint16_t timeout_ms_ = 5000;
  int32_t connect_timeout_ms_ = 5000;
};

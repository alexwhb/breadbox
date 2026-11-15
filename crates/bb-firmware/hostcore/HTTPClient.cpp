#include "HTTPClient.h"

static bool parse_url(const String& url, String& host, uint16_t& port, String& uri) {
  String u = url;
  if (u.startsWith("https://")) return false;  // no TLS in the sim
  if (u.startsWith("http://")) u = u.substring(7);
  int slash = u.indexOf('/');
  String hp = slash < 0 ? u : u.substring(0, (unsigned)slash);
  uri = slash < 0 ? String("/") : u.substring((unsigned)slash);
  int colon = hp.indexOf(':');
  if (colon >= 0) {
    host = hp.substring(0, (unsigned)colon);
    port = (uint16_t)hp.substring((unsigned)colon + 1).toInt();
  } else {
    host = hp;
    port = 80;
  }
  return host.length() > 0;
}

bool HTTPClient::begin(const String& url) { return begin(own_, url); }

bool HTTPClient::begin(WiFiClient& client, const String& url) {
  client_ = &client;
  headers_.clear();
  return parse_url(url, host_, port_, uri_);
}

bool HTTPClient::begin(const String& host, uint16_t port, const String& uri) { return begin(own_, host, port, uri); }

bool HTTPClient::begin(WiFiClient& client, const String& host, uint16_t port, const String& uri) {
  client_ = &client;
  host_ = host;
  port_ = port;
  uri_ = uri;
  headers_.clear();
  return true;
}

void HTTPClient::end() {
  if (client_) client_->stop();
}

void HTTPClient::addHeader(const String& name, const String& value) { headers_.push_back({name, value}); }

String HTTPClient::header(const char* name) {
  for (auto& h : resp_headers_)
    if (h.first.equalsIgnoreCase(name)) return h.second;
  return String();
}

int HTTPClient::sendRequest(const char* method, const uint8_t* payload, size_t size) {
  if (!client_) return HTTPC_ERROR_NOT_CONNECTED;
  body_ = "";
  resp_headers_.clear();
  if (!client_->connected() && !client_->connect(host_.c_str(), port_, connect_timeout_ms_)) return HTTPC_ERROR_CONNECTION_REFUSED;
  String req = String(method) + " " + uri_ + " HTTP/1.1\r\nHost: " + host_;
  if (port_ != 80) req += ":" + String(port_);
  req += "\r\nUser-Agent: " + user_agent_ + "\r\nConnection: close\r\n";
  for (auto& h : headers_) req += h.first + ": " + h.second + "\r\n";
  if (payload || strcmp(method, "POST") == 0 || strcmp(method, "PUT") == 0) req += "Content-Length: " + String((unsigned long)size) + "\r\n";
  req += "\r\n";
  if (client_->write((const uint8_t*)req.c_str(), req.length()) != req.length()) return HTTPC_ERROR_SEND_HEADER_FAILED;
  if (size && client_->write(payload, size) != size) return HTTPC_ERROR_SEND_PAYLOAD_FAILED;
  // Status line + headers.
  String line;
  int code = 0;
  long content_length = -1;
  bool in_headers = true;
  unsigned long t0 = millis();
  while (in_headers) {
    if (millis() - t0 > timeout_ms_) return HTTPC_ERROR_READ_TIMEOUT;
    int c = client_->read();
    if (c < 0) {
      if (!client_->connected()) return code ? code : HTTPC_ERROR_CONNECTION_LOST;
      delay(1);
      continue;
    }
    if (c == '\n') {
      line.trim();
      if (code == 0) {
        if (!line.startsWith("HTTP/")) return HTTPC_ERROR_NO_HTTP_SERVER;
        code = (int)line.substring((unsigned)line.indexOf(' ') + 1).toInt();
      } else if (line.length() == 0) {
        in_headers = false;
      } else {
        int colon = line.indexOf(':');
        if (colon > 0) {
          String k = line.substring(0, (unsigned)colon), v = line.substring((unsigned)colon + 1);
          v.trim();
          if (k.equalsIgnoreCase("Content-Length")) content_length = v.toInt();
          resp_headers_.push_back({k, v});
        }
      }
      line = "";
    } else {
      line += (char)c;
    }
  }
  // Body.
  t0 = millis();
  while (content_length < 0 || (long)body_.length() < content_length) {
    if (millis() - t0 > timeout_ms_) break;
    uint8_t buf[512];
    int n = client_->available() ? client_->read(buf, sizeof buf) : -1;
    if (n > 0) {
      body_.concat((const char*)buf, (unsigned)n);
      t0 = millis();
    } else if (!client_->connected()) {
      break;
    } else {
      delay(1);
    }
  }
  return code;
}

String HTTPClient::errorToString(int error) {
  switch (error) {
    case HTTPC_ERROR_CONNECTION_REFUSED:
      return "connection refused";
    case HTTPC_ERROR_SEND_HEADER_FAILED:
      return "send header failed";
    case HTTPC_ERROR_SEND_PAYLOAD_FAILED:
      return "send payload failed";
    case HTTPC_ERROR_NOT_CONNECTED:
      return "not connected";
    case HTTPC_ERROR_CONNECTION_LOST:
      return "connection lost";
    case HTTPC_ERROR_NO_HTTP_SERVER:
      return "no HTTP server";
    case HTTPC_ERROR_READ_TIMEOUT:
      return "read Timeout";
    default:
      return String();
  }
}

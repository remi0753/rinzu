#ifndef RINZU_HTTP_H
#define RINZU_HTTP_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
enum { RINZU_HTTP_CHUNK = 65536 };
enum RinzuHttpStatus {
  RINZU_HTTP_OWNER = -7301,
  RINZU_HTTP_CLOSED = -7302,
  RINZU_HTTP_BUSY = -7303,
  RINZU_HTTP_ARGUMENT = -7304,
  RINZU_HTTP_SATURATED = -7305,
  RINZU_HTTP_BOUND = -7306,
  RINZU_HTTP_BACKEND = -7307
};
void *rinzu_http_new(void);
bool rinzu_http_owner_has_client(void);
void *rinzu_http_owner_client(void);
bool rinzu_http_busy(void *client);
int32_t rinzu_http_ready(void *client);
int32_t rinzu_http_request(void *client, const char *url, const uint8_t *body, size_t length,
                           bool post, int64_t timeout);
int32_t rinzu_http_close(void *client);
void *rinzu_http_completion(void *client);
int32_t rinzu_http_status(void *client);
int64_t rinzu_http_response_status(void *client);
int64_t rinzu_http_length(void *client);
int64_t rinzu_http_sent(void *client);
const uint8_t *rinzu_http_bytes(void *client);
void rinzu_http_cancel(void *client);
void rinzu_http_finish(void *client);
int64_t rinzu_http_live_clients(void);
int64_t rinzu_http_live_buffers(void);
int64_t rinzu_http_retained_controls(void);
#endif

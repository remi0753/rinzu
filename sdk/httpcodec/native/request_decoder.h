#ifndef RINZU_HTTPCODEC_REQUEST_DECODER_H
#define RINZU_HTTPCODEC_REQUEST_DECODER_H

#include "rinzu/native_bytes.h"
#include <stdbool.h>

typedef struct RzHttpFrame {
  int64_t headOffset, methodEnd, targetEnd, fieldCount;
  int64_t bodyOffset, bodyLength;
  int64_t headVersion;
  bool closing;
} RzHttpFrame;

typedef struct RzHttpFieldSpan {
  int64_t nameStart, nameEnd, valueStart, valueEnd;
} RzHttpFieldSpan;

void *rinzu_httpcodec_decoder(int64_t cache_limit);
void rinzu_httpcodec_close(void *decoder);
int64_t rinzu_httpcodec_feed(void *decoder, const uint8_t *data, size_t length, int64_t start,
                             int64_t owned_head);
int64_t rinzu_httpcodec_feed_read(void *decoder, void *access, void *view, int64_t start,
                                  int64_t owned_head);
int64_t rinzu_httpcodec_end(void *decoder);
const RzHttpFrame *rinzu_httpcodec_frame(void *decoder);
const RzHttpFieldSpan *rinzu_httpcodec_field(void *decoder, int64_t index);
const char *rinzu_httpcodec_read_text(void *access, void *view, int64_t start, int64_t end,
                                      bool lower);
const char *rinzu_httpcodec_stored_text(void *decoder, int64_t start, int64_t end, bool lower);
const rinzu_native_byte_array_t *rinzu_httpcodec_stored_body(void *decoder);

#endif

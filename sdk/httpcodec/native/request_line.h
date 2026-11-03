#ifndef RINZU_HTTPCODEC_REQUEST_LINE_H
#define RINZU_HTTPCODEC_REQUEST_LINE_H
#include <stddef.h>
#include <stdint.h>
/* Synchronous, no retained input or allocation. Positive method-end offset
   on success; -400 / -414 preserve the prototype wire parser's precedence. */
int64_t rinzu_httpcodec_request_line_end(const uint8_t *data, size_t length);
#endif

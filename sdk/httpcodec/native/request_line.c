#include "request_line.h"
#include <string.h>

static int token(uint8_t byte) {
  if ((byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') ||
      (byte >= 'a' && byte <= 'z'))
    return 1;
  switch (byte) {
  case '!': case '#': case '$': case '%': case '&': case '\'': case '*':
  case '+': case '-': case '.': case '^': case '_': case '`': case '|': case '~':
    return 1;
  default:
    return 0;
  }
}

int64_t rinzu_httpcodec_request_line_end(const uint8_t *data, size_t length) {
  if (!data || !length || length > INT64_MAX)
    return -400;
  const uint8_t *space = memchr(data, ' ', length);
  if (!space || space == data)
    return -400;
  size_t first = (size_t)(space - data);
  for (size_t i = 0; i < first; ++i)
    if (!token(data[i]))
      return -400;
  size_t start = first + 1;
  if (start >= length)
    return -400;
  space = memchr(data + start, ' ', length - start);
  if (!space || space == data + start)
    return -400;
  size_t second = (size_t)(space - data), target_length = second - start;
  if (target_length > 4096)
    return -414;
  if (data[start] != '/')
    return -400;
  for (size_t i = start; i < second; ++i)
    if (data[i] < 33 || data[i] > 126 || data[i] == '#')
      return -400;
  if (length - second - 1 != 8 || memcmp(data + second + 1, "HTTP/1.1", 8) != 0)
    return -400;
  return (int64_t)first;
}

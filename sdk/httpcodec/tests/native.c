#include "request_line.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(int argc, char **argv) {
  assert(argc == 2);
  assert(rinzu_httpcodec_request_line_end(NULL, 0) == -400);
  assert(rinzu_httpcodec_request_line_end(NULL, 12) == -400);
  FILE *file = fopen(argv[1], "rb");
  assert(file);
  uint8_t header[12], bytes[16384];
  size_t count = 0, got;
  while ((got = fread(header, 1, sizeof header, file)) != 0) {
    assert(got == sizeof header);
    uint64_t bits = 0;
    uint32_t length = 0;
    for (unsigned i = 0; i < 8; ++i)
      bits |= (uint64_t)header[i] << (8 * i);
    for (unsigned i = 0; i < 4; ++i)
      length |= (uint32_t)header[i + 8] << (8 * i);
    int64_t expected;
    memcpy(&expected, &bits, sizeof expected);
    assert(length <= sizeof bytes && fread(bytes, 1, length, file) == length);
    int64_t actual = rinzu_httpcodec_request_line_end(bytes, length);
    if (actual != expected) {
      fprintf(stderr, "case=%zu length=%u expected=%lld actual=%lld\n", count, length,
              (long long)expected, (long long)actual);
      abort();
    }
    count++;
  }
  assert(feof(file) && !ferror(file) && !fclose(file));
  printf("request-line differential: %zu passed\n", count);
}

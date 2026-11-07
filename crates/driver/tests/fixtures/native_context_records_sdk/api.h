#ifndef RINZU_CONTEXT_RECORD_FIXTURE_H
#define RINZU_CONTEXT_RECORD_FIXTURE_H
#include <stdint.h>
struct Packet {
  int64_t x;
};
void *context_make(void);
int64_t context_check(void *);
int64_t context_change(const struct Packet *, int64_t);
int64_t context_invoke(void *, int64_t (*)(void *));
#endif

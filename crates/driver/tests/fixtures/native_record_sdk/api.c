#include "api.h"
#include <assert.h>
#include <stddef.h>
#include <string.h>
#include <pthread.h>
#include <sys/mman.h>
static _Thread_local struct Packet external;
static pthread_once_t once = PTHREAD_ONCE_INIT;
static void *unreadable;
static void private_page(void) {
  unreadable = mmap(NULL, 16384, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
  assert(unreadable != MAP_FAILED);
}
static void initialize(unsigned char padding) {
  memset(&external, padding, sizeof(external));
  external.a = INT8_MIN;
  external.b = INT16_MIN;
  external.c = INT32_MIN;
  external.d = INT64_MIN;
  external.e = UINT8_MAX;
  external.f = UINT16_MAX;
  external.g = UINT32_MAX;
  external.h = UINT64_MAX;
  uint32_t f = UINT32_C(0x80000000);
  memcpy(&external.i, &f, 4);
  uint64_t d = UINT64_C(0x7ff800000000002a);
  memcpy(&external.j, &d, 8);
  external.k = 1;
}
static int fields_valid(const struct Packet *value, int nan) {
  if (value->a != INT8_MIN || value->b != INT16_MIN || value->c != INT32_MIN ||
      value->d != INT64_MIN || value->e != UINT8_MAX || value->f != UINT16_MAX ||
      value->g != UINT32_MAX || value->h != UINT64_MAX || !value->k)
    return 0;
  uint32_t f = 0;
  memcpy(&f, &value->i, 4);
  uint64_t d = 0;
  memcpy(&d, &value->j, 8);
  return f == UINT32_C(0x80000000) &&
         d == (nan ? UINT64_C(0x7ff800000000002a) : UINT64_C(0x3ff0000000000000));
}
int32_t record_check(const struct Packet *value, _Bool nan) {
  if (!value || !fields_valid(value, nan))
    return 0;
  /* Compare against an independently initialized C struct to verify zero padding. */
  struct Packet expected;
  memset(&expected, 0, sizeof(expected));
  expected.a = value->a;
  expected.b = value->b;
  expected.c = value->c;
  expected.d = value->d;
  expected.e = value->e;
  expected.f = value->f;
  expected.g = value->g;
  expected.h = value->h;
  memcpy(&expected.i, &value->i, 4);
  memcpy(&expected.j, &value->j, 8);
  expected.k = value->k;
  return memcmp(&expected, value, sizeof(expected)) == 0;
}
const struct Packet *record_result(int32_t kind) {
  pthread_once(&once, private_page);
  if (kind == 0)
    return NULL;
  if (kind == 3)
    return unreadable;
  initialize(kind == 4 ? 0x5a : 0xa5);
  if (kind == 2) {
    unsigned char invalid = 2;
    memcpy((unsigned char *)&external + offsetof(struct Packet, k), &invalid, 1);
  }
  return &external;
}
const struct Packet *record_callback(const struct Packet *value,
                                     uint64_t (*callback)(const uint8_t *)) {
  assert(record_check(value, 0));
  static const uint8_t bytes[] = {0, 255, 7, 0};
  for (size_t n = 0; n < 32; ++n)
    assert(callback(bytes) == 4);
  assert(record_check(value, 0));
  initialize(0x3c);
  return &external;
}
void record_mutate(void) {
  memset(&external, 0, sizeof(external));
}
void record_fixture_cleanup(void) {
  if (unreadable && unreadable != MAP_FAILED) {
    assert(!munmap(unreadable, 16384));
    unreadable = NULL;
  }
}

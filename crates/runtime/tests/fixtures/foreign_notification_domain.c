#include <assert.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static unsigned domain;
static void observed_rollback(void);

static void event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  printf("%u,%s,%" PRIu64 ",%" PRId64 ",%" PRId64 "\n", domain, kind, id, a, b);
  if (domain == 1 && !strcmp(kind, "register_drop"))
    observed_rollback();
}

#define RINZU_NATIVE_LIFETIME_EVENT(kind, id, a, b) event(kind, id, a, b)
#include "rinzu/source_async.h"

static rinzu_async_runtime_t runtime[2];
static rinzu_native_completion_t *registered_carrier[2];

static void observed_rollback(void) {
  for (unsigned d = 0; d < 2; ++d) {
    rinzu_async_native_wake_t *target = runtime[d].native_wake;
    rinzu_native_completion_v2_t *carrier = (rinzu_native_completion_v2_t *)registered_carrier[d];
    assert(atomic_load(&carrier->notification) == (uintptr_t)&target->header);
    assert(atomic_load(&target->references) == 2);
  }
  puts("foreign registration returned only the candidate");
}

static void admit(rinzu_native_completion_t *carrier, unsigned kind) {
  rinzu_async_future_t *future =
      rinzu_async_new(&runtime[domain], RINZU_ASYNC_COMPLETION, kind, (rinzu_source_position_t){0});
  future->native_completion = carrier;
  rinzu_async_start(future);
}

int main(int argc, char **argv) {
  assert(argc == 2 && (argv[1][0] == '2' || argv[1][0] == '3'));
  setvbuf(stdout, NULL, _IOLBF, 0);
  rinzu_native_completion_v2_t unit;
  rinzu_native_completion_v3_t scalar;
  rinzu_native_completion_v2_t local;
  rinzu_native_completion_v2_init(&unit);
  rinzu_native_completion_v2_init(&local);
  rinzu_native_completion_v2_init(&scalar.completion);
  scalar.completion.base.version = 3;
  scalar.completion.base.size = sizeof(scalar);
  scalar.result = 7183;
  int transfer = argv[1][0] == '3';
  registered_carrier[0] = transfer ? &scalar.completion.base : &unit.base;
  registered_carrier[1] = &local.base;
  unsigned kind = transfer ? RINZU_SCALAR_I64 : RINZU_TRANSFER_UNIT;
  rinzu_async_init(&runtime[0]);
  rinzu_async_init(&runtime[1]);
  admit(registered_carrier[0], kind);
  domain = 1;
  admit(registered_carrier[1], RINZU_TRANSFER_UNIT);
  assert(runtime[0].native_wake != runtime[1].native_wake);
  admit(registered_carrier[0], kind);
  return 99;
}

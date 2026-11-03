#include <stdlib.h>
#include <assert.h>
#include <stdio.h>
static _Thread_local long budget = -1;
static _Thread_local size_t allocations;
static void *controlled_calloc(size_t count, size_t size) {
  if (!budget)
    return NULL;
  if (budget > 0)
    budget--;
  void *pointer = calloc(count, size);
  if (pointer)
    allocations++;
  return pointer;
}
static inline void *controlled_malloc(size_t size) {
  if (!budget)
    return NULL;
  if (budget > 0)
    budget--;
  void *pointer = malloc(size);
  if (pointer)
    allocations++;
  return pointer;
}
static void controlled_free(void *pointer) {
  if (pointer) {
    assert(allocations);
    allocations--;
  }
  free(pointer);
}
#define calloc controlled_calloc
#define malloc controlled_malloc
#define free controlled_free
#include "rinzu/native_userdata.h"
#undef calloc
#undef malloc
#undef free

static void configure(rinzu_managed_data_t *data, rinzu_value_schema_t *schema) {
  static const size_t fields[] = {0, 1}, event[] = {2}, other[] = {5};
  static const rinzu_key_variant_t variants[] = {{0, NULL}, {1, event}};
  static const rinzu_value_type_t types[] = {
      {.kind = RINZU_VALUE_STRING},
      {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64},
      {.kind = RINZU_VALUE_RECORD, .identity = "State", .fields = fields, .field_count = 2},
      {.kind = RINZU_VALUE_ENUM, .identity = "Event", .variants = variants, .variant_count = 2},
      {.kind = RINZU_VALUE_RECORD, .identity = "Different", .fields = fields, .field_count = 2},
      {.kind = RINZU_VALUE_OTHER},
      {.kind = RINZU_VALUE_RECORD, .identity = "Context", .fields = other, .field_count = 1}};
  assert(!rinzu_value_schema_init(schema, types, 7));
  assert(!rinzu_managed_configure_values(data, schema));
}
static uint64_t record(rinzu_managed_data_t *data, const char *name, size_t length, uint64_t number,
                       size_t type) {
  uint64_t text = rinzu_managed_string(data, (const unsigned char *)name, length), value = 0;
  rinzu_managed_slot_t slots[] = {{text, 1}, {number, 0}};
  assert(!rinzu_managed_composite(data, type, -1, slots, 2, &value));
  return value;
}
static void verify(rinzu_managed_data_t *data, uint64_t value, const char *name, size_t length,
                   uint64_t number) {
  rinzu_managed_slot_t field;
  assert(!rinzu_managed_project(data, value, 2, -1, 0, &field) && field.reference);
  rinzu_managed_value_t *text = rinzu_managed_find(data, field.bits);
  assert(text->length == length && !memcmp(text->bytes, name, length));
  assert(!rinzu_managed_project(data, value, 2, -1, 1, &field) && !field.reference &&
         field.bits == number);
}
static void run(void) {
  size_t before = allocations;
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t roots = {0};
  rinzu_value_schema_t schema = {0};
  rinzu_userdata_registry_t registry = {0};
  configure(&data, &schema);
  uint64_t first = record(&data, "first🙂", 9, 41, 2),
           second = record(&data, "second\0value", 12, 42, 2),
           other = record(&data, "typed", 5, 43, 4), event = 0;
  rinzu_managed_slot_t child = {first, 1};
  assert(!rinzu_managed_composite(&data, 3, 1, &child, 1, &event));
  uint64_t scope = rinzu_userdata_enter(&registry, &data);
  const void *one = rinzu_userdata_prepare(&registry, &data, scope, 2, first);
  const void *duplicate = rinzu_userdata_prepare(&registry, &data, scope, 2, first);
  const void *two = rinzu_userdata_prepare(&registry, &data, scope, 2, second);
  const void *typed = rinzu_userdata_prepare(&registry, &data, scope, 4, other);
  const void *enumeration = rinzu_userdata_prepare(&registry, &data, scope, 3, event);
  assert(!schema.equality[6]);
  rinzu_managed_slot_t opaque = {42, 0};
  uint64_t context = 0;
  assert(!rinzu_managed_composite(&data, 6, -1, &opaque, 1, &context));
  const void *held = rinzu_userdata_prepare(&registry, &data, scope, 6, context);
  rinzu_userdata_process_stats_t active = rinzu_userdata_process_stats();
  assert(active.tokens >= 6 && !active.closed && active.token_bytes > 0);
  assert(one != duplicate && one != two && two != typed && (uintptr_t)one != first);
  rinzu_userdata_open(&registry, &data, scope);
  rinzu_userdata_invocation_t admitted = rinzu_userdata_admit(&registry, &data, scope),
                              nested = rinzu_userdata_admit(&registry, &data, scope);
  uint64_t output = 0;
  for (size_t n = 0; n < 32; n++) {
    rinzu_managed_collect(&data, &roots);
    assert(!rinzu_userdata_resolve(&registry, &data, admitted, 6, held, &output) && output == context);
    rinzu_managed_slot_t field = {0};
    assert(!rinzu_managed_project(&data, context, 6, -1, 0, &field) && !field.reference && field.bits == 42);
    assert(!rinzu_userdata_resolve(&registry, &data, admitted, 2, two, &output));
    verify(&data, output, "second\0value", 12, 42);
    assert(!rinzu_userdata_resolve(&registry, &data, nested, 2, one, &output));
    verify(&data, output, "first🙂", 9, 41);
    assert(!rinzu_userdata_resolve(&registry, &data, admitted, 4, typed, &output) &&
           output == other);
    assert(!rinzu_userdata_resolve(&registry, &data, admitted, 3, enumeration, &output) &&
           output == event);
  }
  budget = 0;
  output = UINT64_MAX;
  assert(rinzu_userdata_resolve(&registry, &data, admitted, 2, NULL, &output) ==
             RINZU_USERDATA_NULL &&
         output == UINT64_MAX);
  assert(!rinzu_userdata_resolve(&registry, &data, admitted, 2, two, &output) && output == second);
  budget = -1;
  rinzu_userdata_returned(&registry, &data, scope);
  assert(registry.in_flight == 2 && registry.roots == 6);
  rinzu_managed_collect(&data, &roots);
  assert(!rinzu_userdata_resolve(&registry, &data, admitted, 2, one, &output) && output == first);
  rinzu_userdata_settle(&registry, &data, nested);
  assert(registry.roots == 6);
  budget = 0;
  rinzu_userdata_settle(&registry, &data, admitted);
  budget = -1;
  rinzu_userdata_finish(&registry, &data);
  rinzu_managed_enter(&roots);
  uint64_t keep = record(&data, "growing", 7, 99, 2);
  rinzu_managed_publish(&roots, &keep, 1);
  const void **addresses = malloc(2049 * sizeof(*addresses));
  assert(addresses);
  for (size_t n = 0; n < 2049; n++) {
    uint64_t next = rinzu_userdata_enter(&registry, &data);
    const void *token = rinzu_userdata_prepare(&registry, &data, next, 2, keep);
    assert(token != one);
    for (size_t prior = 0; prior < n; prior++)
      assert(addresses[prior] != token);
    addresses[n] = token;
    rinzu_userdata_open(&registry, &data, next);
    rinzu_userdata_invocation_t invocation = rinzu_userdata_admit(&registry, &data, next);
    assert(!rinzu_userdata_resolve(&registry, &data, invocation, 2, token, &output) &&
           output == keep);
    rinzu_userdata_settle(&registry, &data, invocation);
    rinzu_userdata_returned(&registry, &data, next);
  }
  free(addresses);
  uint64_t prepared = rinzu_userdata_enter(&registry, &data);
  assert(rinzu_userdata_prepare(&registry, &data, prepared, 2, keep) != one);
  rinzu_userdata_discard_prepared(&registry, &data, prepared);
  rinzu_userdata_finish(&registry, &data);
  assert(registry.created == 2056 && registry.released == 2056);
  for (unsigned mode = 0; mode < 3; ++mode) {
    uint64_t scope = rinzu_userdata_enter(&registry, &data);
    rinzu_userdata_expect_result(&registry, &data, scope, 2);
    (void)rinzu_userdata_prepare(&registry, &data, scope, 2, first);
    const void *two = rinzu_userdata_prepare(&registry, &data, scope, 2, second);
    rinzu_managed_publish(&roots, NULL, 0);
    rinzu_userdata_open(&registry, &data, scope);
    rinzu_userdata_invocation_t invocation = rinzu_userdata_admit(&registry, &data, scope);
    rinzu_userdata_returned(&registry, &data, scope);
    rinzu_userdata_settle(&registry, &data, invocation);
    assert(registry.roots == 2);
    rinzu_managed_collect(&data, &roots);
    uint64_t output = 0;
    if (mode == 0) {
      assert(!rinzu_userdata_receive_result(&registry, &data, scope, 2, two, &output) && output == second);
      verify(&data, output, "second\0value", 12, 42);
    } else if (mode == 1)
      assert(rinzu_userdata_receive_result(&registry, &data, scope, 2, NULL, &output) == RINZU_USERDATA_NULL);
    uint64_t live[] = {first, second};
    rinzu_managed_publish(&roots, live, 2);
    rinzu_userdata_finish_result(&registry, &data, scope);
    rinzu_userdata_finish(&registry, &data);
    rinzu_managed_collect(&data, &roots);
    verify(&data, second, "second\0value", 12, 42);
  }
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !data.root_sets);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_value_schema_dispose(&schema);
  assert(allocations == before); /* No closed token metadata remains. */
}
static void *worker(void *raw) {
  (void)raw;
  run();
  return NULL;
}
struct foreign {
  rinzu_userdata_registry_t *registry;
  rinzu_managed_data_t *data;
  rinzu_userdata_invocation_t invocation;
  const void *token;
};
static void *foreign(void *raw) {
  struct foreign *context = raw;
  uint64_t output = 0;
  (void)rinzu_userdata_resolve(context->registry, context->data, context->invocation, 2,
                               context->token, &output);
  return NULL;
}
int main(int argc, char **argv) {
  if (argc > 1) {
    const char *mode = argv[1];
    rinzu_managed_data_t data = {0};
    rinzu_value_schema_t schema = {0};
    rinzu_userdata_registry_t registry = {0};
    configure(&data, &schema);
    uint64_t value = record(&data, "probe", 5, 7, 2);
    if (!strncmp(mode, "scope-oom-", 10))
      budget = strtol(mode + 10, NULL, 10);
    uint64_t scope = rinzu_userdata_enter(&registry, &data);
    if (!strncmp(mode, "result-", 7)) rinzu_userdata_expect_result(&registry, &data, scope, 2);
    if (!strncmp(mode, "prepare-oom-", 12))
      budget = strtol(mode + 12, NULL, 10);
    if (!strcmp(mode, "not-composite")) {
      (void)rinzu_userdata_prepare(&registry, &data, scope, 1, value);
    }
    const void *token = rinzu_userdata_prepare(&registry, &data, scope, 2, value);
    if (!strcmp(mode, "not-open"))
      (void)rinzu_userdata_admit(&registry, &data, scope);
    rinzu_userdata_open(&registry, &data, scope);
    if (!strcmp(mode, "admit-oom"))
      budget = 0;
    rinzu_userdata_invocation_t invocation = rinzu_userdata_admit(&registry, &data, scope);
    uint64_t output = 0;
    if (!strncmp(mode, "result-", 7)) {
      if (!strcmp(mode,"result-before-return")) (void)rinzu_userdata_receive_result(&registry,&data,scope,2,token,&output);
      if (!strcmp(mode,"result-release-before-return")) rinzu_userdata_finish_result(&registry,&data,scope);
      rinzu_userdata_returned(&registry,&data,scope);
      if (!strcmp(mode,"result-in-flight")) (void)rinzu_userdata_receive_result(&registry,&data,scope,2,token,&output);
      if (!strcmp(mode,"result-early-release")) rinzu_userdata_finish_result(&registry,&data,scope);
      rinzu_userdata_settle(&registry,&data,invocation);
      if (!strcmp(mode,"result-wrong-type")) (void)rinzu_userdata_receive_result(&registry,&data,scope,4,token,&output);
      if (!strcmp(mode,"result-wrong-domain")) {rinzu_managed_data_t other={0};(void)rinzu_userdata_receive_result(&registry,&other,scope,2,token,&output);}
      if (!strcmp(mode,"result-wrong-call")) {
        uint64_t other=rinzu_userdata_enter(&registry,&data);rinzu_userdata_expect_result(&registry,&data,other,2);
        (void)rinzu_userdata_prepare(&registry,&data,other,2,value);
        rinzu_userdata_open(&registry,&data,other);rinzu_userdata_returned(&registry,&data,other);
        (void)rinzu_userdata_receive_result(&registry,&data,other,2,token,&output);
      }
      if (!strcmp(mode,"result-duplicate-read")) {assert(!rinzu_userdata_receive_result(&registry,&data,scope,2,token,&output));(void)rinzu_userdata_receive_result(&registry,&data,scope,2,token,&output);}
      if (!strcmp(mode,"result-missing-finish")) rinzu_userdata_finish(&registry,&data);
      abort();
    }
    if (!strcmp(mode, "unknown"))
      (void)rinzu_userdata_resolve(&registry, &data, invocation, 2, (void *)(uintptr_t)0x1000,
                                   &output);
    if (!strcmp(mode, "wrong-type"))
      (void)rinzu_userdata_resolve(&registry, &data, invocation, 4, token, &output);
    if (!strcmp(mode, "null-bad-type"))
      (void)rinzu_userdata_resolve(&registry, &data, invocation, 5, NULL, &output);
    if (!strcmp(mode, "wrong-domain")) {
      rinzu_managed_data_t other = {0};
      (void)rinzu_userdata_resolve(&registry, &other, invocation, 2, token, &output);
    }
    if (!strcmp(mode, "foreign-thread")) {
      pthread_t thread;
      struct foreign context = {&registry, &data, invocation, token};
      assert(!pthread_create(&thread, NULL, foreign, &context));
      assert(!pthread_join(thread, NULL));
    }
    if (!strcmp(mode, "wrong-scope")) {
      uint64_t next = rinzu_userdata_enter(&registry, &data);
      rinzu_userdata_open(&registry, &data, next);
      rinzu_userdata_invocation_t other = rinzu_userdata_admit(&registry, &data, next);
      (void)rinzu_userdata_resolve(&registry, &data, other, 2, token, &output);
    }
    if (!strcmp(mode, "closed")) {
      rinzu_userdata_settle(&registry, &data, invocation);
      rinzu_userdata_returned(&registry, &data, scope);
      uint64_t next = rinzu_userdata_enter(&registry, &data);
      assert(rinzu_userdata_prepare(&registry, &data, next, 2, value) != token);
      rinzu_userdata_open(&registry, &data, next);
      rinzu_userdata_invocation_t other = rinzu_userdata_admit(&registry, &data, next);
      (void)rinzu_userdata_resolve(&registry, &data, other, 2, token, &output);
    }
    if (!strcmp(mode, "settled") || !strcmp(mode, "null-without-admission")) {
      rinzu_userdata_settle(&registry, &data, invocation);
      (void)rinzu_userdata_resolve(&registry, &data, invocation, 2,
                                   !strcmp(mode, "settled") ? token : NULL, &output);
    }
    if (!strcmp(mode, "admit-after-return")) {
      rinzu_userdata_returned(&registry, &data, scope);
      (void)rinzu_userdata_admit(&registry, &data, scope);
    }
    if (!strcmp(mode, "double-settle")) {
      rinzu_userdata_settle(&registry, &data, invocation);
      rinzu_userdata_settle(&registry, &data, invocation);
    }
    if (!strcmp(mode, "discard-open"))
      rinzu_userdata_discard_prepared(&registry, &data, scope);
    if (!strcmp(mode, "unfinished-owner"))
      rinzu_userdata_finish(&registry, &data);
    puts("UNEXPECTED_RETURN");
    return 1;
  }
  run();
  pthread_t threads[8];
  for (size_t n = 0; n < 8; n++)
    assert(!pthread_create(&threads[n], NULL, worker, NULL));
  for (size_t n = 0; n < 8; n++)
    assert(!pthread_join(threads[n], NULL));
  rinzu_userdata_process_stats_t stats = rinzu_userdata_process_stats();
  assert(!stats.tokens && !stats.closed && !stats.token_bytes);
  printf("PASS userdata actual tokens GC Owner gate baseline process=%zu/%zu/%zu\n", stats.tokens,
         stats.closed, stats.token_bytes);
}

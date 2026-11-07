/* Collector storage behavior is private; native views still cannot span GC. */
#include <assert.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>
static size_t allocs, fail_at;
static void *counted_malloc(size_t bytes) {
  ++allocs;
  if (fail_at == allocs)
    return NULL;
  return malloc(bytes);
}
static void *counted_calloc(size_t n, size_t bytes) {
  ++allocs;
  if (fail_at == allocs)
    return NULL;
  return calloc(n, bytes);
}
#define malloc counted_malloc
#define calloc counted_calloc
#include "rinzu/managed_composite.h"
#undef malloc
#undef calloc
static void *wrong_owner(void *raw) {
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_collect(raw, &roots);
  return NULL;
}
int main(int argc, char **argv) {
  assert(argc == 2);
  unsigned oom = !strncmp(argv[1], "oom", 3) ? (unsigned)atoi(argv[1] + 3) : 0;
  int moving = !strcmp(argv[1], "moving") || !strcmp(argv[1], "stress");
  int stress = !strcmp(argv[1], "stress") || oom;
  moving |= stress;
  atomic_store(&rinzu_resource_enabled, 1);
  const size_t fields[] = {2};
  const rinzu_value_type_t types[] = {
      {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64},
      {.kind = RINZU_VALUE_DYNAMIC_BYTES},
      {.kind = RINZU_VALUE_STRING},
      {.kind = RINZU_VALUE_LIST, .element = 2},
      {.kind = RINZU_VALUE_STATE, .identity = "R01State", .fields = fields, .field_count = 1}};
  rinzu_value_schema_t schema = {0};
  assert(!rinzu_value_schema_init(&schema, types, 5));
  rinzu_managed_data_t data = {0};
  assert(!rinzu_managed_configure_values(&data, &schema));
  data.gc.stress = stress;
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  uint64_t ids[8];
  const size_t lengths[] = {0, 3, 16, 65536};
  for (unsigned i = 0; i < 4; ++i) {
    unsigned char *bytes = malloc(lengths[i] + 1);
    assert(bytes);
    memset(bytes, 33 + i, lengths[i] + 1);
    assert(!rinzu_managed_bytes(&data, lengths[i], bytes, lengths[i], &ids[i]));
    free(bytes);
  }
  ids[4] = rinzu_managed_string(&data, (const unsigned char *)"日本語", 9);
  rinzu_managed_slot_t one = {ids[4], 1};
  assert(!rinzu_managed_composite(&data, 3, -1, &one, 1, &ids[5]));
  rinzu_managed_slot_t many[] = {one, one, one};
  assert(!rinzu_managed_composite(&data, 3, -1, many, 3, &ids[6]));
  assert(!rinzu_managed_composite(&data, 4, -1, &one, 1, &ids[7]));
  (void)rinzu_managed_string(&data, (const unsigned char *)"dead", 4);
  rinzu_managed_publish(&roots, ids, 8);
  if (oom) {
    assert(oom <= 8);
    fail_at = allocs + oom;
    rinzu_managed_collect(&data, &roots);
    _Exit(1);
  }
  for (unsigned turn = 0; turn < 32; ++turn) {
    uintptr_t bytes[5], slots[3];
    for (unsigned i = 0; i < 5; ++i)
      bytes[i] = (uintptr_t)rinzu_managed_find(&data, ids[i])->bytes;
    for (unsigned i = 0; i < 3; ++i)
      slots[i] = (uintptr_t)rinzu_managed_find(&data, ids[5 + i])->slots;
    size_t before = allocs;
    uint64_t epoch = data.collections;
    rinzu_managed_collect(&data, &roots);
    assert(data.collections == epoch + 1 && data.objects == 8);
    if (!moving)
      assert(allocs == before);
    else
      assert(allocs >= before + 8);
    for (unsigned i = 0; i < 5; ++i) {
      rinzu_managed_value_t *v = rinzu_managed_find(&data, ids[i]);
      assert(((uintptr_t)v->bytes != bytes[i]) == moving);
      if (i < 4) {
        assert(v->length == lengths[i] && !v->bytes[lengths[i]]);
        for (size_t j = 0; j < v->length; ++j)
          assert(v->bytes[j] == 33 + i);
        if (!moving && data.inline_bytes_enabled && lengths[i] < sizeof(v->inline_bytes))
          assert(v->bytes == v->inline_bytes);
      } else
        assert(v->length == 9 && !memcmp(v->bytes, "日本語", 9));
    }
    for (unsigned i = 0; i < 3; ++i) {
      rinzu_managed_value_t *v = rinzu_managed_find(&data, ids[5 + i]);
      assert(((uintptr_t)v->slots != slots[i]) == moving && v->slots[0].bits == ids[4]);
      if (!moving && i != 1)
        assert(v->slots == &v->inline_slot);
    }
    assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live ==
           data.heap_bytes + data.index_accounted_bytes);
  }
  for (unsigned guard = 0; guard < 4; ++guard) {
    pid_t child = fork();
    assert(child >= 0);
    if (!child) {
      if (guard == 0)
        data.live_views = 1;
      else if (guard == 1)
        data.view_allocations = 1;
      else if (guard == 2)
        roots.current->writing = 1;
      else {
        pthread_t t;
        assert(!pthread_create(&t, NULL, wrong_owner, &data));
        pthread_join(t, NULL);
        _Exit(1);
      }
      rinzu_managed_collect(&data, &roots);
      _Exit(1);
    }
    int status;
    assert(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 70);
  }
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !data.heap_bytes);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_value_schema_dispose(&schema);
  for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
    assert(!rinzu_resource_counters[i].live);
  puts("R01:normal/stress:32;bytes;slots;inline;roots;borrow;owner;accounting:OK");
}

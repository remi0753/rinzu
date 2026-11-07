/* Frozen-header CPU probe: clang -O2 -pthread -I <include> value_cpu_probe.c.
   Allocation diagnostics run separately from CPU/wall timing. */
#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static volatile int counting;
static uint64_t allocations, allocated_bytes;
#ifdef RINZU_VERIFY_ALLOCATIONS
static size_t live_allocations;
#endif
static void *counted_calloc(size_t count, size_t width) {
  void *pointer = calloc(count, width);
#ifdef RINZU_VERIFY_ALLOCATIONS
  if (pointer)
    ++live_allocations;
#endif
  if (counting && pointer) {
    ++allocations;
    allocated_bytes += count * width;
  }
  return pointer;
}
static inline void *counted_malloc(size_t width) {
  void *pointer = malloc(width);
#ifdef RINZU_VERIFY_ALLOCATIONS
  if (pointer)
    ++live_allocations;
#endif
  if (counting && pointer) {
    ++allocations;
    allocated_bytes += width;
  }
  return pointer;
}
#ifdef RINZU_VERIFY_ALLOCATIONS
static void counted_free(void *pointer) {
  if (pointer) {
    assert(live_allocations);
    --live_allocations;
  }
  free(pointer);
}
#define free counted_free
#endif
#define calloc counted_calloc
#define malloc counted_malloc
#include "rinzu/managed_composite.h"
#undef calloc
#undef malloc
#ifdef RINZU_VERIFY_ALLOCATIONS
#undef free
#endif

static uint64_t tick(clockid_t clock) {
  struct timespec now;
  assert(!clock_gettime(clock, &now));
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
static __attribute__((noinline)) uint64_t operation(rinzu_managed_data_t *data, size_t type,
                                                    const rinzu_value_t *a, const rinzu_value_t *b,
                                                    rinzu_managed_slot_t left,
                                                    rinzu_managed_slot_t right, unsigned op) {
  uint64_t hash = 0;
  int equal = 0;
  switch (op) {
  case 0:
    assert(!rinzu_value_validate(data->schema, type, a));
    return 1;
  case 1:
    assert(!rinzu_value_equal(data->schema, type, a, b, &equal));
    return (uint64_t)equal;
  case 2:
    assert(!rinzu_value_key_hash(data->schema, type, a, &hash));
    return hash;
  case 3:
    assert(!rinzu_managed_values_equal(data, type, left, right, &equal));
    return (uint64_t)equal;
  case 5: {
    rinzu_value_map_t map = {0};
    assert(!rinzu_value_map(data->schema, type, a->entries, a->count, &map));
    uint64_t count = map.count;
    rinzu_value_map_dispose(&map);
    return count;
  }
  case 6: {
    rinzu_managed_snapshot_t view = {0};
    assert(!rinzu_managed_snapshot(data, type, left, &view));
    uint64_t checksum = view.value.count;
    if (view.value.count) {
      const rinzu_value_t *last = &view.value.fields[view.value.count - 1];
      checksum += last->kind == RINZU_VALUE_STRING ? last->length : last->bits;
    }
    rinzu_managed_snapshot_dispose(&view);
    return checksum;
  }
  default:
    assert(!rinzu_managed_value_hash(data, type, left, &hash));
    return hash;
  }
}
static void run(const char *name, rinzu_managed_data_t *data, size_t type, const rinzu_value_t *a,
                const rinzu_value_t *b, rinzu_managed_slot_t left, rinzu_managed_slot_t right,
                unsigned op, size_t rounds) {
  uint64_t expected = operation(data, type, a, b, left, right, op);
  for (size_t i = 0; i < (rounds < 100 ? rounds : 100); ++i)
    assert(operation(data, type, a, b, left, right, op) == expected);
  uint64_t elapsed = 0, cpu = 0, checksum = 0;
  for (int diagnostic = 0; diagnostic < 2; ++diagnostic) {
    allocations = allocated_bytes = checksum = 0;
    counting = diagnostic;
    uint64_t start_cpu = tick(CLOCK_PROCESS_CPUTIME_ID), start = tick(CLOCK_MONOTONIC);
    for (size_t i = 0; i < rounds; ++i)
      checksum += operation(data, type, a, b, left, right, op);
    if (!diagnostic) {
      elapsed = tick(CLOCK_MONOTONIC) - start;
      cpu = tick(CLOCK_PROCESS_CPUTIME_ID) - start_cpu;
    }
    counting = 0;
    assert(checksum == expected * rounds);
    assert(!data->live_views && !data->view_allocations);
  }
  printf("{\"case\":\"%s\",\"operations\":%zu,\"nanoseconds\":%" PRIu64
         ",\"cpu_nanoseconds\":%" PRIu64 ",\"allocations\":%" PRIu64 ",\"allocated_bytes\":%" PRIu64
         ",\"checksum\":%" PRIu64 "}\n",
         name, rounds, elapsed, cpu, allocations, allocated_bytes, checksum);
}

static const rinzu_value_type_t types[] = {
    {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64},
    {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_F64},
    {.kind = RINZU_VALUE_UNIT},
    {.kind = RINZU_VALUE_STRING},
    {.kind = RINZU_VALUE_MAP, .key = 0, .value = 0},
    {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_BOOL},
    {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_U8},
    {.kind = RINZU_VALUE_BYTES, .bytes_length = 3},
    {.kind = RINZU_VALUE_DYNAMIC_BYTES},
};

static void verify(rinzu_managed_data_t *data) {
  for (unsigned kind = 0; kind <= RINZU_SCALAR_BOOL; ++kind) {
    rinzu_value_type_t type = {.kind = RINZU_VALUE_SCALAR, .scalar_kind = kind};
    rinzu_value_schema_t schema = {0};
    assert(!rinzu_value_schema_init(&schema, &type, 1));
    rinzu_managed_data_t domain = {0};
    assert(!rinzu_managed_configure_values(&domain, &schema));
    const uint64_t bits[] = {0,
                             1,
                             2,
                             UINT64_MAX,
                             UINT64_C(0x8000000000000000),
                             UINT64_C(0x7ff8000000000000),
                             UINT64_C(0x80000000),
                             UINT64_C(0x7fc00000)};
    for (size_t i = 0; i < sizeof(bits) / sizeof(*bits); ++i) {
      for (size_t j = 0; j < sizeof(bits) / sizeof(*bits); ++j) {
        rinzu_value_t a = {.kind = RINZU_VALUE_SCALAR, .scalar_kind = kind, .bits = bits[i]}, b = a;
        b.bits = bits[j];
        int expected = rinzu_transfer_valid(kind, a.bits) && rinzu_transfer_valid(kind, b.bits)
                           ? RINZU_VALUE_OK
                           : RINZU_VALUE_TYPE;
        int equal = 37, managed = 37;
        assert(rinzu_value_equal(&schema, 0, &a, &b, &equal) == expected);
        assert(rinzu_managed_values_equal(&domain, 0, (rinzu_managed_slot_t){a.bits, 0},
                                          (rinzu_managed_slot_t){b.bits, 0}, &managed) == expected);
        int answer = expected ? 37 : rinzu_scalar_compare(kind, 0, a.bits, b.bits);
        assert(equal == answer && managed == answer);
        if (!expected && schema.keys.allowed[0]) {
          uint64_t raw = 0, hash = 0;
          assert(!rinzu_value_key_hash(&schema, 0, &a, &raw));
          assert(!rinzu_managed_value_hash(&domain, 0, (rinzu_managed_slot_t){a.bits, 0}, &hash));
          assert(raw == hash);
        }
      }
    }
    rinzu_managed_data_dispose(&domain);
    rinzu_value_schema_dispose(&schema);
  }
  int equal = 37;
  assert(rinzu_managed_values_equal(data, 0, (rinzu_managed_slot_t){0, 2},
                                    (rinzu_managed_slot_t){0, 0}, &equal) == RINZU_VALUE_TYPE &&
         equal == 37);
  assert(rinzu_managed_values_equal(data, 2, (rinzu_managed_slot_t){1, 0},
                                    (rinzu_managed_slot_t){0, 0}, &equal) == RINZU_VALUE_TYPE &&
         equal == 37);
  assert(rinzu_value_equal(data->schema, 0, NULL, NULL, &equal) == RINZU_VALUE_SCHEMA);
  rinzu_value_t invalid = {
      .kind = RINZU_VALUE_STRING, .bytes = (const unsigned char *)"\xff", .length = 1};
  uint64_t hash = 91;
  assert(rinzu_value_key_hash(data->schema, 3, &invalid, &hash) == RINZU_VALUE_TYPE && hash == 91);
  assert(rinzu_value_equal(data->schema, 3, &invalid, &invalid, &equal) == RINZU_VALUE_TYPE &&
         equal == 37);
  invalid = (rinzu_value_t){
      .kind = RINZU_VALUE_BYTES, .bytes = (const unsigned char *)"123", .length = 2};
  assert(rinzu_value_validate(data->schema, 7, &invalid) == RINZU_VALUE_TYPE);
  invalid.kind = RINZU_VALUE_DYNAMIC_BYTES;
  invalid.length = (size_t)INT64_MAX + 1;
  assert(rinzu_value_validate(data->schema, 8, &invalid) == RINZU_VALUE_TYPE);
  rinzu_managed_data_t other = {0};
  assert(!rinzu_managed_configure_values(&other, data->schema));
  uint64_t foreign = rinzu_managed_string(&other, (const unsigned char *)"x", 1);
  assert(rinzu_managed_values_equal(data, 3, (rinzu_managed_slot_t){foreign, 1},
                                    (rinzu_managed_slot_t){foreign, 1},
                                    &equal) == RINZU_VALUE_TYPE &&
         equal == 37);
  rinzu_managed_data_dispose(&other);
}

static void *foreign_owner(void *raw) {
  int equal = 37;
  (void)rinzu_managed_values_equal(raw, 0, (rinzu_managed_slot_t){0, 0},
                                   (rinzu_managed_slot_t){0, 0}, &equal);
  return NULL;
}

static void *foreign_snapshot(void *raw) {
  rinzu_managed_snapshot_t view = {0};
  (void)rinzu_managed_snapshot(raw, 0, (rinzu_managed_slot_t){0, 0}, &view);
  return NULL;
}

static void snapshots(size_t width, int verify_only) {
  size_t *fields = width ? calloc(width, sizeof(*fields)) : NULL;
  const rinzu_value_type_t layouts[] = {
      {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64},
      {.kind = RINZU_VALUE_STRING},
      {.kind = RINZU_VALUE_LIST, .element = 0},
      {.kind = RINZU_VALUE_LIST, .element = 1},
      {.kind = RINZU_VALUE_RECORD,
       .identity = "SnapshotRecord",
       .field_count = width,
       .fields = fields},
  };
  rinzu_value_schema_t schema = {0};
  rinzu_managed_data_t data = {0};
  assert(!rinzu_value_schema_init(&schema, layouts, 5));
  assert(!rinzu_managed_configure_values(&data, &schema));
  rinzu_managed_slot_t *slots = width ? calloc(width, sizeof(*slots)) : NULL;
  for (size_t type = 2; type < 5; ++type) {
    uint64_t text = type == 3 ? rinzu_managed_string(&data, (const unsigned char *)"日本語", 9) : 0;
    for (size_t i = 0; i < width; ++i)
      slots[i] = (rinzu_managed_slot_t){type == 3 ? text : i, type == 3};
    uint64_t root = 0;
    assert(!rinzu_managed_composite(&data, type, -1, slots, width, &root));
    rinzu_managed_roots_t roots = {0};
    rinzu_managed_enter(&roots);
    rinzu_managed_publish(&roots, &root, 1);
    rinzu_managed_collect(&data, &roots);
    if (!verify_only) {
      for (unsigned i = 0; i < 2; ++i) {
        unsigned op = i == 0 ? 6 : (type == 4 ? 4 : 3);
        char name[80];
        assert(snprintf(name, sizeof(name), "%s-%s-%zu",
                        i == 0 ? "snapshot" : (type == 4 ? "hash" : "equal"),
                        type == 2 ? "int-list" : (type == 3 ? "string-list" : "record"),
                        width) > 0);
        run(name, &data, type, NULL, NULL, (rinzu_managed_slot_t){root, 1},
            (rinzu_managed_slot_t){root, 1}, op, width > 8 ? 200 : 10000);
      }
    }
    rinzu_managed_snapshot_t view = {0};
    assert(!rinzu_managed_snapshot(&data, type, (rinzu_managed_slot_t){root, 1}, &view));
    assert(data.live_views == 1 && view.value.count == width);
    for (size_t i = 0; i < width; ++i) {
      if (type == 3)
        assert(view.value.fields[i].length == 9 &&
               !memcmp(view.value.fields[i].bytes, "日本語", 9));
      else
        assert(view.value.fields[i].bits == i);
    }
    rinzu_managed_snapshot_dispose(&view);
    if (width) {
      rinzu_managed_value_t *value = rinzu_managed_find(&data, root);
      rinzu_managed_slot_t original = value->slots[width - 1];
      value->slots[width - 1].reference = 2;
      view.value.kind = 77;
      assert(rinzu_managed_snapshot(&data, type, (rinzu_managed_slot_t){root, 1}, &view) ==
             RINZU_VALUE_TYPE);
      assert(view.value.kind == 77 && !view.initialized && !view.allocations && !view.owner);
      assert(!data.live_views && !data.view_allocations);
      value->slots[width - 1] = original;
    }
    rinzu_managed_leave(&roots);
    rinzu_managed_dispose(&roots);
  }
  rinzu_managed_data_dispose(&data);
  rinzu_value_schema_dispose(&schema);
  free(slots);
  free(fields);
}

static void verify_maps(const rinzu_value_schema_t *schema) {
  for (size_t width = 1; width <= 128; width *= 2) {
    rinzu_value_pair_t *a = calloc(width, sizeof(*a)), *b = calloc(width, sizeof(*b));
    assert(a && b);
    for (size_t i = 0; i < width; ++i) {
      a[i].key =
          (rinzu_value_t){.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64, .bits = i};
      a[i].value = a[i].key;
      b[width - i - 1] = a[i];
    }
    rinzu_value_t left = {.kind = RINZU_VALUE_MAP, .count = width, .entries = a}, right = left;
    right.entries = b;
    int equal = 37;
    assert(!rinzu_value_equal(schema, 4, &left, &right, &equal) && equal);
    b[width - 1].value.bits = 999;
    assert(!rinzu_value_equal(schema, 4, &left, &right, &equal) && !equal);
    b[width - 1].value.kind = RINZU_VALUE_UNIT;
    equal = 37;
    assert(rinzu_value_equal(schema, 4, &left, &right, &equal) == RINZU_VALUE_TYPE && equal == 37);
    b[width - 1].value = a[0].value;
    b[0].key.bits = 9999;
    assert(!rinzu_value_equal(schema, 4, &left, &right, &equal) && !equal);
    if (width > 1) {
      b[0].key = b[width - 1].key;
      equal = 37;
      assert(rinzu_value_validate(schema, 4, &right) == RINZU_VALUE_DUPLICATE);
      assert(rinzu_value_equal(schema, 4, &left, &right, &equal) == RINZU_VALUE_DUPLICATE &&
             equal == 37);
      rinzu_value_map_t normalized = {0};
      assert(!rinzu_value_map(schema, 4, b, width, &normalized));
      assert(normalized.count == width - 1 && normalized.entries[0].value.bits == 0);
      rinzu_value_map_dispose(&normalized);
    }
    free(a);
    free(b);
  }
}

int main(int argc, char **argv) {
  assert(argc == 2);
  rinzu_value_schema_t schema = {0};
  rinzu_managed_data_t data = {0};
  assert(!rinzu_value_schema_init(&schema, types, sizeof(types) / sizeof(*types)));
  assert(!rinzu_managed_configure_values(&data, &schema));
  verify(&data);
  if (!strcmp(argv[1], "foreign-owner") || !strcmp(argv[1], "foreign-snapshot")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL,
                           !strcmp(argv[1], "foreign-owner") ? foreign_owner : foreign_snapshot,
                           &data));
    assert(!pthread_join(thread, NULL));
    abort();
  } else if (!strcmp(argv[1], "leaf")) {
    const size_t type_ids[] = {0, 1, 2, 3};
    const char *names[] = {"int", "float", "unit", "string"};
    const char *ops[] = {"validate", "equal", "hash", "managed-equal", "managed-hash"};
    uint64_t string = rinzu_managed_string(&data, (const unsigned char *)"日本語", 9);
    for (size_t i = 0; i < 4; ++i) {
      size_t type = type_ids[i];
      rinzu_value_t value = {.kind = types[type].kind, .scalar_kind = types[type].scalar_kind};
      value.bits = i == 0 ? 42 : 0;
      value.bytes = (const unsigned char *)"日本語";
      value.length = 9;
      rinzu_managed_slot_t slot = {i == 3 ? string : value.bits, i == 3};
      for (unsigned op = 0; op < 5; ++op) {
        if ((op == 2 || op == 4) && !schema.keys.allowed[type])
          continue;
        char name[80];
        assert(snprintf(name, sizeof(name), "%s-%s", ops[op], names[i]) > 0);
        run(name, &data, type, &value, &value, slot, slot, op, 200000);
      }
    }
  } else if (!strcmp(argv[1], "map")) {
    const size_t widths[] = {0, 1, 16, 128, 1024};
    for (size_t w = 0; w < sizeof(widths) / sizeof(*widths); ++w) {
      size_t width = widths[w];
      rinzu_value_pair_t *pairs = calloc(width, sizeof(*pairs)),
                         *reverse = calloc(width, sizeof(*reverse));
      assert(!width || (pairs && reverse));
      for (size_t i = 0; i < width; ++i) {
        pairs[i].key =
            (rinzu_value_t){.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64, .bits = i};
        pairs[i].value = pairs[i].key;
        reverse[width - i - 1] = pairs[i];
      }
      rinzu_value_t a = {.kind = RINZU_VALUE_MAP, .entries = pairs, .count = width}, b = a;
      b.entries = reverse;
      for (unsigned operation = 0; operation < 3; ++operation) {
        unsigned op = operation == 2 ? 5 : operation;
        const char *names[] = {"validate", "equal", "normalize"};
        char name[80];
        assert(snprintf(name, sizeof(name), "map-%s-%zu", names[operation], width) > 0);
        run(name, &data, 4, &a, &b, (rinzu_managed_slot_t){0}, (rinzu_managed_slot_t){0}, op,
            width == 1024 ? 20 : (width == 128 ? 500 : (width == 16 ? 10000 : 100000)));
      }
      if (width > 1) {
        int equal = 37;
        reverse[width - 1].key = reverse[0].key;
        assert(rinzu_value_validate(&schema, 4, &b) == RINZU_VALUE_DUPLICATE);
        assert(rinzu_value_equal(&schema, 4, &a, &b, &equal) == RINZU_VALUE_DUPLICATE &&
               equal == 37);
      }
      free(pairs);
      free(reverse);
    }
  } else if (!strcmp(argv[1], "verify-map"))
    verify_maps(&schema);
  else if (!strcmp(argv[1], "verify-snapshot"))
    snapshots(32, 1);
  else if (!strcmp(argv[1], "snapshot")) {
    snapshots(0, 0);
    snapshots(8, 0);
    snapshots(1024, 0);
  } else
    assert(!strcmp(argv[1], "verify"));
  rinzu_managed_data_dispose(&data);
  rinzu_value_schema_dispose(&schema);
#ifdef RINZU_VERIFY_ALLOCATIONS
  assert(!live_allocations);
#endif
}

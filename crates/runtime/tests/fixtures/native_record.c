#include <stdlib.h>
#include <errno.h>
static _Thread_local long budget = -1;
static int allowed(void) {
  if (budget < 0)
    return 1;
  if (!budget)
    return 0;
  --budget;
  return 1;
}
static void *controlled_calloc(size_t count, size_t size) {
  return allowed() ? calloc(count, size) : NULL;
}
static int controlled_aligned(void **pointer, size_t alignment, size_t size) {
  return allowed() ? posix_memalign(pointer, alignment, size) : ENOMEM;
}
#define calloc controlled_calloc
#define posix_memalign controlled_aligned
#include "rinzu/native_record.h"
#undef calloc
#undef posix_memalign
#include <assert.h>
#include <stdio.h>
#include <sys/mman.h>

struct Record {
  int8_t a;
  int16_t b;
  int32_t c;
  int64_t d;
  uint8_t e;
  uint16_t f;
  uint32_t g;
  uint64_t h;
  float i;
  double j;
  _Bool k;
};
static const rinzu_native_record_field_t fields[] = {
    {0, offsetof(struct Record, a)}, {1, offsetof(struct Record, b)},
    {2, offsetof(struct Record, c)}, {3, offsetof(struct Record, d)},
    {4, offsetof(struct Record, e)}, {5, offsetof(struct Record, f)},
    {6, offsetof(struct Record, g)}, {7, offsetof(struct Record, h)},
    {8, offsetof(struct Record, i)}, {9, offsetof(struct Record, j)},
    {10, offsetof(struct Record, k)}};
static const rinzu_native_record_layout_t layout = {11, sizeof(struct Record),
                                                    _Alignof(struct Record), 11, fields};
static const uint64_t expected[] = {UINT64_C(0xffffffffffffff80),
                                    UINT64_C(0xffffffffffff8000),
                                    UINT64_C(0xffffffff80000000),
                                    UINT64_C(0x8000000000000000),
                                    255,
                                    65535,
                                    UINT32_MAX,
                                    UINT64_MAX,
                                    UINT64_C(0x80000000),
                                    UINT64_C(0x7ff800000000002a),
                                    1};
static void input(struct Record *value, unsigned char padding) {
  memset(value, padding, sizeof(*value));
  value->a = INT8_MIN;
  value->b = INT16_MIN;
  value->c = INT32_MIN;
  value->d = INT64_MIN;
  value->e = UINT8_MAX;
  value->f = UINT16_MAX;
  value->g = UINT32_MAX;
  value->h = UINT64_MAX;
  uint32_t f = (uint32_t)expected[8];
  memcpy(&value->i, &f, sizeof(f));
  uint64_t d = expected[9];
  memcpy(&value->j, &d, sizeof(d));
  value->k = 1;
}
static void configure(rinzu_managed_data_t *data, rinzu_value_schema_t *schema) {
  static const size_t children[] = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10};
#define SCALAR(k)                                                                                  \
  {                                                                                                \
    .kind = RINZU_VALUE_SCALAR, .scalar_kind = k                                                   \
  }
  static const rinzu_value_type_t types[] = {
      SCALAR(0),
      SCALAR(1),
      SCALAR(2),
      SCALAR(3),
      SCALAR(4),
      SCALAR(5),
      SCALAR(6),
      SCALAR(7),
      SCALAR(8),
      SCALAR(9),
      SCALAR(10),
      {.kind = RINZU_VALUE_RECORD, .identity = "Record", .fields = children, .field_count = 11}};
#undef SCALAR
  assert(!rinzu_value_schema_init(schema, types, 12));
  assert(!rinzu_managed_configure_values(data, schema));
}
static void run(void) {
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t roots = {0};
  rinzu_value_schema_t schema = {0};
  configure(&data, &schema);
  struct Record value;
  input(&value, 0xa5);
  uint64_t handle = 0;
  assert(!rinzu_managed_receive_record(&data, &layout, (const unsigned char *)&value, &handle));
  rinzu_managed_enter(&roots);
  rinzu_managed_publish(&roots, &handle, 1);
  rinzu_native_record_buffer_t copy = rinzu_managed_copy_record(&data, &layout, handle);
  assert((uintptr_t)copy.pointer % layout.alignment == 0);
  struct Record golden;
  input(&golden, 0);
  assert(!memcmp(copy.pointer, &golden, sizeof(golden)));
  uint64_t unchanged = UINT64_MAX;
  size_t count = data.objects;
  budget = 0;
  assert(rinzu_managed_receive_record(&data, &layout, NULL, &unchanged) ==
         RINZU_NATIVE_NULL_RECORD);
  budget = -1;
  assert(unchanged == UINT64_MAX && data.objects == count);
  unsigned char invalid = 2;
  memcpy((unsigned char *)&value + fields[10].offset, &invalid, 1);
  assert(rinzu_managed_receive_record(&data, &layout, (const unsigned char *)&value, &unchanged) ==
         RINZU_NATIVE_RECORD_CONVERSION);
  assert(unchanged == UINT64_MAX && data.objects == count);
  input(&value, 0x5a);
  uint64_t second = 0;
  assert(!rinzu_managed_receive_record(&data, &layout, (const unsigned char *)&value, &second));
  int equal = 0;
  assert(!rinzu_managed_values_equal(&data, 11, (rinzu_managed_slot_t){handle, 1},
                                     (rinzu_managed_slot_t){second, 1}, &equal) &&
         !equal); /* NaN */
  value.j = 1;
  uint64_t finite_a = 0, finite_b = 0;
  assert(!rinzu_managed_receive_record(&data, &layout, (const unsigned char *)&value, &finite_a));
  input(&value, 0x3c);
  value.j = 1;
  assert(!rinzu_managed_receive_record(&data, &layout, (const unsigned char *)&value, &finite_b));
  assert(!rinzu_managed_values_equal(&data, 11, (rinzu_managed_slot_t){finite_a, 1},
                                     (rinzu_managed_slot_t){finite_b, 1}, &equal) &&
         equal);
  uint64_t live[] = {handle, second};
  rinzu_managed_publish(&roots, live, 2);
  memset(&value, 0, sizeof(value));
  unsigned char *address = copy.pointer;
  for (int n = 0; n < 32; ++n) {
    rinzu_managed_collect(&data, &roots);
    assert(copy.pointer == address);
    for (size_t i = 0; i < 11; ++i) {
      rinzu_managed_slot_t slot;
      assert(!rinzu_managed_project(&data, handle, 11, -1, i, &slot));
      assert(!slot.reference && slot.bits == expected[i]);
    }
  }
  rinzu_native_record_buffer_dispose(&copy);
  assert(!copy.pointer);
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !data.live_views && !data.view_allocations);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_value_schema_dispose(&schema);
}
static void *worker(void *unused) {
  (void)unused;
  run();
  return NULL;
}
int main(int argc, char **argv) {
  if (argc > 1 && !strcmp(argv[1], "layout")) {
    printf("%zu %zu", layout.size, layout.alignment);
    for (size_t i = 0; i < 11; ++i)
      printf(" %zu", fields[i].offset);
    puts("");
    return 0;
  }
  if (argc > 1 && !strcmp(argv[1], "golden")) {
    struct Record value;
    input(&value, 0);
    for (size_t i = 0; i < sizeof(value); ++i)
      printf("%02x", ((unsigned char *)&value)[i]);
    puts("");
    return 0;
  }
  if (argc > 1) {
    rinzu_managed_data_t data = {0};
    rinzu_value_schema_t schema = {0};
    configure(&data, &schema);
    struct Record value;
    input(&value, 0);
    uint64_t handle = 0;
    if (!strcmp(argv[1], "read")) {
      void *page = mmap(NULL, 16384, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
      assert(page != MAP_FAILED);
      (void)rinzu_managed_receive_record(&data, &layout, page, &handle);
    } else if (!strncmp(argv[1], "receive-oom-", 12)) {
      budget = strtol(argv[1] + 12, NULL, 10);
      (void)rinzu_managed_receive_record(&data, &layout, (const unsigned char *)&value, &handle);
    } else {
      assert(!rinzu_managed_receive_record(&data, &layout, (const unsigned char *)&value, &handle));
      if (!strcmp(argv[1], "wrong-type")) {
        rinzu_native_record_layout_t bad = layout;
        bad.type = 0;
        (void)rinzu_managed_copy_record(&data, &bad, handle);
      }
      if (!strcmp(argv[1], "copy-oom")) {
        budget = 0;
        (void)rinzu_managed_copy_record(&data, &layout, handle);
      }
    }
    puts("UNEXPECTED_RETURN");
    return 1;
  }
  /* The catalog is immutable before worker start. */
  run();
  pthread_t threads[8];
  for (size_t i = 0; i < 8; ++i)
    assert(!pthread_create(&threads[i], NULL, worker, NULL));
  for (size_t i = 0; i < 8; ++i)
    assert(!pthread_join(threads[i], NULL));
  puts("PASS record target layout bits padding NULL conversion Main Worker moving GC baseline");
}

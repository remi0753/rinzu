#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stddef.h>
static struct {
  void *pointer;
  size_t bytes;
} allocations[32768];
static size_t allocation_calls, live_bytes;
static void remember(void *pointer, size_t bytes) {
  if (!pointer)
    return;
  for (size_t i = 0; i < 32768; ++i)
    if (!allocations[i].pointer) {
      allocations[i].pointer = pointer;
      allocations[i].bytes = bytes;
      live_bytes += bytes;
      ++allocation_calls;
      return;
    }
  abort();
}
static size_t forget(void *pointer) {
  if (!pointer)
    return 0;
  for (size_t i = 0; i < 32768; ++i)
    if (allocations[i].pointer == pointer) {
      size_t bytes = allocations[i].bytes;
      allocations[i].pointer = NULL;
      live_bytes -= bytes;
      return bytes;
    }
  abort();
}
static void *count_malloc(size_t bytes) {
  void *p = malloc(bytes);
  remember(p, bytes);
  return p;
}
static void *count_calloc(size_t n, size_t bytes) {
  void *p = calloc(n, bytes);
  remember(p, n * bytes);
  return p;
}
static void *count_realloc(void *p, size_t bytes) {
  size_t old = forget(p);
  void *out = realloc(p, bytes);
  if (!out && bytes) {
    remember(p, old);
    return NULL;
  }
  remember(out, bytes);
  return out;
}
static int count_aligned(void **p, size_t alignment, size_t bytes) {
  int status = posix_memalign(p, alignment, bytes);
  if (!status)
    remember(*p, bytes);
  return status;
}
static void count_free(void *p) {
  forget(p);
  free(p);
}
#define malloc count_malloc
#define calloc count_calloc
#define realloc count_realloc
#define posix_memalign count_aligned
#define free count_free
#include "rinzu/native_record.h"

static size_t cached_batches(rinzu_managed_data_t *data) {
  size_t count = 0;
  for (rinzu_managed_record_loan_t *link = data->native_record_pool; link; link = link->next) {
    rinzu_native_record_loan_t *loan = (void *)link;
    assert(!link->identities && !link->count && !link->busy && !loan->owner && !loan->entries &&
           !loan->arguments && !loan->roots.current && !loan->root_set.data);
    ++count;
  }
  return count;
}
static void complete(rinzu_managed_data_t *data, rinzu_native_record_loan_t *loan) {
  rinzu_native_record_enter(data, loan);
  rinzu_native_record_returned(data, loan);
  rinzu_native_record_commit(data, loan);
  rinzu_native_record_release(data, loan);
}
static void exercise(size_t count, size_t targets, size_t depth) {
  unsigned kinds[] = {RINZU_SCALAR_I64, RINZU_SCALAR_F64, RINZU_SCALAR_BOOL, RINZU_SCALAR_I16};
  size_t *field_types = malloc(count * sizeof(*field_types));
  rinzu_native_record_field_t *fields = malloc(count * sizeof(*fields));
  rinzu_managed_slot_t *slots = calloc(count, sizeof(*slots));
  size_t offset = 0;
  for (size_t i = 0; i < count; ++i) {
    unsigned kind = kinds[i % 4];
    size_t width = rinzu_scalar_width(kind) / 8;
    offset += (width - offset % width) % width;
    field_types[i] = i % 4;
    fields[i] = (rinzu_native_record_field_t){kind, offset};
    offset += width;
    slots[i] = (rinzu_managed_slot_t){kind == RINZU_SCALAR_BOOL, 0};
  }
  offset += (8 - offset % 8) % 8;
  rinzu_value_type_t types[] = {{.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64},
                                {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_F64},
                                {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_BOOL},
                                {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I16},
                                {.kind = RINZU_VALUE_STATE,
                                 .identity = "Packet",
                                 .field_count = count,
                                 .fields = field_types}};
  rinzu_native_record_layout_t layout = {4, offset, 8, count, fields};
  rinzu_value_schema_t schema = {0};
  assert(!rinzu_value_schema_init(&schema, types, 5));
  rinzu_managed_data_t data = {0};
  assert(!rinzu_managed_configure_values(&data, &schema));
  size_t handles_count = depth ? depth : 4;
  uint64_t *handles = malloc(handles_count * sizeof(*handles));
  for (size_t i = 0; i < handles_count; ++i)
    assert(!rinzu_managed_composite(&data, 4, -1, slots, count, &handles[i]));
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  rinzu_managed_publish(&roots, handles, handles_count);
  if (depth) {
    rinzu_native_record_loan_t **loans = calloc(depth, sizeof(*loans));
    for (size_t i = 0; i < depth; ++i) {
      rinzu_native_record_argument_t arg = {handles[i], &layout};
      assert(!rinzu_native_record_prepare(&data, &arg, 1, &loans[i]));
      rinzu_native_record_enter(&data, loans[i]);
    }
    assert(data.published_root_count == depth);
    for (size_t i = depth; i > 0; --i) {
      rinzu_native_record_returned(&data, loans[i - 1]);
      rinzu_native_record_commit(&data, loans[i - 1]);
      rinzu_native_record_release(&data, loans[i - 1]);
    }
    assert(data.native_record_pool_bytes <= RINZU_NATIVE_RECORD_SCRATCH_LIMIT);
    assert(cached_batches(&data) > 0 && cached_batches(&data) < depth);
    printf("NATIVE_RECORD_SCRATCH={\"case\":\"deep\",\"depth\":%zu,\"retainedBytes\":%zu,"
           "\"cachedBatches\":%zu,\"commonRootCacheBytes\":%zu}\n",
           depth, data.native_record_pool_bytes, cached_batches(&data), data.frame_cache.bytes);
    free(loans);
  } else {
    rinzu_native_record_argument_t arguments[8];
    for (size_t i = 0; i < 8; ++i)
      arguments[i] = (rinzu_native_record_argument_t){handles[i % targets], &layout};
    rinzu_native_record_loan_t *loan = NULL;
    assert(!rinzu_native_record_prepare(&data, arguments, 8, &loan));
    complete(&data, loan);
    if (offset * targets <= 256 || count <= 64) {
      size_t before = allocation_calls, temporary = data.native_record_allocations;
      for (size_t iteration = 0; iteration < 128; ++iteration) {
        assert(!rinzu_native_record_prepare(&data, arguments, 8, &loan));
        assert(loan->link.count == targets);
        assert(rinzu_native_record_argument_pointer(&data, loan, 0) ==
               rinzu_native_record_argument_pointer(&data, loan, targets));
        complete(&data, loan);
      }
      assert(allocation_calls == before && data.native_record_allocations == temporary);
      printf("NATIVE_RECORD_SCRATCH={\"case\":\"warm\",\"fields\":%zu,\"targets\":%zu,"
             "\"arguments\":8,\"cBytes\":%zu,\"iterations\":128,\"temporaryAllocationsPerCall\":0,"
             "\"totalAllocationsPerCall\":0,\"retainedBytes\":%zu,\"commonRootCacheBytes\":%zu}\n",
             count, targets, offset * targets, data.native_record_pool_bytes,
             data.frame_cache.bytes);
      assert(!data.published_root_count && !data.native_record_loans && !data.root_sets);
      assert(data.native_record_pool_bytes <= RINZU_NATIVE_RECORD_SCRATCH_LIMIT);
      /* Reentry uses separate blocks. Busy rejection itself cannot allocate. */
      rinzu_native_record_argument_t first = {handles[0], &layout}, second = {handles[1], &layout};
      rinzu_native_record_loan_t *outer = NULL, *inner = NULL;
      assert(!rinzu_native_record_prepare(&data, &first, 1, &outer));
      rinzu_native_record_enter(&data, outer);
      before = allocation_calls;
      assert(rinzu_native_record_prepare(&data, &first, 1, &inner) == RINZU_VALUE_BUSY && !inner);
      assert(allocation_calls == before);
      assert(!rinzu_native_record_prepare(&data, &second, 1, &inner));
      assert(rinzu_native_record_argument_pointer(&data, outer, 0) !=
             rinzu_native_record_argument_pointer(&data, inner, 0));
      data.gc.stress = 1;
      rinzu_managed_collect(&data, &roots);
      data.gc.stress = 0;
      complete(&data, inner);
      rinzu_native_record_returned(&data, outer);
      rinzu_native_record_commit(&data, outer);
      rinzu_native_record_release(&data, outer);
    } else {
      assert(!data.native_record_pool_bytes && !data.native_record_pool);
      printf(
          "NATIVE_RECORD_SCRATCH={\"case\":\"oversized\",\"fields\":%zu,\"idleRetainedBytes\":0}\n",
          count);
    }
  }
  cached_batches(&data);
  rinzu_managed_leave(&roots);
  data.gc.stress = 1;
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !data.published_root_count && !data.native_record_loans);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  assert(!data.native_record_pool && !data.native_record_pool_bytes);
  rinzu_value_schema_dispose(&schema);
  free(handles);
  free(slots);
  free(fields);
  free(field_types);
  assert(!live_bytes);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live);
}
int main(void) {
  atomic_store(&rinzu_resource_enabled, 1);
  size_t fields[] = {2, 4, 16, 64};
  for (size_t i = 0; i < 4; ++i)
    for (size_t targets = 1; targets <= 4; targets *= 2)
      exercise(fields[i], targets, 0);
  exercise(16384, 1, 0);
  exercise(4, 1, 512);
  assert(!live_bytes);
  puts("PASS C native scratch: allocation0, roots0, alias, GC, reentry, 64KiB, Domain bytes0");
}

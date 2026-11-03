#include <stdlib.h>
#include <stdint.h>
#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
static long rinzu_test_budget = -1;
static size_t rinzu_test_count, rinzu_test_bytes, rinzu_test_baseline_count,
    rinzu_test_baseline_bytes;
/* A private allocator oracle; no test bookkeeping allocation or production cap. */
static struct {
  void *pointer;
  size_t bytes;
} rinzu_test_allocations[1024];
static size_t rinzu_test_find(void *pointer) {
  for (size_t i = 0; i < 1024; ++i)
    if (rinzu_test_allocations[i].pointer == pointer)
      return i;
  return 1024;
}
static void rinzu_test_track(void *pointer, size_t bytes) {
  if (!pointer)
    return;
  size_t index = rinzu_test_find(NULL);
  if (index == 1024 || rinzu_test_find(pointer) != 1024)
    _Exit(90);
  rinzu_test_allocations[index].pointer = pointer;
  rinzu_test_allocations[index].bytes = bytes;
  ++rinzu_test_count;
  rinzu_test_bytes += bytes;
}
static void rinzu_test_untrack(void *pointer) {
  if (!pointer)
    return;
  size_t index = rinzu_test_find(pointer);
  if (index == 1024)
    return;
  --rinzu_test_count;
  rinzu_test_bytes -= rinzu_test_allocations[index].bytes;
  rinzu_test_allocations[index].pointer = NULL;
  rinzu_test_allocations[index].bytes = 0;
}
static int rinzu_test_allowed(void) {
  if (!rinzu_test_budget)
    return 0;
  if (rinzu_test_budget > 0)
    --rinzu_test_budget;
  return 1;
}
static void *rinzu_test_malloc(size_t bytes) {
  if (!rinzu_test_allowed())
    return NULL;
  void *pointer = malloc(bytes);
  rinzu_test_track(pointer, bytes);
  return pointer;
}
static void *rinzu_test_calloc(size_t count, size_t size) {
  if (!rinzu_test_allowed())
    return NULL;
  void *pointer = calloc(count, size);
  rinzu_test_track(pointer, count * size);
  return pointer;
}
static void *rinzu_test_realloc(void *pointer, size_t bytes) {
  if (!rinzu_test_allowed())
    return NULL;
  size_t index = pointer ? rinzu_test_find(pointer) : 1024;
  void *result = realloc(pointer, bytes);
  if (result || !bytes) {
    if (index != 1024) {
      --rinzu_test_count;
      rinzu_test_bytes -= rinzu_test_allocations[index].bytes;
      rinzu_test_allocations[index].pointer = NULL;
      rinzu_test_allocations[index].bytes = 0;
    }
    rinzu_test_track(result, bytes);
  }
  return result;
}
static void rinzu_test_free(void *pointer) {
  rinzu_test_untrack(pointer);
  free(pointer);
}
static void rinzu_test_arm(const char *name) {
  const char *value = getenv(name);
  rinzu_test_budget = value ? strtol(value, NULL, 10) : -1;
}
static void rinzu_test_marker(char marker) {
  const char *path = getenv("RINZU_TEST_USERDATA_ENTRY");
  if (!path)
    return;
  int file = open(path, O_WRONLY | O_CREAT | O_APPEND, 0600);
  if (file < 0 || write(file, &marker, 1) != 1 || close(file))
    _Exit(90);
}
static int rinzu_test_aligned(void **pointer, size_t alignment, size_t bytes) {
  if (!rinzu_test_allowed()) {
    *pointer = NULL;
    return 12;
  }
  int status = posix_memalign(pointer, alignment, bytes);
  if (!status)
    rinzu_test_track(*pointer, bytes);
  return status;
}
#define posix_memalign rinzu_test_aligned
#define malloc rinzu_test_malloc
#define calloc rinzu_test_calloc
#define realloc rinzu_test_realloc
#define free rinzu_test_free

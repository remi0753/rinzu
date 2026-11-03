#include "api.h"
#include <CoreFoundation/CoreFoundation.h>
#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
enum { THREADS = 8 };
static pthread_t owner, threads[THREADS];
static ForeignScalar scalar;
static ForeignUnit unit;
static ForeignText text, failed_text;
static ForeignBytes bytes;
static int32_t (*failed)(const char *);
static const uint8_t *(*failed_bytes)(const char *);
static uint64_t (*core)(const uint8_t *);
static int32_t (*unsupported)(const void *);
static _Atomic size_t finished;
static int started, active, mode, rounds, after_return_bodies;
static char *held_text[THREADS];
static uint8_t *held_bytes[THREADS];
static void *protected_result;
static size_t protected_size;
static const char input[] = "foreign 日本語";
static const uint8_t data[4] = {0, 0xaa, 0, 0x7f};
static void main_owner(void) {
  assert(pthread_equal(owner, pthread_self()));
}
void foreign_reset(int32_t process_main) {
  assert(!active && !rounds);
  owner = pthread_self();
  if (process_main)
    assert(pthread_main_np());
}
void foreign_body_marker(void) {
  fputs("FOREIGN_REJECTED_BODY\n", stderr);
  fflush(stderr);
  abort();
}
void foreign_after_return(void) {
  main_owner();
  assert(!active && rounds == 1);
  after_return_bodies++;
}
static void *late(void *raw) {
  (void)raw;
  (void)text("closed");
  abort();
}
static void *negative(void *raw) {
  (void)raw;
  (void)text(input);
  abort();
}
void foreign_nested(void) {
  main_owner();
  assert(active);
  pthread_t thread;
  assert(!pthread_create(&thread, NULL, negative, NULL));
  assert(!pthread_join(thread, NULL));
  abort();
}
int32_t foreign_ordinary(ForeignText callback) {
  text = callback;
  pthread_t thread;
  assert(!pthread_create(&thread, NULL, negative, NULL));
  assert(!pthread_join(thread, NULL));
  abort();
}
static void *invoke(void *raw) {
  size_t index = (size_t)raw;
  assert(!pthread_equal(owner, pthread_self()));
  if (mode == 1) {
    (void)unsupported((void *)(uintptr_t)1);
    abort();
  }
  if (mode == 3) {
    (void)text(input);
    abort();
  }
  if (mode == 4 || mode == 5) {
    const char *returned = text("settled 日本語");
    assert(returned && !strcmp(returned, "foreign result 🧭"));
    size_t length = strlen(returned) + 1;
    held_text[index] = malloc(length);
    held_bytes[index] = malloc(4);
    if (!held_text[index] || !held_bytes[index])
      _Exit(71);
    memcpy(held_text[index], returned, length);
    memcpy(held_bytes[index], data, 4);
    atomic_fetch_add(&finished, 1);
    return NULL;
  }
  assert(scalar(-17, -2001, -1234567, -4000000000LL, 255, 65535, UINT32_MAX, UINT64_MAX, -0.0f,
                42.5, true, input, data) == 42.5);
  unit("unit 日本語", NULL);
  const char *returned = text(input);
  assert(returned && !strcmp(returned, "foreign result 🧭"));
  size_t length = strlen(returned) + 1;
  held_text[index] = malloc(length);
  if (!held_text[index])
    _Exit(71);
  memcpy(held_text[index], returned, length);
  const uint8_t *result = bytes(data);
  assert(result && !memcmp(result, data, 4));
  held_bytes[index] = malloc(4);
  if (!held_bytes[index])
    _Exit(71);
  memcpy(held_bytes[index], result, 4);
  assert(failed(NULL) == -7);
  assert(failed("body-failure") == -7);
  static const char invalid[] = {(char)0xc0, (char)0xaf, 0};
  const char *replacement = failed_text(invalid);
  assert(replacement && !strcmp(replacement, "fallback 日本語"));
  const uint8_t *replacement_bytes = failed_bytes(invalid);
  const uint8_t expected[4] = {0xfe, 0, 0, 0};
  assert(replacement_bytes && !memcmp(replacement_bytes, expected, 4));
  assert(core(data) == 4);
  atomic_fetch_add(&finished, 1);
  return NULL;
}
static void tick(CFRunLoopTimerRef timer, void *context) {
  (void)context;
  main_owner();
  if (!started) {
    started = 1;
    if (mode == 2) {
      unit("nested", NULL);
      abort();
    }
    for (size_t index = 0; index < THREADS; index++)
      assert(!pthread_create(&threads[index], NULL, invoke, (void *)index));
  }
  if (atomic_load(&finished) == THREADS) {
    CFRunLoopTimerInvalidate(timer);
    CFRunLoopStop(CFRunLoopGetCurrent());
  }
}
int32_t foreign_run(ForeignScalar a, ForeignUnit b, ForeignText c, ForeignBytes d,
                    int32_t (*e)(const char *), ForeignText f, const uint8_t *(*g)(const char *),
                    uint64_t (*h)(const uint8_t *), int32_t (*i)(const void *), int32_t selected) {
  main_owner();
  assert(!active);
  if ((selected >= 1 && selected <= 3) || selected == 5)
    alarm(20);
  active = 1;
  mode = selected;
  scalar = a;
  unit = b;
  text = c;
  bytes = d;
  failed = e;
  failed_text = f;
  failed_bytes = g;
  core = h;
  unsupported = i;
  started = 0;
  atomic_store(&finished, 0);
  CFRunLoopTimerContext timer_context = {0};
  CFRunLoopTimerRef timer = CFRunLoopTimerCreate(NULL, CFAbsoluteTimeGetCurrent() + 0.001, 0.001, 0,
                                                 0, tick, &timer_context);
  assert(timer);
  CFStringRef run_mode = mode == 3 ? CFSTR("foreign-unsupported") : kCFRunLoopDefaultMode;
  CFRunLoopAddTimer(CFRunLoopGetCurrent(), timer, run_mode);
  fprintf(stderr, "FOREIGN_NATIVE_ENTER\n");
  (void)CFRunLoopRunInMode(run_mode, 10, 0);
  if (mode == 4 || mode == 5) {
    assert(!atomic_load(&finished));
    assert(!after_return_bodies);
  } else {
    assert(atomic_load(&finished) == THREADS);
    for (size_t index = 0; index < THREADS; index++)
      assert(!pthread_join(threads[index], NULL));
  }
  CFRunLoopTimerInvalidate(timer);
  CFRelease(timer);
  active = 0;
  rounds++;
  fputs("FOREIGN_NATIVE_RETURN\n", stderr);
  return 23;
}
const char *foreign_run_protected(ForeignScalar a, ForeignUnit b, ForeignText c, ForeignBytes d,
                                  int32_t (*e)(const char *), ForeignText f,
                                  const uint8_t *(*g)(const char *), uint64_t (*h)(const uint8_t *),
                                  int32_t (*i)(const void *), int32_t selected) {
  assert(!protected_result);
  protected_size = (size_t)sysconf(_SC_PAGESIZE);
  protected_result = mmap(NULL, protected_size, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
  if (protected_result == MAP_FAILED)
    _Exit(71);
  assert(foreign_run(a, b, c, d, e, f, g, h, i, selected) == 23);
  return protected_result;
}
int32_t foreign_finish(void) {
  main_owner();
  assert(!active);
  if (mode == 4 || mode == 5) {
    assert(after_return_bodies == THREADS);
    for (size_t index = 0; index < THREADS; index++)
      assert(!pthread_join(threads[index], NULL));
    assert(atomic_load(&finished) == THREADS);
    if (mode == 5) {
      pthread_t thread;
      assert(!pthread_create(&thread, NULL, late, NULL));
      assert(!pthread_join(thread, NULL));
      abort();
    }
  }
  for (size_t index = 0; index < THREADS; index++) {
    assert(held_text[index] && !strcmp(held_text[index], "foreign result 🧭"));
    assert(held_bytes[index] && !memcmp(held_bytes[index], data, 4));
    free(held_text[index]);
    free(held_bytes[index]);
    held_text[index] = NULL;
    held_bytes[index] = NULL;
  }
  if (protected_result) {
    assert(!munmap(protected_result, protected_size));
    protected_result = NULL;
  }
  fputs("FOREIGN_FINISH_AFTER_RETURN\n", stderr);
  return rounds;
}

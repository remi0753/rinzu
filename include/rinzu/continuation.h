#ifndef RINZU_CONTINUATION_H
#define RINZU_CONTINUATION_H
#include "rinzu/failure.h"
#include "rinzu/thread_identity.h"
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

/* Build-time native code, never writable executable memory. The initial
   backend preserves the target's complete nonvolatile register set. Managed
   roots are published separately; native stacks are not conservative roots. */
#if !defined(__APPLE__) || !defined(__aarch64__)
#error "Rinzu native continuations require the macOS arm64 backend"
#endif
typedef struct {
  uint64_t x[12]; /* x19..x30, including frame pointer and return address */
  uint64_t sp, fpcr, fpsr;
  uint64_t d[8]; /* nonvolatile low halves of v8..v15 */
} rinzu_continuation_registers_t;
_Static_assert(offsetof(rinzu_continuation_registers_t, sp) == 96, "continuation SP layout");
_Static_assert(offsetof(rinzu_continuation_registers_t, d) == 120, "continuation SIMD layout");
void rinzu_continuation_switch(rinzu_continuation_registers_t *,
                               const rinzu_continuation_registers_t *);
void rinzu_continuation_bootstrap(void);
__asm__(".text\n.p2align 2\n"
        "_rinzu_continuation_switch:\n"
        "stp x19, x20, [x0, #0]\n"
        "stp x21, x22, [x0, #16]\n"
        "stp x23, x24, [x0, #32]\n"
        "stp x25, x26, [x0, #48]\n"
        "stp x27, x28, [x0, #64]\n"
        "stp x29, x30, [x0, #80]\n"
        "mov x9, sp\nstr x9, [x0, #96]\n"
        "mrs x9, fpcr\nstr x9, [x0, #104]\n"
        "mrs x9, fpsr\nstr x9, [x0, #112]\n"
        "stp d8, d9, [x0, #120]\n"
        "stp d10, d11, [x0, #136]\n"
        "stp d12, d13, [x0, #152]\n"
        "stp d14, d15, [x0, #168]\n"
        "ldp x19, x20, [x1, #0]\n"
        "ldp x21, x22, [x1, #16]\n"
        "ldp x23, x24, [x1, #32]\n"
        "ldp x25, x26, [x1, #48]\n"
        "ldp x27, x28, [x1, #64]\n"
        "ldp x29, x30, [x1, #80]\n"
        "ldr x9, [x1, #96]\nmov sp, x9\n"
        "ldr x9, [x1, #104]\nmsr fpcr, x9\n"
        "ldr x9, [x1, #112]\nmsr fpsr, x9\n"
        "ldp d8, d9, [x1, #120]\n"
        "ldp d10, d11, [x1, #136]\n"
        "ldp d12, d13, [x1, #152]\n"
        "ldp d14, d15, [x1, #168]\n"
        "ret\n"
        "_rinzu_continuation_bootstrap:\n"
        "mov x0, x19\nblr x20\nbrk #0\n");

typedef struct rinzu_continuation_stack {
  struct rinzu_continuation_stack *next;
  void *mapping;
  size_t mapping_size;
} rinzu_continuation_stack_t;
typedef struct {
  /* live counts initialized continuations, including those not yet resumed;
     mapped_bytes counts physical mappings, both active and cached. */
  size_t live, peak_live, mapped_bytes, peak_mapped_bytes;
  pthread_t owner;
  size_t page_bytes, cache_limit, cached_bytes, allocations, reuses;
  rinzu_continuation_stack_t *cached;
} rinzu_continuation_accounting_t;
typedef struct rinzu_continuation {
  rinzu_continuation_registers_t registers;
  pthread_t owner;
  void *mapping;
  size_t mapping_size;
  void (*body)(void *);
  void *argument;
  struct rinzu_continuation *return_to;
  rinzu_continuation_accounting_t *accounting;
  int started, finished, running;
} rinzu_continuation_t;

static inline void rinzu_continuation_owner(const rinzu_continuation_t *continuation) {
  if (!rinzu_thread_equal(continuation->owner, pthread_self()))
    rinzu_failure_fatal();
}
static inline void rinzu_continuation_init_root(rinzu_continuation_t *root) {
  memset(root, 0, sizeof(*root));
  root->owner = pthread_self();
  root->running = 1;
}
static inline void rinzu_continuation_yield(rinzu_continuation_t *continuation) {
  rinzu_continuation_owner(continuation);
  rinzu_continuation_t *caller = continuation->return_to;
  if (!continuation->running || !caller || caller->running)
    rinzu_failure_fatal();
  rinzu_continuation_owner(caller);
  continuation->running = 0;
  caller->running = 1;
  rinzu_continuation_switch(&continuation->registers, &caller->registers);
}
static void rinzu_continuation_run(void *raw) {
  rinzu_continuation_t *continuation = raw;
  rinzu_continuation_owner(continuation);
  continuation->body(continuation->argument);
  continuation->finished = 1;
  rinzu_continuation_yield(continuation);
  rinzu_failure_fatal();
}
static inline void rinzu_continuation_init(rinzu_continuation_t *continuation, size_t stack_bytes,
                                           void (*body)(void *), void *argument,
                                           rinzu_continuation_accounting_t *accounting) {
  memset(continuation, 0, sizeof(*continuation));
  continuation->owner = pthread_self();
  if (!body || !accounting || !stack_bytes)
    rinzu_failure_fatal();
  if (!accounting->page_bytes) {
    long page = sysconf(_SC_PAGESIZE);
    if (page <= 0)
      rinzu_failure_fatal();
    accounting->page_bytes = (size_t)page;
    accounting->owner = pthread_self();
    /* This bounds idle memory only, never task admission or live stacks. */
    accounting->cache_limit = 8 * 1024 * 1024;
    const char *setting = getenv("RINZU_CONTINUATION_CACHE_BYTES");
    if (setting) {
      size_t limit = 0;
      if (!*setting)
        rinzu_failure_fatal();
      for (; *setting; ++setting) {
        if (*setting < '0' || *setting > '9' || limit > (SIZE_MAX - (*setting - '0')) / 10)
          rinzu_failure_fatal();
        limit = limit * 10 + (*setting - '0');
      }
      accounting->cache_limit = limit;
    }
  }
  if (!rinzu_thread_equal(accounting->owner, pthread_self()))
    rinzu_failure_fatal();
  size_t page_bytes = accounting->page_bytes;
  if (stack_bytes > SIZE_MAX - page_bytes + 1)
    rinzu_failure_fatal();
  stack_bytes = ((stack_bytes + page_bytes - 1) / page_bytes) * page_bytes;
  if (page_bytes > SIZE_MAX / 2 || stack_bytes > SIZE_MAX - 2 * page_bytes)
    rinzu_failure_fatal();
  size_t mapping_size = stack_bytes + 2 * page_bytes;
  if (accounting->live == SIZE_MAX || mapping_size > SIZE_MAX - accounting->mapped_bytes)
    rinzu_failure_fatal();
  /* Admission keeps the body/arguments and its initial floating environment.
     Materialize a guarded stack only on the first actual resume, when another
     completed continuation may already have returned one to the cache. */
  continuation->mapping_size = mapping_size;
  continuation->body = body;
  continuation->argument = argument;
  continuation->accounting = accounting;
  continuation->registers.x[0] = (uint64_t)(uintptr_t)continuation;
  continuation->registers.x[1] = (uint64_t)(uintptr_t)rinzu_continuation_run;
  continuation->registers.x[11] = (uint64_t)(uintptr_t)rinzu_continuation_bootstrap;
  __asm__ volatile("mrs %0, fpcr" : "=r"(continuation->registers.fpcr));
  __asm__ volatile("mrs %0, fpsr" : "=r"(continuation->registers.fpsr));
  accounting->live += 1;
  if (accounting->live > accounting->peak_live)
    accounting->peak_live = accounting->live;
}
static inline void rinzu_continuation_prepare_stack(rinzu_continuation_t *continuation) {
  rinzu_continuation_accounting_t *accounting = continuation->accounting;
  if (continuation->mapping || continuation->started || !continuation->body || !accounting ||
      !accounting->page_bytes || !accounting->live ||
      !rinzu_thread_equal(accounting->owner, pthread_self()))
    rinzu_failure_fatal();
  size_t mapping_size = continuation->mapping_size, page_bytes = accounting->page_bytes;
  if (page_bytes > SIZE_MAX / 2 || mapping_size <= 2 * page_bytes ||
      mapping_size > SIZE_MAX - accounting->mapped_bytes)
    rinzu_failure_fatal();
  size_t stack_bytes = mapping_size - 2 * page_bytes;
  void *mapping = NULL;
  rinzu_continuation_stack_t **cached = &accounting->cached;
  while (*cached && (*cached)->mapping_size != mapping_size)
    cached = &(*cached)->next;
  if (*cached) {
    rinzu_continuation_stack_t *entry = *cached;
    mapping = entry->mapping;
    *cached = entry->next;
    accounting->cached_bytes -= mapping_size;
    accounting->reuses++;
  } else {
    mapping = mmap(NULL, mapping_size, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
    if (mapping == MAP_FAILED)
      rinzu_failure_fatal();
    if (mprotect((unsigned char *)mapping + page_bytes, stack_bytes, PROT_READ | PROT_WRITE))
      rinzu_failure_fatal();
    accounting->mapped_bytes += mapping_size;
    accounting->allocations++;
  }
  continuation->mapping = mapping;
  continuation->registers.sp = ((uintptr_t)mapping + page_bytes + stack_bytes) & ~(uintptr_t)15;
  if (accounting->mapped_bytes > accounting->peak_mapped_bytes)
    accounting->peak_mapped_bytes = accounting->mapped_bytes;
}
static inline void rinzu_continuation_resume(rinzu_continuation_t *caller,
                                             rinzu_continuation_t *continuation) {
  rinzu_continuation_owner(caller);
  rinzu_continuation_owner(continuation);
  if (!caller->running || continuation->running || continuation->finished)
    rinzu_failure_fatal();
  if (!continuation->mapping)
    rinzu_continuation_prepare_stack(continuation);
  continuation->return_to = caller;
  continuation->started = 1;
  caller->running = 0;
  continuation->running = 1;
  rinzu_continuation_switch(&caller->registers, &continuation->registers);
}
static inline void rinzu_continuation_dispose(rinzu_continuation_t *continuation) {
  rinzu_continuation_owner(continuation);
  if (continuation->running ||
      (continuation->started && (!continuation->finished || !continuation->mapping)))
    rinzu_failure_fatal();
  rinzu_continuation_accounting_t *accounting = continuation->accounting;
  if (!accounting || !accounting->live || !continuation->body || !continuation->mapping_size ||
      !rinzu_thread_equal(accounting->owner, pthread_self()))
    rinzu_failure_fatal();
  accounting->live -= 1;
  if (!continuation->mapping) {
    *continuation = (rinzu_continuation_t){0};
    return;
  }
  if (accounting->mapped_bytes < continuation->mapping_size)
    rinzu_failure_fatal();
  if (continuation->mapping_size <= accounting->cache_limit - accounting->cached_bytes) {
    /* The body has returned and its typed roots/native scopes are retired by
       the caller. Stale stack bytes are never roots. Keep both guards intact;
       no pointer or register from this continuation is reused as task state. */
    rinzu_continuation_stack_t *entry =
        (void *)((unsigned char *)continuation->mapping + accounting->page_bytes);
    *entry = (rinzu_continuation_stack_t){accounting->cached, continuation->mapping,
                                          continuation->mapping_size};
    accounting->cached = entry;
    accounting->cached_bytes += continuation->mapping_size;
  } else {
    if (munmap(continuation->mapping, continuation->mapping_size))
      rinzu_failure_fatal();
    accounting->mapped_bytes -= continuation->mapping_size;
  }
  *continuation = (rinzu_continuation_t){0};
}
static inline void rinzu_continuation_cache_dispose(rinzu_continuation_accounting_t *accounting) {
  if (accounting->live ||
      (accounting->page_bytes && !rinzu_thread_equal(accounting->owner, pthread_self())))
    rinzu_failure_fatal();
  while (accounting->cached) {
    rinzu_continuation_stack_t entry = *accounting->cached;
    accounting->cached = entry.next;
    if (munmap(entry.mapping, entry.mapping_size))
      rinzu_failure_fatal();
    accounting->mapped_bytes -= entry.mapping_size;
    accounting->cached_bytes -= entry.mapping_size;
  }
  if (accounting->mapped_bytes || accounting->cached_bytes)
    rinzu_failure_fatal();
}
#endif

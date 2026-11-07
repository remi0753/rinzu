#include <assert.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef struct {
  uint64_t file;
  uint32_t start, end, present;
} Source;
typedef struct {
  uint64_t sequence, owner, failures;
} Events;
typedef struct {
  unsigned depth;
  struct {
    uint64_t primary;
  } current;
  Events events;
} Pending;
typedef struct {
  int explicit_shutdown;
} Async;
typedef struct {
  _Atomic int cancel_requested;
} Worker;
typedef struct {
  uint64_t polls, *domain_polls, shutdown_after, cancel_after;
  int shutdown_requested, cancel_requested, cancel_masked;
  unsigned callback_depth;
  Pending pending;
  Source source;
  Async *async;
  Worker *worker;
} RinzuContext;
static uint64_t atomic_reads;
#undef atomic_load_explicit
#define atomic_load_explicit(p, order) (++atomic_reads, __c11_atomic_load((p), (order)))
static void rinzu_failure_fatal(void) {
  _Exit(70);
}
static void *rinzu_context_transitions(RinzuContext *c) {
  return &c->pending.events;
}
static void rinzu_owner_request_shutdown(void *raw) {
  Events *e = raw;
  e->owner++;
  e->sequence = e->sequence * 17 + 1;
}
static void rinzu_failure_fail(Pending *p, int cause, Source s) {
  p->events.failures++;
  p->events.sequence = p->events.sequence * 17 + (unsigned)cause + s.start;
  if (!p->current.primary)
    p->current.primary = (unsigned)cause;
}
/* CURRENT_ASYNC_HELPERS */
#ifdef RINZU_TOOLING_POLL_ASYNC_HELPERS
#include RINZU_TOOLING_POLL_ASYNC_HELPERS
#endif
static inline int64_t poll_original(RinzuContext *context) {
  uint64_t *polls = context->domain_polls ? context->domain_polls : &context->polls;
  if (*polls == UINT64_MAX)
    rinzu_failure_fatal();
  *polls += 1;
  if (rinzu_aot_async_root_shutdown(context))
    context->shutdown_requested = 1;
  if (context->shutdown_after && *polls >= context->shutdown_after)
    context->shutdown_requested = 1;
  if (rinzu_aot_async_cancelled(context) ||
      (context->cancel_after && *polls >= context->cancel_after))
    context->cancel_requested = 1;
  if (context->shutdown_requested || context->cancel_requested)
    rinzu_owner_request_shutdown(rinzu_context_transitions(context));
  if (context->pending.depth || context->cancel_masked)
    return 0;
  if (context->callback_depth)
    return context->pending.current.primary != 0;
  if (context->shutdown_requested)
    rinzu_failure_fail(&context->pending, 4, context->source);
  if (context->cancel_requested)
    rinzu_failure_fail(&context->pending, 2, context->source);
  return context->pending.current.primary != 0;
}
/* CURRENT_POLL_BODY */
#ifdef RINZU_TOOLING_POLL_BODY
#include RINZU_TOOLING_POLL_BODY
#endif

int main(int argc, char **argv) {
  if (argc > 1) {
    RinzuContext c = {.polls = UINT64_MAX};
    if (!strcmp(argv[1], "old"))
      return (int)poll_original(&c);
    return (int)poll_candidate(&c);
  }
  uint64_t starts[] = {0, 1, 6, 7, UINT64_MAX - 1}, limits[] = {0, 1, 7, 8, UINT64_MAX};
  uint64_t cases = 0;
  for (unsigned flags = 0; flags < 2048; ++flags)
    for (unsigned pi = 0; pi < 5; ++pi)
      for (unsigned ci = 0; ci < 5; ++ci)
        for (unsigned si = 0; si < 5; ++si) {
          Async a = {.explicit_shutdown = !!(flags & 512)};
          Worker w;
          atomic_init(&w.cancel_requested, !!(flags & 128));
          RinzuContext c;
          memset(&c, 0, sizeof(c));
          c.polls = starts[pi];
          c.cancel_after = limits[ci];
          c.shutdown_after = limits[si];
          c.cancel_requested = (flags & 1) ? -1 : 0;
          c.shutdown_requested = (flags & 2) ? 7 : 0;
          c.pending.depth = (flags & 4) ? 3 : 0;
          c.cancel_masked = (flags & 8) ? -1 : 0;
          c.callback_depth = (flags & 16) ? 2 : 0;
          c.pending.current.primary = (flags & 32) ? 9 : 0;
          c.worker = (flags & 64) ? &w : NULL;
          c.async = (flags & 256) ? &a : NULL;
          c.source = (Source){31, 57, 79, 1};
          RinzuContext old = c, live = c;
          uint64_t op = starts[pi], lp = starts[pi];
          if (flags & 1024) {
            old.domain_polls = &op;
            live.domain_polls = &lp;
          }
          atomic_reads = 0;
          int64_t ov = poll_original(&old);
          uint64_t oa = atomic_reads;
          atomic_reads = 0;
          int64_t lv = poll_candidate(&live);
          uint64_t la = atomic_reads;
          old.domain_polls = live.domain_polls = NULL;
          assert(ov == lv && oa == la && op == lp && !memcmp(&old, &live, sizeof(old)));
          ++cases;
        }
  assert(cases == 256000);
  printf("N21:256000-control-states:events-source-polls-worker-atomic:OK\n");
  return 0;
}

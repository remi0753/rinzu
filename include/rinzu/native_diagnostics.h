#ifndef RINZU_NATIVE_DIAGNOSTICS_H
#define RINZU_NATIVE_DIAGNOSTICS_H
#include "rinzu/thread_identity.h"
#include "rinzu/foreign_queue.h"
#include <stdio.h>
#include <poll.h>
#include <fcntl.h>
#include <stddef.h>
#ifdef __APPLE__
#include <mach/mach_time.h>
#endif

/* Private development diagnostics. Only native metadata is published here;
   the watchdog cannot touch a managed heap, root, frame or callable address. */
enum {
  RINZU_ACTIVITY_MANAGED,
  RINZU_ACTIVITY_NATIVE,
  RINZU_ACTIVITY_FOREIGN_WAIT,
  RINZU_ACTIVITY_OWNER_STALL,
  RINZU_ACTIVITY_SHUTDOWN,
  RINZU_ACTIVITY_KINDS
};
typedef struct rinzu_activity_board rinzu_activity_board_t;
typedef struct rinzu_activity rinzu_activity_t;
typedef struct rinzu_wait_dependency rinzu_wait_dependency_t;
typedef struct {
  uint64_t prepared, ended, warnings[RINZU_ACTIVITY_KINDS];
  size_t live, active, bytes, warning_pins;
  size_t ready_live, ready_bytes;
  size_t wait_live, wait_bytes;
  /* Retired native storage, distinct from live diagnostic intervals. */
  size_t cached_records, cached_bytes;
} rinzu_activity_stats_t;
#if UINTPTR_MAX == UINT64_MAX
_Static_assert(sizeof(rinzu_activity_stats_t) == 136 &&
                   offsetof(rinzu_activity_stats_t, cached_records) == 120 &&
                   offsetof(rinzu_activity_stats_t, cached_bytes) == 128,
               "ActivityStats C/Rust cache fields must match");
#endif
struct rinzu_wait_dependency {
  rinzu_activity_board_t *board;
  rinzu_wait_dependency_t *next;
  uint64_t task, dependency, file, owner, depth;
  uint32_t start_byte, end_byte;
  size_t bytes;
  char symbol[];
};
struct rinzu_activity {
  rinzu_activity_board_t *board;
  rinzu_activity_t *next;
  /* Gate-protected membership slot, independent of the emitter queue/pin. */
  rinzu_activity_t **link;
  rinzu_activity_t *pending_next;
  rinzu_activity_t *blocker;
  pthread_t creator;
  uint64_t identity, started, owner, depth, file;
  uint32_t start_byte, end_byte;
  size_t references, bytes, message_length;
  unsigned kind, active, warned;
  /* A bounded diagnostic line is not a limit on invocations or symbols. The
     original symbol is retained at its full length in this prepared record. */
  char message[512];
  char symbol[];
};
#ifndef RINZU_ACTIVITY_CACHE_CAPACITY
#define RINZU_ACTIVITY_CACHE_CAPACITY 64
#endif
#ifndef RINZU_ACTIVITY_CACHE_PREFIX_RESET
#define RINZU_ACTIVITY_CACHE_PREFIX_RESET 1
#endif
#define RINZU_ACTIVITY_CACHE_SLOT_BYTES 768
_Static_assert(sizeof(rinzu_activity_t) < RINZU_ACTIVITY_CACHE_SLOT_BYTES,
               "activity class must hold its prefix and symbol terminator");
_Static_assert(RINZU_ACTIVITY_CACHE_CAPACITY <= SIZE_MAX / RINZU_ACTIVITY_CACHE_SLOT_BYTES,
               "retained activity bytes must not overflow");
#define RINZU_ACTIVITY_WAIT_INDEXED 1
#ifndef RINZU_ACTIVITY_WAIT_INLINE_BUCKETS
#define RINZU_ACTIVITY_WAIT_INLINE_BUCKETS 128
#endif
_Static_assert(RINZU_ACTIVITY_WAIT_INLINE_BUCKETS >= 1 &&
                   !(RINZU_ACTIVITY_WAIT_INLINE_BUCKETS & (RINZU_ACTIVITY_WAIT_INLINE_BUCKETS - 1)),
               "wait inline buckets must be a nonzero power of two");
struct rinzu_activity_board {
  pthread_mutex_t gate;
  _Atomic unsigned gate_lease;
  pthread_cond_t gate_handoff;
  pthread_cond_t changed;
  pthread_t watchdog;
  pthread_t owner;
  uint64_t owner_turn;
  unsigned shutdown_requested;
  rinzu_activity_t *head, *pending, *pending_tail;
  rinzu_wait_dependency_t **waits;
  rinzu_activity_stats_t stats;
  int stop;
#ifdef RINZU_ACTIVITY_TESTING
  rinzu_foreign_notify_t before_emit;
  void *test_context;
#endif
  size_t wait_capacity;
  rinzu_wait_dependency_t *inline_waits[RINZU_ACTIVITY_WAIT_INLINE_BUCKETS];
  rinzu_activity_t *cached_records;
  /* Immutable clock units for this board. Zero denom keeps nanosecond mode,
     including manually owned boards. Darwin keeps libc's exact modular
     product before its division; no timestamp is cached or shared. */
  uint32_t clock_numer, clock_denom;
  uint64_t clock_not_due[RINZU_ACTIVITY_KINDS];
};
/* The fast lease and physical mutex protect the same board-data critical section.
   0 idle; 1 fast lease; 2 physical mutex/reserved slow path; 3 handoff requested.
   Only the lease holder or physical-mutex holder may inspect board data. */
#ifndef RINZU_ACTIVITY_GATE_LEASE
#define RINZU_ACTIVITY_GATE_LEASE (ATOMIC_INT_LOCK_FREE == 2)
#endif
static inline void rinzu_activity_gate_init(rinzu_activity_board_t *board) {
  rinzu_foreign_sync(pthread_mutex_init(&board->gate, NULL));
#if RINZU_ACTIVITY_GATE_LEASE
  atomic_init(&board->gate_lease, 0);
  rinzu_foreign_sync(pthread_cond_init(&board->gate_handoff, NULL));
#endif
}
/* The physical mutex is already held. A waiter releases it while the fast
   holder finishes; every wake reclaims bit2 before any shared-data access. */
static inline void rinzu_activity_gate_claim_locked(rinzu_activity_board_t *board) {
#if RINZU_ACTIVITY_GATE_LEASE
  for (;;) {
    unsigned previous = atomic_fetch_or_explicit(&board->gate_lease, 2, memory_order_acquire);
    if (previous > 3)
      _Exit(86);
    if (!(previous & 1))
      return;
    rinzu_foreign_sync(pthread_cond_wait(&board->gate_handoff, &board->gate));
  }
#else
  (void)board;
#endif
}
static inline void rinzu_activity_gate_lock_slow(rinzu_activity_board_t *board) {
  rinzu_foreign_lock(&board->gate);
  rinzu_activity_gate_claim_locked(board);
}
static inline void rinzu_activity_gate_lock(rinzu_activity_board_t *board) {
#if RINZU_ACTIVITY_GATE_LEASE
  unsigned expected = 0;
  if (atomic_compare_exchange_strong_explicit(&board->gate_lease, &expected, 1,
                                              memory_order_acquire, memory_order_relaxed)) {
#ifdef RINZU_FOREIGN_TESTING
    ++rinzu_foreign_held_locks;
#endif
    return;
  }
#endif
  rinzu_activity_gate_lock_slow(board);
}
/* Retain logical ownership while adding the physical mutex. It protects the
   condition-wait gap, so no signal can precede a waiter's atomic unlock/wait.
   Direct pthread locking here avoids counting one logical gate twice in tests. */
static inline void rinzu_activity_gate_promote(rinzu_activity_board_t *board) {
#if RINZU_ACTIVITY_GATE_LEASE
  unsigned state = atomic_load_explicit(&board->gate_lease, memory_order_relaxed);
  if (state == 2)
    return;
  if (state != 1 && state != 3)
    _Exit(86);
  if (pthread_mutex_lock(&board->gate))
    _Exit(86);
  state = atomic_exchange_explicit(&board->gate_lease, 2, memory_order_relaxed);
  if (state != 1 && state != 3)
    _Exit(86);
  if (state == 3)
    rinzu_foreign_sync(pthread_cond_broadcast(&board->gate_handoff));
#else
  (void)board;
#endif
}
static inline void rinzu_activity_gate_unlock(rinzu_activity_board_t *board) {
#if RINZU_ACTIVITY_GATE_LEASE
  unsigned expected = 1;
  if (atomic_compare_exchange_strong_explicit(&board->gate_lease, &expected, 0,
                                              memory_order_release, memory_order_relaxed)) {
#ifdef RINZU_FOREIGN_TESTING
    if (!rinzu_foreign_held_locks)
      _Exit(86);
    --rinzu_foreign_held_locks;
#endif
    return;
  }
  if (expected == 3) {
    if (pthread_mutex_lock(&board->gate))
      _Exit(86);
    if (atomic_exchange_explicit(&board->gate_lease, 2, memory_order_release) != 3)
      _Exit(86);
    rinzu_foreign_sync(pthread_cond_broadcast(&board->gate_handoff));
    /* Keep the slow reservation until a woken waiter obtains the mutex. */
  } else if (expected == 2) {
    atomic_store_explicit(&board->gate_lease, 0, memory_order_release);
  } else
    _Exit(86);
#endif
  rinzu_foreign_unlock(&board->gate);
}
static inline void rinzu_activity_gate_signal(rinzu_activity_board_t *board) {
  rinzu_activity_gate_promote(board);
  rinzu_foreign_sync(pthread_cond_signal(&board->changed));
}
/* Only the watchdog calls these, while owning the physical mutex. Publishers
   may take a fast lease in the wait gap, but must promote before notifying.
   A waking watcher reclaims its lease before reading any board predicate. */
static inline void rinzu_activity_gate_before_wait(rinzu_activity_board_t *board) {
#if RINZU_ACTIVITY_GATE_LEASE
  if (atomic_exchange_explicit(&board->gate_lease, 0, memory_order_release) != 2)
    _Exit(86);
#else
  (void)board;
#endif
}
static inline void rinzu_activity_gate_dispose(rinzu_activity_board_t *board) {
#if RINZU_ACTIVITY_GATE_LEASE
  if (atomic_load_explicit(&board->gate_lease, memory_order_relaxed))
    _Exit(86);
  rinzu_foreign_sync(pthread_cond_destroy(&board->gate_handoff));
#endif
  rinzu_foreign_sync(pthread_mutex_destroy(&board->gate));
}

/* Only after watchdog join, or for a manually owned board with no watchdog.
   All logical records and warning pins must already have settled. */
static inline void rinzu_activity_cache_drain(rinzu_activity_board_t *board) {
  if (board->head || board->pending || board->stats.live || board->stats.active ||
      board->stats.warning_pins)
    _Exit(86);
  while (board->cached_records) {
    rinzu_activity_t *record = board->cached_records;
    board->cached_records = record->next;
    if (!board->stats.cached_records || board->stats.cached_bytes < RINZU_ACTIVITY_CACHE_SLOT_BYTES)
      _Exit(86);
    board->stats.cached_records--;
    board->stats.cached_bytes -= RINZU_ACTIVITY_CACHE_SLOT_BYTES;
    free(record);
  }
  if (board->stats.cached_records || board->stats.cached_bytes)
    _Exit(86);
}
static inline uint64_t rinzu_activity_now(void) {
  struct timespec time;
  rinzu_foreign_time(&time);
  if (time.tv_sec < 0 || (uint64_t)time.tv_sec > UINT64_MAX / UINT64_C(1000000000))
    _Exit(86);
  uint64_t seconds = (uint64_t)time.tv_sec * UINT64_C(1000000000);
  if ((uint64_t)time.tv_nsec > UINT64_MAX - seconds)
    _Exit(86);
  return seconds + (uint64_t)time.tv_nsec;
}
/* One outgoing wait per managed task. Only explicit runtime dependencies enter
   this graph; native locks/joins with unknown endpoints are never inferred. */
static inline size_t rinzu_activity_wait_bucket(uint64_t task, size_t capacity) {
  return (size_t)task & (capacity - 1);
}
static inline rinzu_wait_dependency_t *
rinzu_activity_wait_find_locked(rinzu_activity_board_t *board, uint64_t task) {
  size_t bucket = rinzu_activity_wait_bucket(task, board->wait_capacity);
  for (rinzu_wait_dependency_t *wait = board->waits[bucket]; wait; wait = wait->next)
    if (wait->task == task)
      return wait;
  return NULL;
}
/* Only the Owner mutates the graph. Temporarily release its publication gate
   for allocation; the watchdog can still report an overdue native interval.
   Expansion/shrink is optional: allocation failure keeps the correct old map.
   Return detached dynamic storage for the caller to free outside the gate. */
static inline rinzu_wait_dependency_t **
rinzu_activity_wait_resize_locked(rinzu_activity_board_t *board, size_t capacity,
                                  size_t reserved_bytes) {
  if (capacity == board->wait_capacity)
    return NULL;
  if (!capacity || (capacity & (capacity - 1)))
    _Exit(86);
  if (capacity > SIZE_MAX / sizeof(*board->waits))
    return NULL;
  rinzu_wait_dependency_t **old = board->waits;
  size_t old_capacity = board->wait_capacity;
  size_t old_bytes = old == board->inline_waits ? 0 : old_capacity * sizeof(*old);
  size_t new_bytes = capacity == RINZU_ACTIVITY_WAIT_INLINE_BUCKETS ? 0 : capacity * sizeof(*old);
  if (board->stats.wait_bytes < old_bytes)
    _Exit(86);
  size_t used = board->stats.wait_bytes - old_bytes;
  if (reserved_bytes > SIZE_MAX - used || new_bytes > SIZE_MAX - used - reserved_bytes)
    return NULL;
  rinzu_wait_dependency_t **index = board->inline_waits;
  if (new_bytes) {
    size_t live = board->stats.wait_live;
    rinzu_activity_gate_unlock(board);
    int saved_errno = errno;
    index = calloc(capacity, sizeof(*index));
    errno = saved_errno;
    rinzu_activity_gate_lock(board);
    if (board->stop || board->waits != old || board->wait_capacity != old_capacity ||
        board->stats.wait_live != live)
      _Exit(86);
    if (!index)
      return NULL;
  } else
    memset(index, 0, sizeof(board->inline_waits));
  for (size_t slot = 0; slot < old_capacity; ++slot) {
    rinzu_wait_dependency_t *wait = old[slot];
    while (wait) {
      rinzu_wait_dependency_t *next = wait->next;
      size_t bucket = rinzu_activity_wait_bucket(wait->task, capacity);
      wait->next = index[bucket];
      index[bucket] = wait;
      wait = next;
    }
  }
  if (old == board->inline_waits)
    memset(old, 0, sizeof(board->inline_waits));
  board->waits = index;
  board->wait_capacity = capacity;
  board->stats.wait_bytes = used + new_bytes;
  return old == board->inline_waits ? NULL : old;
}
static inline void rinzu_activity_diagnostic_write(const char *line, size_t bytes) {
  const char *path = getenv("RINZU_DIAGNOSTIC_OUTPUT");
  int fd = path ? open(path, O_WRONLY | O_CREAT | O_APPEND, 0600) : STDERR_FILENO;
  if (fd < 0)
    return;
  struct pollfd output = {.fd = fd, .events = POLLOUT};
  if (poll(&output, 1, 0) > 0 && (output.revents & POLLOUT))
    (void)write(fd, line, bytes);
  if (path)
    (void)close(fd);
}
static inline void rinzu_activity_foreign_unavailable_fatal(rinzu_activity_board_t *board,
                                                            const char *reason, const char *symbol,
                                                            uint64_t file, uint32_t start,
                                                            uint32_t end, uint64_t owner,
                                                            uint64_t depth) {
  if (!board || !reason || !symbol)
    _Exit(86);
  rinzu_activity_gate_lock(board);
  char line[512];
  int length = snprintf(line, sizeof(line),
                        "rinzu fatal: foreign-dispatch-unavailable reason=%s symbol=%.240s "
                        "source=%llu:%u:%u owner=0x%llx depth=%llu\n",
                        reason, symbol, (unsigned long long)file, start, end,
                        (unsigned long long)owner, (unsigned long long)depth);
  if (length < 0)
    _Exit(86);
  rinzu_activity_diagnostic_write(line, (size_t)length < sizeof(line) ? (size_t)length
                                                                      : sizeof(line) - 1);
  for (rinzu_activity_t *frame = board->head; frame; frame = frame->next) {
    if (!frame->active || frame->kind > RINZU_ACTIVITY_NATIVE || frame->owner != owner)
      continue;
    length = snprintf(line, sizeof(line), "rinzu foreign stack: symbol=%.240s\n", frame->symbol);
    if (length < 0)
      _Exit(86);
    rinzu_activity_diagnostic_write(line, (size_t)length < sizeof(line) ? (size_t)length
                                                                        : sizeof(line) - 1);
  }
  /* Known queue progress denial is not proof of an unknown native lock cycle. */
  _Exit(86);
}
static inline void rinzu_activity_wait_cycle_fatal_locked(rinzu_wait_dependency_t *closing) {
  static const char heading[] = "rinzu fatal: observable-wait-cycle\n";
  rinzu_activity_diagnostic_write(heading, sizeof(heading) - 1);
  rinzu_wait_dependency_t *wait = closing;
  do {
    char line[512];
    int length =
        snprintf(line, sizeof(line),
                 "rinzu wait: task=%llu dependency=%llu symbol=%.240s source=%llu:%u:%u "
                 "owner=0x%llx depth=%llu\n",
                 (unsigned long long)wait->task, (unsigned long long)wait->dependency, wait->symbol,
                 (unsigned long long)wait->file, wait->start_byte, wait->end_byte,
                 (unsigned long long)wait->owner, (unsigned long long)wait->depth);
    if (length < 0)
      _Exit(86);
    rinzu_activity_diagnostic_write(line, (size_t)length < sizeof(line) ? (size_t)length
                                                                        : sizeof(line) - 1);
    wait = rinzu_activity_wait_find_locked(closing->board, wait->dependency);
    if (!wait)
      _Exit(86);
  } while (wait != closing);
  for (rinzu_activity_t *frame = closing->board->head; frame; frame = frame->next) {
    if (!frame->active || frame->kind > RINZU_ACTIVITY_NATIVE || frame->owner != closing->owner)
      continue;
    char line[320];
    int length = snprintf(line, sizeof(line), "rinzu wait stack: symbol=%.240s\n", frame->symbol);
    if (length < 0)
      _Exit(86);
    rinzu_activity_diagnostic_write(line, (size_t)length < sizeof(line) ? (size_t)length
                                                                        : sizeof(line) - 1);
  }
  /* No synthesized return, fallback, managed cleanup or premature release. */
  _Exit(86);
}
static inline rinzu_wait_dependency_t *
rinzu_activity_wait_begin_on(rinzu_activity_board_t *board, uint64_t task, uint64_t dependency,
                             const char *symbol, uint64_t file, uint32_t start, uint32_t end,
                             uint64_t owner, uint64_t depth, pthread_t creator) {
  if (!board || !symbol || !rinzu_thread_equal(board->owner, creator))
    _Exit(86);
  size_t length = strlen(symbol);
  if (length > SIZE_MAX - sizeof(rinzu_wait_dependency_t) - 1)
    _Exit(71);
  size_t bytes = sizeof(rinzu_wait_dependency_t) + length + 1;
  rinzu_wait_dependency_t *wait = rinzu_foreign_allocate(bytes);
  wait->board = board;
  wait->task = task;
  wait->dependency = dependency;
  wait->file = file;
  wait->start_byte = start;
  wait->end_byte = end;
  wait->owner = owner;
  wait->depth = depth;
  wait->bytes = bytes;
  memcpy(wait->symbol, symbol, length + 1);
  rinzu_activity_gate_lock(board);
  if (board->stop || rinzu_activity_wait_find_locked(board, task))
    _Exit(86);
  if (board->stats.wait_live == SIZE_MAX || bytes > SIZE_MAX - board->stats.wait_bytes)
    _Exit(71);
  rinzu_wait_dependency_t **retired_index = NULL;
  if (board->stats.wait_live >= board->wait_capacity) {
    size_t capacity = board->wait_capacity;
    while (capacity <= board->stats.wait_live && capacity <= SIZE_MAX / 2)
      capacity *= 2;
    if (capacity > board->stats.wait_live)
      retired_index = rinzu_activity_wait_resize_locked(board, capacity, bytes);
  }
  size_t bucket = rinzu_activity_wait_bucket(task, board->wait_capacity);
  wait->next = board->waits[bucket];
  board->waits[bucket] = wait;
  board->stats.wait_live++;
  board->stats.wait_bytes += bytes;
  uint64_t cursor = dependency;
  for (size_t links = 0;; links++) {
    if (cursor == task)
      rinzu_activity_wait_cycle_fatal_locked(wait);
    rinzu_wait_dependency_t *next = rinzu_activity_wait_find_locked(board, cursor);
    if (!next)
      break;
    if (links >= board->stats.wait_live)
      _Exit(86);
    cursor = next->dependency;
  }
  rinzu_activity_gate_unlock(board);
  if (retired_index)
    free(retired_index);
  return wait;
}
/* Public entry retains a fresh current-thread read and its null guards. */
static inline rinzu_wait_dependency_t *rinzu_activity_wait_begin(rinzu_activity_board_t *board,
                                                                 uint64_t task, uint64_t dependency,
                                                                 const char *symbol, uint64_t file,
                                                                 uint32_t start, uint32_t end,
                                                                 uint64_t owner, uint64_t depth) {
  if (!board || !symbol)
    _Exit(86);
  return rinzu_activity_wait_begin_on(board, task, dependency, symbol, file, start, end, owner,
                                      depth, pthread_self());
}
static inline void rinzu_activity_wait_end(rinzu_wait_dependency_t *wait) {
  if (!wait || !rinzu_thread_equal(wait->board->owner, pthread_self()))
    _Exit(86);
  rinzu_activity_board_t *board = wait->board;
  rinzu_activity_gate_lock(board);
  size_t bucket = rinzu_activity_wait_bucket(wait->task, board->wait_capacity);
  rinzu_wait_dependency_t **cursor = &board->waits[bucket];
  while (*cursor && *cursor != wait)
    cursor = &(*cursor)->next;
  if (!*cursor || !board->stats.wait_live || board->stats.wait_bytes < wait->bytes)
    _Exit(86);
  *cursor = wait->next;
  board->stats.wait_live--;
  board->stats.wait_bytes -= wait->bytes;
  size_t capacity = board->wait_capacity;
  if (!board->stats.wait_live)
    capacity = RINZU_ACTIVITY_WAIT_INLINE_BUCKETS;
  else {
    /* Keep small populated maps; shrink sparse large peaks with hysteresis. */
    size_t floor =
        RINZU_ACTIVITY_WAIT_INLINE_BUCKETS > 128 ? RINZU_ACTIVITY_WAIT_INLINE_BUCKETS : 128;
    while (capacity > floor && board->stats.wait_live < capacity / 8)
      capacity /= 2;
  }
  rinzu_wait_dependency_t **retired_index = NULL;
  if (capacity != board->wait_capacity)
    retired_index = rinzu_activity_wait_resize_locked(board, capacity, 0);
  rinzu_activity_gate_unlock(board);
  if (retired_index)
    free(retired_index);
  free(wait);
}
static inline uint64_t rinzu_activity_threshold(unsigned kind) {
  static const uint64_t limits[] = {UINT64_C(50000000), UINT64_C(50000000), UINT64_C(100000000),
                                    UINT64_C(1000000000), UINT64_C(5000000000)};
  if (kind >= RINZU_ACTIVITY_KINDS)
    _Exit(86);
  return limits[kind];
}
static inline void rinzu_activity_clock_init(rinzu_activity_board_t *board) {
#ifdef __APPLE__
  mach_timebase_info_data_t info;
  int saved_errno = errno;
  kern_return_t status = mach_timebase_info(&info);
  errno = saved_errno;
  /* A failed metadata query must not move a clock failure into preparation.
     Leave the original clock path responsible at each actual observation. */
  if (status != KERN_SUCCESS || !info.numer || !info.denom)
    return;
  board->clock_numer = info.numer;
  board->clock_denom = info.denom;
  for (unsigned kind = 0; kind < RINZU_ACTIVITY_KINDS; ++kind) {
    __uint128_t cutoff = (__uint128_t)(rinzu_activity_threshold(kind) - 1) * info.denom;
    board->clock_not_due[kind] = cutoff > UINT64_MAX ? UINT64_MAX : (uint64_t)cutoff;
  }
#else
  (void)board;
#endif
}
static inline uint64_t rinzu_activity_sample(rinzu_activity_board_t *board) {
#ifdef __APPLE__
  if (board->clock_denom) {
    /* Match clock_gettime_nsec_np's uint64_t multiplication, including wrap.
       Dividing each endpoint separately is deferred, never approximated. */
    uint64_t product = mach_continuous_time() * board->clock_numer;
    if (product < board->clock_denom)
      _Exit(86);
    return product;
  }
#else
  (void)board;
#endif
  return rinzu_activity_now();
}
static inline uint64_t rinzu_activity_sample_ns(const rinzu_activity_board_t *board,
                                                uint64_t sample) {
  return board->clock_denom ? sample / board->clock_denom : sample;
}
static inline int rinzu_activity_sample_before(const rinzu_activity_board_t *board, uint64_t now,
                                               uint64_t started) {
  return now < started &&
         rinzu_activity_sample_ns(board, now) < rinzu_activity_sample_ns(board, started);
}
static inline int rinzu_activity_sample_due(const rinzu_activity_board_t *board, uint64_t now,
                                            uint64_t started, unsigned kind) {
  uint64_t limit = rinzu_activity_threshold(kind);
  if (now < started)
    return 0;
  if (!board->clock_denom)
    return now - started >= limit;
  /* floor(now/d)-floor(started/d) <= ceil((now-started)/d). This cutoff
     proves not-due for every rounding phase; ambiguous endpoints use the
     exact original nanosecond subtraction below. Saturation is also safe. */
  if (now - started <= board->clock_not_due[kind])
    return 0;
  return rinzu_activity_sample_ns(board, now) - rinzu_activity_sample_ns(board, started) >= limit;
}
static inline const char *rinzu_activity_name(unsigned kind) {
  static const char *names[] = {"long-managed", "long-native", "foreign-result-wait", "owner-stall",
                                "shutdown-blocker"};
  if (kind >= RINZU_ACTIVITY_KINDS)
    _Exit(86);
  return names[kind];
}
static inline void rinzu_activity_release_locked(rinzu_activity_t *activity) {
  if (!activity->references)
    _Exit(86);
  if (--activity->references)
    return;
  rinzu_activity_board_t *board = activity->board;
  if (activity->active || !board->stats.live || board->stats.bytes < activity->bytes)
    _Exit(86);
  if (activity->link)
    _Exit(86);
  board->stats.live--;
  board->stats.bytes -= activity->bytes;
#if RINZU_ACTIVITY_CACHE_CAPACITY
  /* This is the final reference, after logical unlink and every emitter pin. */
  if (activity->bytes == RINZU_ACTIVITY_CACHE_SLOT_BYTES &&
      board->stats.cached_records < RINZU_ACTIVITY_CACHE_CAPACITY) {
    activity->next = board->cached_records;
    board->cached_records = activity;
    board->stats.cached_records++;
    board->stats.cached_bytes += RINZU_ACTIVITY_CACHE_SLOT_BYTES;
    return;
  }
#endif
  free(activity);
}
static inline int rinzu_activity_warn_locked(rinzu_activity_t *activity, uint64_t now) {
  if (!activity->active || activity->warned)
    return 0;
  rinzu_activity_board_t *board = activity->board;
  if (rinzu_activity_sample_before(board, now, activity->started))
    _Exit(86);
  uint64_t started = activity->started;
  if (activity->kind == RINZU_ACTIVITY_OWNER_STALL && board->owner_turn > started)
    started = board->owner_turn;
  /* A newer Owner progress sample supersedes this observation. There is no
     elapsed stall interval to report yet; try again on the next watchdog tick.
     Best-effort diagnostics must not terminate a healthy Owner for this case. */
  if (now < started)
    return 0;
  if (!rinzu_activity_sample_due(board, now, started, activity->kind))
    return 0;
  uint64_t elapsed =
      rinzu_activity_sample_ns(board, now) - rinzu_activity_sample_ns(board, started);
  activity->warned = 1;
  int length = snprintf(
      activity->message, sizeof(activity->message),
      "rinzu warning: %s symbol=%.240s source=%llu:%u:%u owner=0x%llx depth=%llu elapsed_ms=%llu "
      "activity=%llu\n",
      rinzu_activity_name(activity->kind), activity->symbol, (unsigned long long)activity->file,
      activity->start_byte, activity->end_byte, (unsigned long long)activity->owner,
      (unsigned long long)activity->depth, (unsigned long long)(elapsed / UINT64_C(1000000)),
      (unsigned long long)activity->identity);
  if (length < 0)
    _Exit(86);
  /* Snapshot the current logical native/managed scopes while their prepared
     symbols are pinned. Foreign wait records name their Origin Owner so that
     its executing scopes are shown. Parked/ended scopes and other Owners are
     not represented as executing frames. No managed frame is inspected. */
  size_t used =
      (size_t)length < sizeof(activity->message) ? (size_t)length : sizeof(activity->message) - 1;
  if (used && used < sizeof(activity->message) - 1) {
    used--;
    int added = snprintf(activity->message + used, sizeof(activity->message) - used, " stack=");
    if (added < 0)
      _Exit(86);
    used += (size_t)added;
    const char *separator = "";
    for (rinzu_activity_t *frame = board->head; frame && used < sizeof(activity->message) - 2;
         frame = frame->next) {
      if (!frame->active || frame->owner != activity->owner || frame->kind > RINZU_ACTIVITY_NATIVE)
        continue;
      added = snprintf(activity->message + used, sizeof(activity->message) - used, "%s%s",
                       separator, frame->symbol);
      if (added < 0)
        _Exit(86);
      size_t available = sizeof(activity->message) - used;
      used += (size_t)added < available ? (size_t)added : available - 1;
      separator = ">";
    }
    if (used >= sizeof(activity->message) - 1)
      used = sizeof(activity->message) - 2;
    activity->message[used++] = '\n';
    activity->message[used] = 0;
    length = (int)used;
  }
  activity->message_length =
      (size_t)length < sizeof(activity->message) ? (size_t)length : sizeof(activity->message) - 1;
  if (activity->references == SIZE_MAX || board->stats.warning_pins == SIZE_MAX ||
      board->stats.warnings[activity->kind] == UINT64_MAX)
    _Exit(71);
  activity->references++;
  board->stats.warning_pins++;
  board->stats.warnings[activity->kind]++;
  return 1;
}
static inline rinzu_activity_t *rinzu_activity_due_locked(rinzu_activity_board_t *board,
                                                          uint64_t now) {
  for (rinzu_activity_t *activity = board->head; activity; activity = activity->next)
    if (rinzu_activity_warn_locked(activity, now))
      return activity;
  return NULL;
}
static inline void rinzu_activity_enqueue_locked(rinzu_activity_t *activity) {
  rinzu_activity_board_t *board = activity->board;
  activity->pending_next = NULL;
  if (board->pending_tail)
    board->pending_tail->pending_next = activity;
  else
    board->pending = activity;
  board->pending_tail = activity;
}
static inline void rinzu_activity_owner_turn(rinzu_activity_board_t *board) {
  if (!board || !rinzu_thread_equal(board->owner, pthread_self()))
    _Exit(86);
  rinzu_activity_gate_lock(board);
  uint64_t now = rinzu_activity_sample(board);
  /* A ready record's deadline starts at max(started, owner_turn). Until one
     full stall interval has passed, none can be overdue. A shutdown request
     still needs the physical-native-frame scan before it can be cleared. */
  if (board->shutdown_requested || rinzu_activity_sample_before(board, now, board->owner_turn) ||
      rinzu_activity_sample_due(board, now, board->owner_turn, RINZU_ACTIVITY_OWNER_STALL)) {
    /* Publish a missed overdue interval before recording progress. Active ready
       records remain linked independently of the emitter's warning queue. */
    int native_active = 0;
    for (rinzu_activity_t *activity = board->head; activity; activity = activity->next) {
      if (activity->kind == RINZU_ACTIVITY_OWNER_STALL && rinzu_activity_warn_locked(activity, now))
        rinzu_activity_enqueue_locked(activity);
      if (activity->kind == RINZU_ACTIVITY_NATIVE && activity->active)
        native_active = 1;
    }
    /* Once every physical native frame returned, a later ordinary task must not
       inherit another task's interruption. Its own control polls can re-arm. */
    if (!native_active)
      board->shutdown_requested = 0;
  }
  board->owner_turn = now;
  /* Progress postpones a stall deadline. Wake only if it also queued a missed
     overdue interval; otherwise the existing active timer remains sufficient. */
  if (board->pending)
    rinzu_activity_gate_signal(board);
  rinzu_activity_gate_unlock(board);
}
static inline void *rinzu_activity_watch(void *raw) {
  rinzu_activity_board_t *board = raw;
  rinzu_activity_gate_lock_slow(board);
  while (!board->stop || board->pending) {
    rinzu_activity_t *activity = board->pending;
    if (activity) {
      board->pending = activity->pending_next;
      if (!board->pending)
        board->pending_tail = NULL;
      activity->pending_next = NULL;
    } else
      activity = rinzu_activity_due_locked(board, rinzu_activity_sample(board));
    if (activity) {
#ifdef RINZU_ACTIVITY_TESTING
      rinzu_foreign_notify_t hook = board->before_emit;
      void *context = board->test_context;
#endif
      rinzu_activity_gate_unlock(board);
#ifdef RINZU_ACTIVITY_TESTING
      if (hook)
        hook(context);
#endif
      /* Best-effort diagnostics never wait for an Owner turn. Skip a currently
         unwritable stderr. No control/admission/heap lock spans this write. */
      rinzu_activity_diagnostic_write(activity->message, activity->message_length);
      rinzu_activity_gate_lock_slow(board);
      if (!board->stats.warning_pins)
        _Exit(86);
      board->stats.warning_pins--;
      rinzu_activity_release_locked(activity);
      continue;
    }
    /* No active record has a deadline. Activation/shutdown/stop still signal
       under this same gate, so entering an indefinite wait cannot lose work. */
    if (!board->stats.active) {
      rinzu_activity_gate_before_wait(board);
      rinzu_foreign_sync(pthread_cond_wait(&board->changed, &board->gate));
      rinzu_activity_gate_claim_locked(board);
      continue;
    }
    rinzu_activity_gate_before_wait(board);
#ifdef __APPLE__
    struct timespec duration = {.tv_sec = 0, .tv_nsec = 10000000L};
    int status = pthread_cond_timedwait_relative_np(&board->changed, &board->gate, &duration);
#else
    struct timespec deadline;
    if (clock_gettime(CLOCK_MONOTONIC, &deadline))
      _Exit(86);
    deadline.tv_nsec += 10000000L;
    if (deadline.tv_nsec >= 1000000000L) {
      deadline.tv_sec++;
      deadline.tv_nsec -= 1000000000L;
    }
    int status = pthread_cond_timedwait(&board->changed, &board->gate, &deadline);
#endif
    if (status && status != ETIMEDOUT)
      rinzu_foreign_sync(status);
    rinzu_activity_gate_claim_locked(board);
  }
  rinzu_activity_gate_unlock(board);
  return NULL;
}
static inline rinzu_activity_board_t *rinzu_activity_new(void) {
  rinzu_activity_board_t *board = rinzu_foreign_allocate(sizeof(*board));
  board->owner = pthread_self();
  board->waits = board->inline_waits;
  board->wait_capacity = RINZU_ACTIVITY_WAIT_INLINE_BUCKETS;
  rinzu_activity_clock_init(board);
  rinzu_activity_gate_init(board);
  pthread_condattr_t attributes;
  rinzu_foreign_sync(pthread_condattr_init(&attributes));
#ifndef __APPLE__
  rinzu_foreign_sync(pthread_condattr_setclock(&attributes, CLOCK_MONOTONIC));
#endif
  rinzu_foreign_sync(pthread_cond_init(&board->changed, &attributes));
  rinzu_foreign_sync(pthread_condattr_destroy(&attributes));
  rinzu_foreign_sync(pthread_create(&board->watchdog, NULL, rinzu_activity_watch, board));
  return board;
}
static inline rinzu_activity_t *rinzu_activity_prepare_on(rinzu_activity_board_t *board,
                                                          unsigned kind, const char *symbol,
                                                          uint64_t file, uint32_t start_byte,
                                                          uint32_t end_byte, uint64_t owner,
                                                          uint64_t depth, pthread_t creator) {
  if (!board || !symbol)
    _Exit(86);
  (void)rinzu_activity_threshold(kind);
  size_t length = strlen(symbol);
  if (length > SIZE_MAX - sizeof(rinzu_activity_t) - 1)
    _Exit(71);
  size_t bytes = sizeof(rinzu_activity_t) + length + 1;
  if (bytes <= RINZU_ACTIVITY_CACHE_SLOT_BYTES)
    bytes = RINZU_ACTIVITY_CACHE_SLOT_BYTES;
  rinzu_activity_t *activity = NULL;
#if RINZU_ACTIVITY_CACHE_CAPACITY
  unsigned from_cache = 0;
  rinzu_activity_gate_lock(board);
  if (bytes == RINZU_ACTIVITY_CACHE_SLOT_BYTES && board->cached_records) {
    activity = board->cached_records;
    board->cached_records = activity->next;
    if (!board->stats.cached_records || board->stats.cached_bytes < bytes)
      _Exit(86);
    board->stats.cached_records--;
    board->stats.cached_bytes -= bytes;
    from_cache = 1;
#if RINZU_ACTIVITY_CACHE_PREFIX_RESET
    /* Every semantic field precedes message. A warning formats a new message
       before exposing its exact fresh length to the emitter. Symbol is copied
       including its new terminator below; no stale suffix is observed. */
    memset(activity, 0, offsetof(rinzu_activity_t, message));
#else
    memset(activity, 0, bytes);
#endif
  }
  if (!activity) {
    /* Optional cache miss never allocates while holding the diagnostic gate. */
    rinzu_activity_gate_unlock(board);
    activity = rinzu_foreign_allocate(bytes);
  }
#else
  activity = rinzu_foreign_allocate(bytes);
#endif
  activity->board = board;
  activity->creator = creator;
  activity->references = 1;
  activity->bytes = bytes;
  activity->kind = kind;
  activity->file = file;
  activity->start_byte = start_byte;
  activity->end_byte = end_byte;
  activity->owner = owner;
  activity->depth = depth;
  memcpy(activity->symbol, symbol, length + 1);
  /* A miss is private until publication. In particular an unbounded full
     symbol copy must not hold the watchdog's gate. Hits copy at most the
     fixed class's symbol space, under their existing pop/publication gate. */
#if RINZU_ACTIVITY_CACHE_CAPACITY
  if (!from_cache)
    rinzu_activity_gate_lock(board);
#else
  rinzu_activity_gate_lock(board);
#endif
  if (board->stop || board->stats.prepared == UINT64_MAX || board->stats.live == SIZE_MAX ||
      bytes > SIZE_MAX - board->stats.bytes)
    _Exit(71);
  activity->identity = ++board->stats.prepared;
  board->stats.live++;
  board->stats.bytes += bytes;
  activity->next = board->head;
  activity->link = &board->head;
  if (activity->next)
    activity->next->link = &activity->next;
  board->head = activity;
  rinzu_activity_gate_unlock(board);
  return activity;
}
static inline rinzu_activity_t *rinzu_activity_prepare(rinzu_activity_board_t *board, unsigned kind,
                                                       const char *symbol, uint64_t file,
                                                       uint32_t start_byte, uint32_t end_byte,
                                                       uint64_t owner, uint64_t depth) {
  return rinzu_activity_prepare_on(board, kind, symbol, file, start_byte, end_byte, owner, depth,
                                   pthread_self());
}
/* Both intervals exist before physical native entry. A request can arm only
   the paired shutdown descriptor while the actual native interval is active. */
static inline rinzu_activity_t *rinzu_activity_prepare_native_on(rinzu_activity_board_t *board,
                                                                 const char *symbol, uint64_t file,
                                                                 uint32_t start, uint32_t end,
                                                                 uint64_t owner, uint64_t depth,
                                                                 pthread_t creator) {
  rinzu_activity_t *native = rinzu_activity_prepare_on(board, RINZU_ACTIVITY_NATIVE, symbol, file,
                                                       start, end, owner, depth, creator);
  rinzu_activity_t *blocker = rinzu_activity_prepare_on(board, RINZU_ACTIVITY_SHUTDOWN, symbol,
                                                        file, start, end, owner, depth, creator);
  rinzu_activity_gate_lock(board);
  native->blocker = blocker;
  rinzu_activity_gate_unlock(board);
  return native;
}
static inline rinzu_activity_t *rinzu_activity_prepare_native(rinzu_activity_board_t *board,
                                                              const char *symbol, uint64_t file,
                                                              uint32_t start, uint32_t end,
                                                              uint64_t owner, uint64_t depth) {
  return rinzu_activity_prepare_native_on(board, symbol, file, start, end, owner, depth,
                                          pthread_self());
}
static inline void rinzu_activity_activate_locked(rinzu_activity_t *activity, uint64_t now) {
  rinzu_activity_board_t *board = activity->board;
  if (board->stop || activity->active || activity->warned)
    _Exit(86);
  if (board->stats.active == SIZE_MAX)
    _Exit(71);
  activity->started = now;
  activity->active = 1;
  board->stats.active++;
}
static inline void rinzu_activity_activate(rinzu_activity_t *activity) {
  rinzu_activity_board_t *board = activity->board;
  rinzu_activity_gate_lock(board);
  uint64_t now = rinzu_activity_sample(board);
  int was_inactive = board->stats.active == 0;
  rinzu_activity_activate_locked(activity, now);
  if (activity->blocker && board->shutdown_requested)
    rinzu_activity_activate_locked(activity->blocker, now);
  /* A nonempty active set already keeps the watchdog on its 10ms timer.
     Only 0 -> 1 must wake its possible indefinite wait. The same gate
     makes the active check and condition wait atomic against publication. */
  if (was_inactive)
    rinzu_activity_gate_signal(board);
  rinzu_activity_gate_unlock(board);
}
/* A native-only request may come from another thread holding a board pin. It
   never stops a symbol or releases its Owner's frame/root/lease. */
static inline void rinzu_activity_request_shutdown(rinzu_activity_board_t *board) {
  if (!board)
    _Exit(86);
  rinzu_activity_gate_lock(board);
  if (board->stop)
    _Exit(86);
  board->shutdown_requested = 1;
  uint64_t now = rinzu_activity_sample(board);
  for (rinzu_activity_t *native = board->head; native; native = native->next)
    if (native->active && native->blocker && !native->blocker->active)
      rinzu_activity_activate_locked(native->blocker, now);
  rinzu_activity_gate_signal(board);
  rinzu_activity_gate_unlock(board);
}
static inline void rinzu_activity_start(rinzu_activity_t *activity) {
  if (!activity || !rinzu_thread_equal(activity->creator, pthread_self()))
    _Exit(86);
  rinzu_activity_activate(activity);
}
/* Called only by a prepared ready-work publication. Its physical producer can
   start an Owner timer; settlement remains confined to the Origin Owner. */
static inline void rinzu_activity_publish_ready(rinzu_activity_t *activity) {
  if (!activity || activity->kind != RINZU_ACTIVITY_OWNER_STALL)
    _Exit(86);
  rinzu_activity_activate(activity);
}
static inline void rinzu_activity_end_locked(rinzu_activity_t *activity, uint64_t now) {
  rinzu_activity_board_t *board = activity->board;
  rinzu_activity_t **entry = activity->link;
  if (!entry || *entry != activity || board->stats.ended == UINT64_MAX)
    _Exit(86);
  int overdue = rinzu_activity_warn_locked(activity, now);
  *entry = activity->next;
  if (activity->next)
    activity->next->link = entry;
  activity->link = NULL;
  activity->next = NULL;
  /* If the watchdog was descheduled, physical end still publishes its overdue
     warning to the native emitter, never writing from the Owner itself. */
  if (overdue) {
    rinzu_activity_enqueue_locked(activity);
  }
  if (activity->active) {
    if (!board->stats.active)
      _Exit(86);
    board->stats.active--;
    activity->active = 0;
  }
  board->stats.ended++;
  rinzu_activity_release_locked(activity);
}
static inline void rinzu_activity_end(rinzu_activity_t *activity) {
  if (!activity || !rinzu_thread_equal(activity->creator, pthread_self()))
    _Exit(86);
  rinzu_activity_board_t *board = activity->board;
  rinzu_activity_gate_lock(board);
  uint64_t now = rinzu_activity_sample(board);
  rinzu_activity_t *blocker = activity->blocker;
  activity->blocker = NULL;
  if (blocker)
    rinzu_activity_end_locked(blocker, now);
  rinzu_activity_end_locked(activity, now);
  /* Removing a timer advances no remaining deadline. Queued overdue warnings
     still wake immediately, including when the last active record ended. */
  if (board->pending)
    rinzu_activity_gate_signal(board);
  rinzu_activity_gate_unlock(board);
}
static inline rinzu_activity_stats_t rinzu_activity_stats(rinzu_activity_board_t *board) {
  if (!board)
    _Exit(86);
  rinzu_activity_gate_lock(board);
  rinzu_activity_stats_t stats = board->stats;
  rinzu_activity_gate_unlock(board);
  return stats;
}
static inline void rinzu_activity_dispose(rinzu_activity_board_t *board) {
  if (!board)
    _Exit(86);
  rinzu_activity_gate_lock(board);
  if (board->head || board->stats.active || board->stats.prepared != board->stats.ended ||
      board->stats.ready_live || board->stats.ready_bytes || board->stats.wait_live ||
      board->stats.wait_bytes)
    _Exit(86);
  if (board->waits != board->inline_waits ||
      board->wait_capacity != RINZU_ACTIVITY_WAIT_INLINE_BUCKETS)
    _Exit(86);
  for (size_t slot = 0; slot < board->wait_capacity; ++slot)
    if (board->waits[slot])
      _Exit(86);
  board->stop = 1;
  rinzu_activity_gate_signal(board);
  rinzu_activity_gate_unlock(board);
  rinzu_foreign_sync(pthread_join(board->watchdog, NULL));
  if (board->stats.live || board->stats.bytes || board->stats.warning_pins)
    _Exit(86);
  rinzu_activity_cache_drain(board);
  rinzu_foreign_sync(pthread_cond_destroy(&board->changed));
  rinzu_activity_gate_dispose(board);
  free(board);
}
#ifdef RINZU_ACTIVITY_TESTING
static inline void rinzu_activity_test_hook(rinzu_activity_board_t *board,
                                            rinzu_foreign_notify_t hook, void *context) {
  rinzu_activity_gate_lock(board);
  if (board->stats.prepared)
    _Exit(86);
  board->before_emit = hook;
  board->test_context = context;
  rinzu_activity_gate_unlock(board);
}
#endif
#endif

/* Bounded macOS TCP transport. Source wrapper/standard registry: M5.12-8. */
#include "rinzu_socket.h"
#include "rinzu/native_completion.h"
#include "rinzu/native_read_view.h"
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <sys/event.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

enum {
  HANDLES = 256,
  DATA_OPS = 512,
  OPS = DATA_OPS + HANDLES,
  CHUNK = 65536,
  OWNER_HANDLES = 64,
  OWNER_OPS = 128,
  BUFFER_LIMIT = 32 * 1024 * 1024,
  OWNER_BUFFER_LIMIT = 8 * 1024 * 1024,
  OP_WORDS = (OPS + 63) / 64,
  HANDLE_WORDS = (HANDLES + 63) / 64
};
enum Kind { LISTEN, CONNECT, ACCEPT, READ, WRITE, HALF, CLOSE };
/* Keep the generation shared by handles and operations. A slot is only a
   candidate: its complete ID must match before any other field is read. */
#define TOKEN_SLOT_BITS 10
#define TOKEN_SLOT_MASK ((((uintptr_t)1) << TOKEN_SLOT_BITS) - 1)
_Static_assert(HANDLES <= TOKEN_SLOT_MASK && OPS < TOKEN_SLOT_MASK,
               "socket tables must fit the token slot bits");
struct ReadyRead {
  uintptr_t id;
  struct Handle *stream;
  rinzu_native_byte_array_t bytes;
};
/* Entries are protected by lock and remain stable until all of an Owner's
   handles and operations have been released. Reactor cleanup must not retain
   pointers into an Owner thread's TLS. Every production operation holds a
   handle, so HANDLES entries also bound the number of concurrent Owners. */
struct Owner {
  pthread_t thread;
  size_t handles, operations, data_operations, bytes;
  struct ReadyRead read;
};
struct Handle {
  uintptr_t id;
  pthread_t owner;
  struct Owner *account;
  int fd;
  bool listener, closing, eof, send_closed;
  unsigned refs, reads, writes;
};
struct Operation {
  rinzu_native_completion_v2_t completion;
  int64_t completion_result;
  uintptr_t id;
  pthread_t owner;
  struct Owner *account;
  enum Kind kind;
  struct Handle *handle, *value;
  struct Operation *previous_read;
  struct sockaddr_in address;
  bool ready, attempt, all, ack, registered;
  int backlog;
  /* Pending READs use the recv limit; ready READs use the initialized prefix
     watermark. WRITE buffers start at the prefix sent before reservation. */
  union {
    size_t maximum;
    size_t initialized;
    size_t sent_prefix;
  };
  int32_t status;
  union {
    rinzu_native_byte_array_t bytes;
    /* C11 common initial sequence: these fields and bytes have identical
       types/order. Reuse existing metadata, without an allocated descriptor. */
    struct {
      uint8_t *buffer;
      size_t length;
    };
  };
  size_t capacity;
  int64_t deadline;
};
_Static_assert(offsetof(struct Operation, completion_result) ==
                   offsetof(rinzu_native_completion_v3_t, result),
               "socket transfer receipt");
static struct Handle handles[HANDLES];
static struct Operation operations[OPS];
static struct Owner owners[HANDLES];
static size_t handle_count, operation_count, data_count;
static uint64_t pending_operations[OP_WORDS], closing_handles[HANDLE_WORDS];
static size_t pending_count;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_t worker;
static int queue_fd = -1;
static bool running, stopping, wake_pending, worker_data_io;
/* Held from bounded poll selection until the reactor regains lock. */
static bool polling_with_timeout;
/* Only the reactor thread installs a batch. A synchronous Owner publication
   uses the immediate notification path even when a reactor syscall unlocks. */
struct CompletionWakeBatch {
  rinzu_native_completion_wake_t *target;
};
static _Thread_local struct CompletionWakeBatch *completion_batch;
/* Completed reads are immutable until the Owner ends the scope. A consumer
   on a different thread only sees that thread's own active scopes. */
static _Thread_local struct Operation *active_read;
static _Thread_local struct ReadyRead *active_ready_read;
static _Thread_local rinzu_native_byte_array_t read_slice;
static uintptr_t next_id = 1;
static size_t buffers, buffer_bytes, peak_buffer_bytes, partial_writes, event_count, stale_events;
static size_t allocations, copied_bytes, read_calls, write_calls, received_bytes, sent_bytes;
static size_t kevent_calls, user_wakes, poll_timeouts, round_robin;

static _Noreturn void fatal(void) {
  _Exit(86);
}
static int32_t failure(void) {
  if (errno == ENOMEM)
    _Exit(70);
  return errno ? errno : EIO;
}
static int64_t monotonic(void) {
  struct timespec time;
  if (clock_gettime(CLOCK_MONOTONIC, &time))
    fatal();
  return time.tv_sec * INT64_C(1000) + time.tv_nsec / 1000000;
}
static void bitmap_add(uint64_t *bitmap, size_t index) {
  bitmap[index / 64] |= UINT64_C(1) << (index % 64);
}
static void bitmap_remove(uint64_t *bitmap, size_t index) {
  bitmap[index / 64] &= ~(UINT64_C(1) << (index % 64));
}
/* First selected slot in [start,end). Re-reading at each step also skips slots
   removed by a preceding advance/close without changing their original order. */
static size_t bitmap_next(const uint64_t *bitmap, size_t start, size_t end) {
  if (start >= end)
    return end;
  size_t word = start / 64, last = (end - 1) / 64;
  uint64_t mask = UINT64_MAX << (start % 64);
  for (;;) {
    uint64_t selected = bitmap[word] & mask;
    if (word == last && end % 64)
      selected &= UINT64_MAX >> (64 - end % 64);
    if (selected)
      return word * 64 + (size_t)__builtin_ctzll(selected);
    if (word == last)
      return end;
    ++word;
    mask = UINT64_MAX;
  }
}
static void request_close(struct Handle *h) {
  h->closing = true;
  if (h->fd >= 0)
    bitmap_add(closing_handles, (size_t)(h - handles));
}
static uintptr_t identity(size_t slot) {
  if (slot >= TOKEN_SLOT_MASK || next_id > ((uintptr_t)INT64_MAX >> TOKEN_SLOT_BITS))
    fatal();
  return (next_id++ << TOKEN_SLOT_BITS) | (slot + 1);
}
static struct Handle *handle(void *token) {
  uintptr_t id = (uintptr_t)token;
  size_t slot = id & TOKEN_SLOT_MASK;
  if (!id || !slot || slot > HANDLES)
    return NULL;
  struct Handle *found = &handles[slot - 1];
  return found->id == id ? found : NULL;
}
static struct Operation *lookup_operation(uintptr_t id) {
  size_t slot = id & TOKEN_SLOT_MASK;
  if (!id || !slot || slot > OPS)
    return NULL;
  struct Operation *found = &operations[slot - 1];
  return found->id == id ? found : NULL;
}
static struct Operation *operation(void *token) {
  struct Operation *found = lookup_operation((uintptr_t)token);
  if (!found)
    fatal();
  return found;
}
static struct Owner *owner_account(void) {
  struct Owner *available = NULL;
  for (size_t i = 0; i < HANDLES; ++i) {
    struct Owner *account = &owners[i];
    if (account->handles || account->operations) {
      if (pthread_equal(account->thread, pthread_self()))
        return account;
    } else if (!available)
      available = account;
  }
  if (available) {
    memset(available, 0, sizeof(*available));
    available->thread = pthread_self();
  }
  return available;
}
static struct Handle *reserve_handle(void) {
  struct Owner *account = owner_account();
  if (!account || account->handles >= OWNER_HANDLES)
    return NULL;
  for (size_t i = 0; i < HANDLES; ++i)
    if (!handles[i].id) {
      struct Handle *h = &handles[i];
      memset(h, 0, sizeof(*h));
      h->id = identity(i);
      h->owner = pthread_self();
      h->account = account;
      h->fd = -1;
      ++account->handles;
      ++handle_count;
      return h;
    }
  return NULL;
}
static void discard_handle(struct Handle *h) {
  bitmap_remove(closing_handles, (size_t)(h - handles));
  --h->account->handles;
  --handle_count;
  h->account = NULL;
  h->id = 0;
}
static void release_handle(struct Handle *h) {
  if (h && h->id && h->closing && h->fd < 0 && !h->refs)
    discard_handle(h);
}
static int check(struct Handle *h, bool listener) {
  if (!h)
    return -7502;
  if (!pthread_equal(h->owner, pthread_self()))
    return -7501;
  if (h->closing)
    return -7502;
  if (h->listener != listener)
    return -7504;
  return 0;
}
static int filter(enum Kind kind) {
  return kind == READ || kind == ACCEPT ? EVFILT_READ : EVFILT_WRITE;
}
static void unregister(struct Operation *op) {
  if (!op->registered)
    return;
  if (!op->handle)
    fatal();
  struct kevent event;
  EV_SET(&event, op->handle->fd, filter(op->kind), EV_DELETE, 0, 0, NULL);
  ++kevent_calls;
  if (kevent(queue_fd, &event, 1, NULL, 0, NULL) < 0 && errno != ENOENT && errno != EBADF)
    fatal();
  op->registered = false;
}
static void flush_completion_wake(struct CompletionWakeBatch *batch) {
  rinzu_native_completion_wake_t *target = batch->target;
  batch->target = NULL;
  if (target) {
    target->notify(target);
    target->release(target);
  }
}
static void publish(struct Operation *op, int32_t status) {
  unregister(op);
  op->status = status;
  if ((op->kind == READ || op->kind == WRITE) && op->length > CHUNK)
    fatal();
  if (op->kind == READ || op->kind == WRITE)
    op->completion_result = (int64_t)status * INT64_C(131072) + (int64_t)op->length;
  if (op->kind == READ)
    op->initialized = op->length;
  op->ready = true;
  bitmap_remove(pending_operations, (op->id & TOKEN_SLOT_MASK) - 1);
  --pending_count;
  rinzu_native_completion_wake_t *target =
      rinzu_native_completion_v2_publish_target(&op->completion);
  if (!target)
    return;
  struct CompletionWakeBatch *batch = completion_batch;
  if (!batch) {
    target->notify(target);
    target->release(target);
  } else if (target == batch->target) {
    target->release(target);
  } else {
    /* Each target wakes a whole notification domain. Retain one reference
       across the pass, and notify the previous domain before switching. */
    flush_completion_wake(batch);
    batch->target = target;
  }
}
static void arm(struct Operation *op) {
  struct kevent event;
  EV_SET(&event, op->handle->fd, filter(op->kind), EV_ADD | EV_ENABLE | EV_ONESHOT, 0, 0,
         (void *)op->id);
  ++kevent_calls;
  if (kevent(queue_fd, &event, 1, NULL, 0, NULL) < 0)
    publish(op, failure());
  else
    op->registered = true;
}
static void close_handle(struct Handle *h) {
  h->closing = true;
  bitmap_remove(closing_handles, (size_t)(h - handles));
  for (size_t i = bitmap_next(pending_operations, 0, OPS); i < OPS;
       i = bitmap_next(pending_operations, i + 1, OPS)) {
    struct Operation *op = &operations[i];
    if (op->handle == h && op->kind != CLOSE)
      publish(op, ECANCELED);
  }
  if (h->fd >= 0) {
    int fd = h->fd;
    h->fd = -1;
    (void)close(fd);
  }
  release_handle(h);
}
static int configure(int fd) {
  int one = 1;
  if (fcntl(fd, F_SETFL, O_NONBLOCK) < 0 || fcntl(fd, F_SETFD, FD_CLOEXEC) < 0 ||
      setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof(one)) < 0)
    return failure();
#ifdef RINZU_SOCKET_TEST_BUFFERS
  int buffer = 4096;
  if (setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &buffer, sizeof(buffer)) < 0 ||
      setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &buffer, sizeof(buffer)) < 0)
    return failure();
#endif
  return 0;
}
struct Budget {
  unsigned calls;
  size_t bytes;
};
static void wake(void);
#ifndef RINZU_SOCKET_DATA_IO_STAGE
#define RINZU_SOCKET_DATA_IO_STAGE(operation, stage) ((void)0)
#endif
/* An Owner may advance a newly admitted READ/WRITE only while the worker has no
   unlocked syscall lease, and holds lock through that entire advance. The
   worker marks its lease before unlock, retaining its pending slot, buffer and
   Handle reference. This exclusion also prevents an Owner error/timeout from
   closing the worker's descriptor in the opposite direction. */
static ssize_t data_io(struct Operation *op, bool writing, bool owner) {
  struct Handle *h = op->handle;
  uintptr_t id = op->id, handle_id = h->id;
  int fd = h->fd;
  uint8_t *allocation = op->buffer;
  uint8_t *buffer = allocation + (writing ? op->length - op->sent_prefix : 0);
  size_t count = writing ? op->capacity - op->length : op->maximum;
  if (owner) {
    if (worker_data_io)
      fatal();
    return writing ? send(fd, buffer, count, 0) : recv(fd, buffer, count, 0);
  }
  if (worker_data_io)
    fatal();
  worker_data_io = true;
  pthread_mutex_unlock(&lock);
  RINZU_SOCKET_DATA_IO_STAGE(op, 0);
  ssize_t n = writing ? send(fd, buffer, count, 0) : recv(fd, buffer, count, 0);
  int error = n < 0 ? errno : 0;
  RINZU_SOCKET_DATA_IO_STAGE(op, 1);
  pthread_mutex_lock(&lock);
  worker_data_io = false;
  if (op->id != id || op->ready || op->handle != h || h->id != handle_id || h->fd != fd ||
      op->buffer != allocation)
    fatal();
  /* The claimed syscall precedes a concurrent cancellation. Preserve its
     result, while applying the close that cancel would request after ready.
     Wake also covers a cancellation flag published before its SDK callback. */
  if (atomic_load_explicit(&op->completion.base.cancelled, memory_order_acquire)) {
    request_close(h);
    wake();
  }
  if (n < 0)
    errno = error;
  return n;
}
static void advance(struct Operation *op, struct Budget *budget, bool owner) {
  struct Handle *h = op->handle;
  if (op->deadline && monotonic() >= op->deadline && op->kind != CLOSE) {
    struct Handle *closing = op->kind == ACCEPT ? op->value : h;
    publish(op, -7509);
    close_handle(closing);
    return;
  }
  if (atomic_load_explicit(&op->completion.base.cancelled, memory_order_acquire) &&
      op->kind != CLOSE) {
    if (op->kind == ACCEPT) {
      close_handle(op->value);
      publish(op, ECANCELED);
    } else
      close_handle(h);
    return;
  }
  if (h->closing && op->kind != CLOSE) {
    publish(op, ECANCELED);
    return;
  }
  if (!op->attempt)
    return;
  unsigned needed = op->kind == LISTEN                 ? 6
                    : op->kind == CONNECT && h->fd < 0 ? 5
                    : op->kind == ACCEPT               ? 4
                                                       : 1;
  if (op->kind != CLOSE && (budget->calls > 64 - needed || budget->bytes >= 1024 * 1024))
    return;
  op->attempt = false;
  if (op->kind == CLOSE) {
    close_handle(h);
    publish(op, 0);
    return;
  }
  if (op->kind == LISTEN || (op->kind == CONNECT && h->fd < 0)) {
    budget->calls += op->kind == LISTEN ? 6 : 5;
    h->fd = socket(AF_INET, SOCK_STREAM, 0);
    if (h->fd < 0) {
      publish(op, failure());
      request_close(h);
      return;
    }
    int status = configure(h->fd);
    if (status) {
      publish(op, status);
      close_handle(h);
      return;
    }
    if (op->kind == LISTEN) {
      if (bind(h->fd, (struct sockaddr *)&op->address, sizeof(op->address)) < 0 ||
          listen(h->fd, op->backlog) < 0) {
        int error = failure();
        publish(op, error);
        close_handle(h);
        return;
      }
      publish(op, 0);
      return;
    }
    if (connect(h->fd, (struct sockaddr *)&op->address, sizeof(op->address)) == 0) {
      publish(op, 0);
      return;
    }
    if (errno == EINPROGRESS) {
      arm(op);
      return;
    }
    int error = failure();
    publish(op, error);
    close_handle(h);
    return;
  }
  if (op->kind == CONNECT) {
    ++budget->calls;
    int error = 0;
    socklen_t size = sizeof(error);
    if (getsockopt(h->fd, SOL_SOCKET, SO_ERROR, &error, &size) < 0)
      error = failure();
    publish(op, error);
    if (error)
      close_handle(h);
    return;
  }
  if (op->kind == ACCEPT) {
    budget->calls += 4;
    int fd = accept(h->fd, NULL, NULL);
    if (fd >= 0) {
      int error = configure(fd);
      if (error) {
        (void)close(fd);
        request_close(op->value);
        publish(op, error);
        return;
      }
      op->value->fd = fd;
      publish(op, 0);
      return;
    }
  } else if (op->kind == READ) {
    if (h->eof) {
      publish(op, 0);
      return;
    }
    ++budget->calls;
    ++read_calls;
    ssize_t n = data_io(op, false, owner);
    if (n >= 0) {
      received_bytes += (size_t)n;
      budget->bytes += (size_t)n;
      op->length = (size_t)n;
      h->eof = n == 0;
      publish(op, 0);
      return;
    }
  } else if (op->kind == WRITE) {
    if (!op->capacity) {
      publish(op, 0);
      return;
    }
    for (unsigned count = 0; count < 4 && budget->calls < 64 && budget->bytes < 1024 * 1024;
         ++count) {
      ++budget->calls;
      ++write_calls;
      ssize_t n = data_io(op, true, owner);
      if (n > 0) {
        if ((size_t)n < op->capacity - op->length)
          ++partial_writes;
        op->length += (size_t)n;
        sent_bytes += (size_t)n;
        budget->bytes += (size_t)n;
        if (!op->all || op->length == op->capacity) {
          publish(op, 0);
          return;
        }
        continue;
      }
      if (n == 0) {
        publish(op, -7508);
        close_handle(h);
        return;
      }
      if (errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR) {
        int error = failure();
        publish(op, error);
        close_handle(h);
        return;
      }
      arm(op);
      return;
    }
    arm(op);
    return;
  } else if (op->kind == HALF) {
    ++budget->calls;
    if (shutdown(h->fd, SHUT_WR) < 0) {
      int error = failure();
      publish(op, error);
      close_handle(h);
    } else {
      h->send_closed = true;
      publish(op, 0);
    }
    return;
  } else {
    fatal();
  }
  if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
    arm(op);
    return;
  }
  int error = failure();
  bool reading = op->kind == READ;
  struct Handle *value = op->value;
  publish(op, error);
  if (reading)
    close_handle(h);
  else if (value)
    request_close(value);
}
/* Validate immutable operation identity before touching a reused slot/descriptor. */
static void observe_event(const struct kevent *event) {
  if (event->filter == EVFILT_USER) {
    /* The SDK lock orders receipt against every producer. Work enqueued
       before this receipt is visible to the next advance pass; a producer
       after it must publish a fresh notification. Other user IDs are not ours. */
    if (event->ident == 1)
      wake_pending = false;
    return;
  }
  ++event_count;
  uintptr_t id = (uintptr_t)event->udata;
  struct Operation *found = lookup_operation(id);
  if (!found || found->ready || found->handle->fd != (int)event->ident ||
      filter(found->kind) != event->filter) {
    ++stale_events;
    return;
  }
  found->registered = false;
  found->attempt = true;
  if (event->flags & EV_ERROR)
    publish(found, (int32_t)event->data);
}
static void advance_range(size_t start, size_t end, struct Budget *budget) {
  for (size_t index = bitmap_next(pending_operations, start, end); index < end;
       index = bitmap_next(pending_operations, index + 1, end))
    advance(&operations[index], budget, false);
}
static void *drive(void *unused) {
  (void)unused;
  struct CompletionWakeBatch completions = {0};
  completion_batch = &completions;
  struct kevent events[64];
  int count = -1;
  pthread_mutex_lock(&lock);
  for (;;) {
    polling_with_timeout = false;
    if (count >= 0) {
      ++kevent_calls;
      if (!count)
        ++poll_timeouts;
      for (int i = 0; i < count; ++i)
        observe_event(&events[i]);
    }
    for (size_t i = bitmap_next(closing_handles, 0, HANDLES); i < HANDLES;
         i = bitmap_next(closing_handles, i + 1, HANDLES)) {
      bitmap_remove(closing_handles, i);
      if (handles[i].id && handles[i].closing && handles[i].fd >= 0)
        close_handle(&handles[i]);
    }
    struct Budget budget = {0};
    advance_range(round_robin, OPS, &budget);
    advance_range(0, round_robin, &budget);
    round_robin = (round_robin + 1) % OPS;
    bool pending = pending_count != 0, runnable = false;
    if (pending)
      for (size_t i = bitmap_next(pending_operations, 0, OPS); i < OPS && !runnable;
           i = bitmap_next(pending_operations, i + 1, OPS))
        runnable = operations[i].attempt;
    bool stop = stopping;
    polling_with_timeout = pending && !stop;
    pthread_mutex_unlock(&lock);
    flush_completion_wake(&completions);
    if (stop)
      break;
    struct timespec timeout = {0, runnable ? 0 : 10000000};
    count = kevent(queue_fd, NULL, 0, events, 64, pending ? &timeout : NULL);
    if (count < 0 && errno != EINTR)
      fatal();
    pthread_mutex_lock(&lock);
  }
  completion_batch = NULL;
  return NULL;
}
static void wake(void) {
  /* Called with the SDK lock held. Keep the latch until the matching event is
     actually received, including the gap between kevent return and this lock.
     A nonempty operation table alone cannot certify an outstanding wake. */
  if (wake_pending)
    return;
  wake_pending = true;
  ++user_wakes;
  struct kevent event;
  EV_SET(&event, 1, EVFILT_USER, 0, NOTE_TRIGGER, 0, NULL);
  ++kevent_calls;
  if (kevent(queue_fd, &event, 1, NULL, 0, NULL) < 0)
    fatal();
}
static void start(void) {
  if (running)
    return;
  queue_fd = kqueue();
  if (queue_fd < 0)
    fatal();
  if (fcntl(queue_fd, F_SETFD, FD_CLOEXEC) < 0)
    fatal();
  struct kevent event;
  EV_SET(&event, 1, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, NULL);
  ++kevent_calls;
  if (kevent(queue_fd, &event, 1, NULL, 0, NULL) < 0)
    fatal();
  running = true;
  stopping = false;
  wake_pending = false;
  polling_with_timeout = false;
  if (pthread_create(&worker, NULL, drive, NULL))
    fatal();
}
static bool data_credit(const struct Owner *account) {
  return !stopping && account->data_operations < OWNER_OPS && data_count < DATA_OPS;
}
static struct Operation *reserve(enum Kind kind, struct Handle *h) {
  if (stopping)
    return NULL;
  struct Owner *account = h ? h->account : owner_account();
  if (!account || (kind != CLOSE && !data_credit(account)))
    return NULL;
  size_t first = 0, end = DATA_OPS;
  if (kind == CLOSE) {
    first = DATA_OPS + (size_t)(h - handles);
    end = first + 1;
  }
  for (size_t i = first; i < end; ++i)
    if (!operations[i].id) {
      start();
      struct Operation *op = &operations[i];
      memset(op, 0, sizeof(*op));
      rinzu_native_completion_v2_init(&op->completion);
      if (kind == READ || kind == WRITE) {
        op->completion.base.version = 3;
        op->completion.base.size = sizeof(rinzu_native_completion_v3_t);
      }
      op->id = identity(i);
      op->kind = kind;
      op->owner = pthread_self();
      op->account = account;
      op->handle = h;
      op->attempt = true;
      ++account->operations;
      account->data_operations += kind != CLOSE;
      data_count += kind != CLOSE;
      ++operation_count;
      bitmap_add(pending_operations, i);
      ++pending_count;
      if (h) {
        ++h->refs;
        if (kind == READ || kind == ACCEPT)
          ++h->reads;
        else if (kind == WRITE || kind == HALF)
          ++h->writes;
      }
      return op;
    }
  return NULL;
}
static bool occupied(struct Handle *h, enum Kind kind) {
  return kind == READ || kind == ACCEPT ? h->reads != 0 : h->writes != 0;
}
static int parse_address(const char *host, int32_t port, bool connecting,
                         struct sockaddr_in *address) {
  if (port < (connecting ? 1 : 0) || port > 65535 || !host || !*host)
    return -7504;
  memset(address, 0, sizeof(*address));
  address->sin_len = sizeof(*address);
  address->sin_family = AF_INET;
  address->sin_port = htons((uint16_t)port);
  return inet_pton(AF_INET, host, &address->sin_addr) == 1 ? 0 : -7506;
}
int32_t rinzu_socket_address_status(const char *host, int32_t port) {
  struct sockaddr_in address;
  return parse_address(host, port, false, &address);
}
static int64_t begin_address(enum Kind kind, const char *host, int32_t port, int32_t backlog,
                             int64_t timeout) {
  struct sockaddr_in address;
  int status = parse_address(host, port, kind == CONNECT, &address);
  if (!status && (timeout < 0 || timeout > 30000 || backlog < 1 || backlog > 128))
    status = -7504;
  if (status)
    return status;
  pthread_mutex_lock(&lock);
  if (stopping) {
    pthread_mutex_unlock(&lock);
    return -7505;
  }
  struct Handle *h = reserve_handle();
  if (!h) {
    pthread_mutex_unlock(&lock);
    return -7505;
  }
  struct Operation *op = reserve(kind, h);
  if (!op) {
    discard_handle(h);
    pthread_mutex_unlock(&lock);
    return -7505;
  }
  h->listener = kind == LISTEN;
  op->value = h;
  op->address = address;
  op->backlog = backlog;
  if (timeout)
    op->deadline = monotonic() + timeout;
  int64_t id = (int64_t)op->id;
  wake();
  pthread_mutex_unlock(&lock);
  return id;
}
int64_t rinzu_socket_listen(const char *host, int32_t port, int32_t backlog) {
  return begin_address(LISTEN, host, port, backlog, 0);
}
int64_t rinzu_socket_connect(const char *host, int32_t port, int64_t timeout) {
  return begin_address(CONNECT, host, port, 16, timeout);
}
#ifndef RINZU_SOCKET_OWNER_WRITE_ALLOWED
#define RINZU_SOCKET_OWNER_WRITE_ALLOWED() true
#endif
#ifndef RINZU_SOCKET_OWNER_READ_ALLOWED
#define RINZU_SOCKET_OWNER_READ_ALLOWED() true
#endif
struct InlineAttempt {
  size_t length;
  int64_t deadline;
  int32_t error;
};
/* The SDK lock covers both the direct send and any pending admission. Only
   the caller owns data, and the worker cannot be in an unlocked I/O syscall. */
static bool try_write(struct Handle *h, const uint8_t *data, size_t size, bool all, int64_t timeout,
                      struct InlineAttempt *attempt) {
  if (timeout) {
    attempt->deadline = monotonic() + timeout;
    if (monotonic() >= attempt->deadline) {
      attempt->error = RINZU_SOCKET_DEADLINE;
      return false;
    }
  }
  if (!size)
    return true;
  ++write_calls;
  ssize_t n = send(h->fd, data, size, 0);
  if (n > 0) {
    attempt->length = (size_t)n;
    sent_bytes += (size_t)n;
    if ((size_t)n < size)
      ++partial_writes;
    return !all || (size_t)n == size;
  }
  if (!n)
    attempt->error = RINZU_SOCKET_NO_PROGRESS;
  else if (errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR)
    attempt->error = failure();
  return false;
}
static bool try_read(struct Handle *h, uint8_t *buffer, size_t size, int64_t timeout,
                     struct InlineAttempt *attempt) {
  if (timeout) {
    attempt->deadline = monotonic() + timeout;
    if (monotonic() >= attempt->deadline) {
      attempt->error = RINZU_SOCKET_DEADLINE;
      return false;
    }
  }
  if (h->eof)
    return true;
  ++read_calls;
  ssize_t n = recv(h->fd, buffer, size, 0);
  if (n >= 0) {
    attempt->length = (size_t)n;
    received_bytes += (size_t)n;
    h->eof = n == 0;
    return true;
  }
  if (errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR)
    attempt->error = failure();
  return false;
}
static void account_buffer(struct Owner *owner, size_t capacity) {
  ++buffers;
  ++allocations;
  buffer_bytes += capacity;
  owner->bytes += capacity;
  if (buffer_bytes > peak_buffer_bytes)
    peak_buffer_bytes = buffer_bytes;
}
static void free_buffer(struct Owner *owner, uint8_t *buffer, size_t capacity) {
  free(buffer);
  --buffers;
  buffer_bytes -= capacity;
  owner->bytes -= capacity;
}
/* No await can occur while this receipt is active. Hold the same credits and
   stream reference as a reserved read, without a pending slot or carrier. */
static int64_t ready_read(struct Handle *h, uint8_t *buffer, size_t length) {
  struct Owner *owner = h->account;
  if (active_ready_read || owner->read.id)
    fatal();
  uintptr_t id = identity(TOKEN_SLOT_MASK - 1);
  owner->read = (struct ReadyRead){id, h, {buffer, length}};
  active_ready_read = &owner->read;
  ++owner->operations;
  ++owner->data_operations;
  ++operation_count;
  ++data_count;
  ++h->reads;
  ++h->refs;
  return (int64_t)id;
}
static int64_t begin(enum Kind kind, void *token, const uint8_t *data, size_t size, bool all,
                     int64_t timeout, bool direct) {
  pthread_mutex_lock(&lock);
  struct Handle *h = handle(token);
  int status = check(h, kind == ACCEPT);
  if (kind == CLOSE && h && pthread_equal(h->owner, pthread_self()) && !h->closing)
    status = 0;
  if (!status && kind != CLOSE && occupied(h, kind))
    status = -7503;
  if (!status && (kind == WRITE || kind == HALF) && h->send_closed)
    status = -7507;
  if (!status && (timeout < 0 || timeout > 30000 ||
                  (kind == READ ? size < 1 || size > CHUNK : kind == WRITE && size > CHUNK)))
    status = -7504;
  size_t capacity = kind == READ ? CHUNK : kind == WRITE ? size : 0;
  if (!status && capacity &&
      (buffer_bytes > BUFFER_LIMIT - capacity || h->account->bytes > OWNER_BUFFER_LIMIT - capacity))
    status = -7505;
  struct InlineAttempt attempt = {0};
  uint8_t *read_buffer = NULL;
  bool attempted = !status && direct && !worker_data_io && (kind == WRITE || !active_ready_read);
  if (attempted) {
    /* Check credit before touching the stream. The lock preserves the
       checked credit if the syscall needs a pending operation. */
    if (!data_credit(h->account))
      status = -7505;
    else if (kind == READ) {
      read_buffer = malloc(capacity);
      if (!read_buffer)
        _Exit(70);
      account_buffer(h->account, capacity);
      if (try_read(h, read_buffer, size, timeout, &attempt)) {
        int64_t id = ready_read(h, read_buffer, attempt.length);
        pthread_mutex_unlock(&lock);
        return id;
      }
    } else if (try_write(h, data, size, all, timeout, &attempt)) {
      pthread_mutex_unlock(&lock);
      return RINZU_SOCKET_WRITE_READY_BASE - (int64_t)attempt.length;
    }
  }
  struct Handle *value = NULL;
  if (!status && kind == ACCEPT) {
    value = reserve_handle();
    if (!value)
      status = -7505;
  }
  struct Operation *op = status ? NULL : reserve(kind, h);
  if (!status && !op) {
    /* The lock preserved the checked credit through the first syscall. */
    if (attempted)
      fatal();
    status = -7505;
  }
  if (status) {
    if (value)
      discard_handle(value);
    pthread_mutex_unlock(&lock);
    return status;
  }
  op->value = value;
  if (value)
    ++value->refs;
  if (read_buffer || (capacity && !attempt.error)) {
    op->capacity = capacity;
    size_t retained = capacity - attempt.length;
    if (read_buffer) {
      op->buffer = read_buffer;
    } else {
      op->buffer = kind == READ ? malloc(retained) : calloc(retained, 1);
      if (!op->buffer)
        _Exit(70);
      account_buffer(op->account, capacity);
    }
    if (kind == WRITE) {
      op->sent_prefix = attempt.length;
      /* Charge the original write size until finish, including for partial
         sends, to preserve the existing admission budget. */
      memcpy(op->buffer, data + attempt.length, retained);
      copied_bytes += retained;
    } else {
      op->maximum = size;
    }
  }
  if (timeout)
    op->deadline = attempted ? attempt.deadline : monotonic() + timeout;
  op->all = all;
  if (kind == CLOSE)
    request_close(h);
  int64_t id = (int64_t)op->id;
  bool pending = true;
  if (attempted || (!worker_data_io && ((kind == WRITE && RINZU_SOCKET_OWNER_WRITE_ALLOWED()) ||
                                        (kind == READ && RINZU_SOCKET_OWNER_READ_ALLOWED())))) {
    if (attempted) {
      op->length = attempt.length;
      op->attempt = false;
      if (attempt.error) {
        publish(op, attempt.error);
        close_handle(h);
      } else {
        arm(op);
      }
    } else {
      struct Budget budget = {.calls = 63};
      advance(op, &budget, true);
    }
    /* Publication ends carrier access. The saved ID selects only its pending
       bitmap bit while lock still excludes physical finish/reuse. */
    size_t slot = ((uintptr_t)id & TOKEN_SLOT_MASK) - 1;
    pending = (pending_operations[slot / 64] >> (slot % 64)) & 1;
  }
  /* A registered operation will be reported by kqueue. An already bounded
     poll also covers deadline and cancellation checks when it stays idle.
     Runnable work and the transition from an indefinite poll still need a wake. */
  if (pending && (!op->registered || !polling_with_timeout))
    wake();
  pthread_mutex_unlock(&lock);
  return id;
}
int64_t rinzu_socket_accept(void *h, int64_t timeout) {
  return begin(ACCEPT, h, NULL, 0, false, timeout, false);
}
int64_t rinzu_socket_read(void *h, int64_t size, int64_t timeout) {
  return begin(READ, h, NULL, (size_t)size, false, timeout, false);
}
int64_t rinzu_socket_read_start(void *h, int64_t size, int64_t timeout) {
  return begin(READ, h, NULL, (size_t)size, false, timeout, true);
}
int64_t rinzu_socket_write(void *h, const uint8_t *data, size_t size, bool all, int64_t timeout) {
  return begin(WRITE, h, data, size, all, timeout, false);
}
int64_t rinzu_socket_write_start(void *h, const uint8_t *data, size_t size, bool all,
                                 int64_t timeout) {
  return begin(WRITE, h, data, size, all, timeout, true);
}
int64_t rinzu_socket_try_write(void *token, const uint8_t *data, size_t size) {
  pthread_mutex_lock(&lock);
  struct Handle *h = handle(token);
  int status = check(h, false);
  if (!status && occupied(h, WRITE))
    status = RINZU_SOCKET_BUSY;
  if (!status && h->send_closed)
    status = RINZU_SOCKET_SEND_CLOSED;
  if (!status && size > CHUNK)
    status = RINZU_SOCKET_ARGUMENT;
  struct InlineAttempt attempt = {0};
  if (!status && !worker_data_io) {
    try_write(h, data, size, false, 0, &attempt);
    if (attempt.error)
      close_handle(h);
  }
  pthread_mutex_unlock(&lock);
  if (status)
    return status;
  if (attempt.error)
    return attempt.error;
  return RINZU_SOCKET_WRITE_READY_BASE - (int64_t)attempt.length;
}
void *rinzu_socket_write_ready(void) {
  /* Already published, so a waiter never takes the notification slot.
     Cancellation may set base.cancelled, which has no producer to notify. */
  static rinzu_native_completion_v2_t ready = {
      .base = {.version = 2, .size = sizeof(rinzu_native_completion_v2_t), .done = 1},
      .notification = RINZU_NATIVE_COMPLETION_DONE,
  };
  return &ready;
}
int64_t rinzu_socket_shutdown(void *h) {
  return begin(HALF, h, NULL, 0, false, 0, false);
}
int64_t rinzu_socket_close(void *h) {
  return begin(CLOSE, h, NULL, 0, false, 0, false);
}
int64_t rinzu_socket_close_listener(void *h) {
  return rinzu_socket_close(h);
}
int64_t rinzu_socket_close_stream(void *h) {
  return rinzu_socket_close(h);
}
void *rinzu_socket_operation(int64_t id) {
  pthread_mutex_lock(&lock);
  struct Operation *op = operation((void *)(uintptr_t)id);
  if (!pthread_equal(op->owner, pthread_self()))
    fatal();
  pthread_mutex_unlock(&lock);
  return (void *)(uintptr_t)id;
}
static struct Operation *owned(void *token) {
  struct Operation *op = operation(token);
  if (!pthread_equal(op->owner, pthread_self()))
    fatal();
  return op;
}
void *rinzu_socket_completion(void *token) {
  pthread_mutex_lock(&lock);
  void *p = &owned(token)->completion;
  pthread_mutex_unlock(&lock);
  return p;
}
/* Private SDK receipt path: the Source frame already owns the positive full
   generation ID. All C entry points still validate generation and Owner. */
void *rinzu_socket_transfer_completion(int64_t id) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned((void *)(uintptr_t)id);
  if (op->kind != READ && op->kind != WRITE)
    fatal();
  void *p = &op->completion;
  pthread_mutex_unlock(&lock);
  return p;
}
const rinzu_native_byte_array_t *rinzu_socket_bytes_view_id(int64_t id) {
  if (((uintptr_t)id & TOKEN_SLOT_MASK) == TOKEN_SLOT_MASK) {
    struct ReadyRead *ready = active_ready_read;
    if (!ready || ready->id != (uintptr_t)id)
      fatal();
    pthread_mutex_lock(&lock);
    copied_bytes += ready->bytes.length;
    pthread_mutex_unlock(&lock);
    return &ready->bytes;
  }
  return rinzu_socket_bytes_view((void *)(uintptr_t)id);
}
static const rinzu_native_byte_array_t *read_access(void *view) {
  if (active_ready_read && active_ready_read->id == (uintptr_t)view)
    return &active_ready_read->bytes;
  for (const struct Operation *op = active_read; op; op = op->previous_read)
    if (op->id == (uintptr_t)view)
      return &op->bytes;
  return NULL;
}
void *rinzu_socket_borrow_read(int64_t id) {
  if (((uintptr_t)id & TOKEN_SLOT_MASK) == TOKEN_SLOT_MASK) {
    if (!active_ready_read || active_ready_read->id != (uintptr_t)id)
      fatal();
    return (void *)(uintptr_t)id;
  }
  pthread_mutex_lock(&lock);
  struct Operation *op = owned((void *)(uintptr_t)id);
  if (op->kind != READ || !op->ready || op->ack || op->status || !op->buffer ||
      op->length > op->initialized || op->length > op->capacity)
    fatal();
  op->ack = true;
  op->previous_read = active_read;
  active_read = op;
  pthread_mutex_unlock(&lock);
  return (void *)(uintptr_t)id;
}
void *rinzu_socket_read_access(void) {
  return (void *)(rinzu_native_read_access_t)read_access;
}
int64_t rinzu_socket_read_length(void *view) {
  const rinzu_native_byte_array_t *read = read_access(view);
  return read ? (int64_t)read->length : -1;
}
int64_t rinzu_socket_read_byte(void *view, int64_t index) {
  const rinzu_native_byte_array_t *read = read_access(view);
  return read && index >= 0 && (uint64_t)index < read->length ? read->data[index] : -1;
}
int64_t rinzu_socket_find_read_byte(void *view, int64_t byte, int64_t start, int64_t end) {
  const rinzu_native_byte_array_t *read = read_access(view);
  if (!read || byte < 0 || byte > UINT8_MAX || start < 0 || end < start ||
      (uint64_t)end > read->length)
    return -2;
  const uint8_t *match = memchr(read->data + start, (int)byte, (size_t)(end - start));
  return match ? (int64_t)(match - read->data) : -1;
}
const rinzu_native_byte_array_t *rinzu_socket_copy_read(void *view, int64_t start, int64_t end) {
  const rinzu_native_byte_array_t *read = read_access(view);
  if (!read || start < 0 || end < start || (uint64_t)end > read->length)
    return NULL;
  read_slice = (rinzu_native_byte_array_t){read->data + start, (size_t)(end - start)};
  pthread_mutex_lock(&lock);
  copied_bytes += read_slice.length;
  pthread_mutex_unlock(&lock);
  return &read_slice;
}
void rinzu_socket_end_read(int64_t id) {
  if (!active_read || active_read->id != (uintptr_t)id)
    fatal();
  active_read = active_read->previous_read;
  rinzu_socket_finish_id(id);
}
void rinzu_socket_ack_id(int64_t id) {
  rinzu_socket_ack((void *)(uintptr_t)id);
}
void rinzu_socket_cancel_id(int64_t id) {
  rinzu_socket_cancel((void *)(uintptr_t)id);
}
void rinzu_socket_finish_id(int64_t id) {
  rinzu_socket_finish((void *)(uintptr_t)id);
}
int32_t rinzu_socket_status(void *token) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ready)
    fatal();
  int32_t n = op->status;
  pthread_mutex_unlock(&lock);
  return n;
}
int64_t rinzu_socket_length(void *token) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ready)
    fatal();
  int64_t n = (int64_t)op->length;
  pthread_mutex_unlock(&lock);
  return n;
}
int64_t rinzu_socket_transfer_result(void *token) {
  _Static_assert(CHUNK < 131072, "transfer result length must fit its low 17 bits");
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ready || (op->kind != READ && op->kind != WRITE) || op->length > CHUNK)
    fatal();
  int64_t result = (int64_t)op->status * INT64_C(131072) + (int64_t)op->length;
  pthread_mutex_unlock(&lock);
  return result;
}
void *rinzu_socket_value(void *token) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ready || op->status || !op->value)
    fatal();
  uintptr_t id = op->value->id;
  pthread_mutex_unlock(&lock);
  return (void *)id;
}
static void *typed_value(void *token, bool listener) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ready || op->status || !op->value || op->value->listener != listener)
    fatal();
  uintptr_t id = op->value->id;
  pthread_mutex_unlock(&lock);
  return (void *)id;
}
void *rinzu_socket_listener_value(void *token) {
  return typed_value(token, true);
}
void *rinzu_socket_stream_value(void *token) {
  return typed_value(token, false);
}
static const uint8_t *read_bytes_prefix(void *token, size_t prefix) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ready || op->kind != READ || !op->buffer || op->length > prefix)
    fatal();
  /* recv initialized [0,length). Expose only initialized bytes, extending the
     zero padding monotonically so earlier prefix snapshots stay immutable. */
  if (op->initialized < prefix) {
    memset(op->buffer + op->initialized, 0, prefix - op->initialized);
    op->initialized = prefix;
  }
  copied_bytes += prefix;
  const uint8_t *bytes = op->buffer;
  pthread_mutex_unlock(&lock);
  return bytes;
}
const rinzu_native_byte_array_t *rinzu_socket_bytes_view(void *token) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ready || op->kind != READ || !op->buffer || op->length > op->capacity)
    fatal();
  copied_bytes += op->length;
  const rinzu_native_byte_array_t *view = &op->bytes;
  pthread_mutex_unlock(&lock);
  return view;
}
const uint8_t *rinzu_socket_bytes(void *token) {
  return read_bytes_prefix(token, CHUNK);
}
const uint8_t *rinzu_socket_bytes_256(void *token) {
  return read_bytes_prefix(token, 256);
}
const uint8_t *rinzu_socket_bytes_4096(void *token) {
  return read_bytes_prefix(token, 4096);
}
const uint8_t *rinzu_socket_bytes_8192(void *token) {
  return read_bytes_prefix(token, 8192);
}
const uint8_t *rinzu_socket_bytes_16384(void *token) {
  return read_bytes_prefix(token, 16384);
}
const uint8_t *rinzu_socket_bytes_32768(void *token) {
  return read_bytes_prefix(token, 32768);
}
void rinzu_socket_ack(void *token) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ready)
    fatal();
  op->ack = true;
  pthread_mutex_unlock(&lock);
}
void rinzu_socket_cancel(void *token) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ack) {
    atomic_store_explicit(&op->completion.base.cancelled, 1, memory_order_release);
    if (op->ready && !op->status) {
      struct Handle *h = op->value ? op->value : op->handle;
      if (h && op->kind != CLOSE)
        request_close(h);
    }
    wake();
  }
  pthread_mutex_unlock(&lock);
}
/* Enter and return with the SDK lock held. */
static void stop_if_idle(void) {
  if (handle_count || operation_count)
    return;
  stopping = true;
  wake();
  pthread_mutex_unlock(&lock);
  if (pthread_join(worker, NULL))
    fatal();
  pthread_mutex_lock(&lock);
  (void)close(queue_fd);
  queue_fd = -1;
  running = stopping = false;
}
void rinzu_socket_end_ready_read(int64_t id, bool delivered) {
  struct ReadyRead *ready = active_ready_read;
  if (!ready || ready->id != (uintptr_t)id)
    fatal();
  pthread_mutex_lock(&lock);
  struct Handle *h = ready->stream;
  struct Owner *owner = h->account;
  if (!delivered) {
    /* A control poll may interrupt the Source task after recv and before its
       consumer has observed the bytes. The stream must not skip that prefix. */
    request_close(h);
    wake();
  }
  active_ready_read = NULL;
  free_buffer(owner, ready->bytes.data, CHUNK);
  *ready = (struct ReadyRead){0};
  --owner->operations;
  --owner->data_operations;
  --operation_count;
  --data_count;
  --h->refs;
  --h->reads;
  release_handle(h);
  stop_if_idle();
  pthread_mutex_unlock(&lock);
}
void rinzu_socket_finish(void *token) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ready || (op->kind == READ && read_access(token)))
    fatal();
  if (op->buffer)
    free_buffer(op->account, op->buffer, op->capacity);
  --op->account->operations;
  op->account->data_operations -= op->kind != CLOSE;
  data_count -= op->kind != CLOSE;
  struct Handle *h = op->handle, *v = op->value;
  if (v && v != h) {
    if (op->status)
      request_close(v);
    --v->refs;
    release_handle(v);
  }
  if (h) {
    if (op->kind == READ || op->kind == ACCEPT)
      --h->reads;
    else if (op->kind == WRITE || op->kind == HALF)
      --h->writes;
    --h->refs;
    release_handle(h);
  }
  op->id = 0;
  op->account = NULL;
  --operation_count;
  stop_if_idle();
  pthread_mutex_unlock(&lock);
}
int64_t rinzu_socket_port(void *token) {
  pthread_mutex_lock(&lock);
  struct Handle *h = handle(token);
  int status = check(h, true);
  struct sockaddr_in address;
  socklen_t size = sizeof(address);
  int64_t port = status ? status
                 : getsockname(h->fd, (struct sockaddr *)&address, &size) < 0
                     ? -failure()
                     : ntohs(address.sin_port);
  pthread_mutex_unlock(&lock);
  return port;
}
bool rinzu_socket_join_idle(void) {
  pthread_mutex_lock(&lock);
  for (size_t i = 0; i < OPS; ++i)
    if (operations[i].id && pthread_equal(operations[i].owner, pthread_self())) {
      pthread_mutex_unlock(&lock);
      return false;
    }
  for (size_t i = 0; i < HANDLES; ++i)
    if (handles[i].id && pthread_equal(handles[i].owner, pthread_self())) {
      release_handle(&handles[i]);
      if (handles[i].id) {
        wake();
        pthread_mutex_unlock(&lock);
        return false;
      }
    }
  if (!running || handle_count || operation_count) {
    pthread_mutex_unlock(&lock);
    return true;
  }
  if (stopping) {
    pthread_mutex_unlock(&lock);
    return false;
  }
  stopping = true;
  wake();
  pthread_mutex_unlock(&lock);
  if (pthread_join(worker, NULL))
    fatal();
  pthread_mutex_lock(&lock);
  (void)close(queue_fd);
  queue_fd = -1;
  running = stopping = false;
  pthread_mutex_unlock(&lock);
  return true;
}
bool rinzu_socket_owner_has_handle(void) {
  pthread_mutex_lock(&lock);
  bool any = false;
  for (size_t i = 0; i < HANDLES; ++i)
    if (handles[i].id && pthread_equal(handles[i].owner, pthread_self()) && handles[i].fd >= 0 &&
        !handles[i].closing)
      any = true;
  pthread_mutex_unlock(&lock);
  return any;
}
void *rinzu_socket_owner_handle(void) {
  pthread_mutex_lock(&lock);
  uintptr_t id = 0;
  for (size_t i = 0; i < HANDLES; ++i)
    if (handles[i].id && pthread_equal(handles[i].owner, pthread_self()) && handles[i].fd >= 0 &&
        !handles[i].closing) {
      id = handles[i].id;
      break;
    }
  pthread_mutex_unlock(&lock);
  if (!id)
    fatal();
  return (void *)id;
}
static uintptr_t owner_value(bool listener) {
  uintptr_t id = 0;
  for (size_t i = 0; i < HANDLES; ++i)
    if (handles[i].id && pthread_equal(handles[i].owner, pthread_self()) && handles[i].fd >= 0 &&
        !handles[i].closing && handles[i].listener == listener) {
      id = handles[i].id;
      break;
    }
  return id;
}
bool rinzu_socket_owner_has_listener(void) {
  pthread_mutex_lock(&lock);
  bool found = owner_value(true) != 0;
  pthread_mutex_unlock(&lock);
  return found;
}
bool rinzu_socket_owner_has_stream(void) {
  pthread_mutex_lock(&lock);
  bool found = owner_value(false) != 0;
  pthread_mutex_unlock(&lock);
  return found;
}
static void *owned_value(bool listener) {
  pthread_mutex_lock(&lock);
  uintptr_t id = owner_value(listener);
  pthread_mutex_unlock(&lock);
  if (!id)
    fatal();
  return (void *)id;
}
void *rinzu_socket_owner_listener(void) {
  return owned_value(true);
}
void *rinzu_socket_owner_stream(void) {
  return owned_value(false);
}
int64_t rinzu_socket_live_handles(void) {
  pthread_mutex_lock(&lock);
  int64_t n = (int64_t)handle_count;
  pthread_mutex_unlock(&lock);
  return n;
}
int64_t rinzu_socket_live_operations(void) {
  pthread_mutex_lock(&lock);
  int64_t n = (int64_t)operation_count;
  pthread_mutex_unlock(&lock);
  return n;
}
int64_t rinzu_socket_live_buffers(void) {
  pthread_mutex_lock(&lock);
  int64_t n = buffers;
  pthread_mutex_unlock(&lock);
  return n;
}
int64_t rinzu_socket_partial_writes(void) {
  pthread_mutex_lock(&lock);
  int64_t n = partial_writes;
  pthread_mutex_unlock(&lock);
  return n;
}
int64_t rinzu_socket_event_count(void) {
  pthread_mutex_lock(&lock);
  int64_t n = event_count;
  pthread_mutex_unlock(&lock);
  return n;
}
int64_t rinzu_socket_thread_count(void) {
  pthread_mutex_lock(&lock);
  int64_t n = running;
  pthread_mutex_unlock(&lock);
  return n;
}
int64_t rinzu_socket_stale_events(void) {
  pthread_mutex_lock(&lock);
  int64_t n = stale_events;
  pthread_mutex_unlock(&lock);
  return n;
}
int64_t rinzu_socket_blocked_writes(void) {
  pthread_mutex_lock(&lock);
  int64_t n = 0;
  for (size_t i = 0; i < OPS; ++i)
    n += operations[i].id && operations[i].kind == WRITE && operations[i].registered &&
         !operations[i].ready;
  pthread_mutex_unlock(&lock);
  return n;
}
int64_t rinzu_socket_stat(int32_t statistic) {
  pthread_mutex_lock(&lock);
  size_t value = 0;
  switch (statistic) {
  case RINZU_SOCKET_LISTENERS:
  case RINZU_SOCKET_STREAMS:
    for (size_t i = 0; i < HANDLES; ++i)
      value += handles[i].id && handles[i].fd >= 0 &&
               handles[i].listener == (statistic == RINZU_SOCKET_LISTENERS);
    break;
  case RINZU_SOCKET_HANDLE_SLOTS:
    value = handle_count;
    break;
  case RINZU_SOCKET_OPERATIONS:
    value = operation_count;
    break;
  case RINZU_SOCKET_PENDING_OPERATIONS:
    value = pending_count;
    break;
  case RINZU_SOCKET_FINISH_WAIT_OPERATIONS:
    value = operation_count - pending_count;
    break;
  case RINZU_SOCKET_DATA_OPERATIONS:
    value = data_count;
    break;
  case RINZU_SOCKET_CONTROL_OPERATIONS:
    value = operation_count - data_count;
    break;
  case RINZU_SOCKET_BUFFERS:
    value = buffers;
    break;
  case RINZU_SOCKET_BUFFER_BYTES:
    value = buffer_bytes;
    break;
  case RINZU_SOCKET_PEAK_BUFFER_BYTES:
    value = peak_buffer_bytes;
    break;
  case RINZU_SOCKET_CONTROL_BYTES:
  case RINZU_SOCKET_RETAINED_CONTROL_BYTES:
    value = sizeof(handles) + sizeof(operations) + sizeof(owners) + sizeof(pending_operations) +
            sizeof(closing_handles);
    break;
  case RINZU_SOCKET_EVENT_THREADS:
    value = running;
    break;
  case RINZU_SOCKET_KEVENT_CALLS:
    value = kevent_calls;
    break;
  case RINZU_SOCKET_READINESS_EVENTS:
    value = event_count;
    break;
  case RINZU_SOCKET_USER_WAKES:
    value = user_wakes;
    break;
  case RINZU_SOCKET_POLL_TIMEOUTS:
    value = poll_timeouts;
    break;
  case RINZU_SOCKET_STALE_EVENTS:
    value = stale_events;
    break;
  case RINZU_SOCKET_READ_CALLS:
    value = read_calls;
    break;
  case RINZU_SOCKET_WRITE_CALLS:
    value = write_calls;
    break;
  case RINZU_SOCKET_PARTIAL_WRITES:
    value = partial_writes;
    break;
  case RINZU_SOCKET_COPY_BYTES:
    value = copied_bytes;
    break;
  case RINZU_SOCKET_ALLOCATIONS:
    value = allocations;
    break;
  case RINZU_SOCKET_RECEIVED_BYTES:
    value = received_bytes;
    break;
  case RINZU_SOCKET_SENT_BYTES:
    value = sent_bytes;
    break;
  default:
    fatal();
  }
  pthread_mutex_unlock(&lock);
  if (value > INT64_MAX)
    fatal();
  return (int64_t)value;
}

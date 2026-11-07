/* Bounded macOS TCP transport. Source wrapper/standard registry: M5.12-8. */
#include "rinzu_socket.h"
#include "rinzu/native_completion.h"
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
_Static_assert(HANDLES <= TOKEN_SLOT_MASK && OPS <= TOKEN_SLOT_MASK,
               "socket tables must fit the token slot bits");
/* Entries are protected by lock and remain stable until all of an Owner's
   handles and operations have been released. Reactor cleanup must not retain
   pointers into an Owner thread's TLS. Every production operation holds a
   handle, so HANDLES entries also bound the number of concurrent Owners. */
struct Owner {
  pthread_t thread;
  size_t handles, operations, data_operations, bytes;
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
  struct sockaddr_in address;
  bool ready, attempt, all, ack, registered;
  int backlog;
  /* Pending READs use the recv limit. Publication ends that lifetime and
     starts the initialized prefix watermark; ready READs never recv again. */
  union {
    size_t maximum;
    size_t initialized;
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
static size_t handle_count, operation_count;
static uint64_t pending_operations[OP_WORDS], closing_handles[HANDLE_WORDS];
static size_t pending_count;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_t worker;
static int queue_fd = -1;
static bool running, stopping, wake_pending, worker_data_io;
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
  rinzu_native_completion_v2_publish(&op->completion);
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
  uint8_t *buffer = allocation + (writing ? op->length : 0);
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
  for (;;) {
    pthread_mutex_lock(&lock);
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
    pthread_mutex_unlock(&lock);
    if (stop)
      break;
    struct kevent events[64];
    struct timespec timeout = {0, runnable ? 0 : 10000000};
    int n = kevent(queue_fd, NULL, 0, events, 64, pending ? &timeout : NULL);
    if (n < 0) {
      if (errno == EINTR)
        continue;
      fatal();
    }
    pthread_mutex_lock(&lock);
    ++kevent_calls;
    if (!n)
      ++poll_timeouts;
    for (int i = 0; i < n; ++i)
      observe_event(&events[i]);
    pthread_mutex_unlock(&lock);
  }
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
  if (pthread_create(&worker, NULL, drive, NULL))
    fatal();
}
static struct Operation *reserve(enum Kind kind, struct Handle *h) {
  if (stopping)
    return NULL;
  struct Owner *account = h ? h->account : owner_account();
  if (!account || (kind != CLOSE && account->data_operations >= OWNER_OPS))
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
static int64_t begin(enum Kind kind, void *token, const uint8_t *data, size_t size, bool all,
                     int64_t timeout) {
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
  struct Handle *value = NULL;
  if (!status && kind == ACCEPT) {
    value = reserve_handle();
    if (!value)
      status = -7505;
  }
  struct Operation *op = status ? NULL : reserve(kind, h);
  if (!status && !op)
    status = -7505;
  if (status) {
    if (value)
      discard_handle(value);
    pthread_mutex_unlock(&lock);
    return status;
  }
  op->value = value;
  if (value)
    ++value->refs;
  if (capacity) {
    op->buffer = kind == READ ? malloc(capacity) : calloc(capacity, 1);
    if (!op->buffer)
      _Exit(70);
    op->capacity = capacity;
    op->maximum = size;
    ++buffers;
    ++allocations;
    buffer_bytes += capacity;
    op->account->bytes += capacity;
    if (buffer_bytes > peak_buffer_bytes)
      peak_buffer_bytes = buffer_bytes;
    if (kind == WRITE) {
      memcpy(op->buffer, data, size);
      copied_bytes += size;
    }
  }
  if (timeout)
    op->deadline = monotonic() + timeout;
  op->all = all;
  if (kind == CLOSE)
    request_close(h);
  int64_t id = (int64_t)op->id;
  bool pending = true;
  if (!worker_data_io && ((kind == WRITE && RINZU_SOCKET_OWNER_WRITE_ALLOWED()) ||
                          (kind == READ && RINZU_SOCKET_OWNER_READ_ALLOWED()))) {
    struct Budget budget = {.calls = 63};
    advance(op, &budget, true);
    /* Publication ends carrier access. The saved ID selects only its pending
       bitmap bit while lock still excludes physical finish/reuse. */
    size_t slot = ((uintptr_t)id & TOKEN_SLOT_MASK) - 1;
    pending = (pending_operations[slot / 64] >> (slot % 64)) & 1;
  }
  if (pending)
    wake();
  pthread_mutex_unlock(&lock);
  return id;
}
int64_t rinzu_socket_accept(void *h, int64_t timeout) {
  return begin(ACCEPT, h, NULL, 0, false, timeout);
}
int64_t rinzu_socket_read(void *h, int64_t size, int64_t timeout) {
  return begin(READ, h, NULL, (size_t)size, false, timeout);
}
int64_t rinzu_socket_write(void *h, const uint8_t *data, size_t size, bool all, int64_t timeout) {
  return begin(WRITE, h, data, size, all, timeout);
}
int64_t rinzu_socket_shutdown(void *h) {
  return begin(HALF, h, NULL, 0, false, 0);
}
int64_t rinzu_socket_close(void *h) {
  return begin(CLOSE, h, NULL, 0, false, 0);
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
  return rinzu_socket_bytes_view((void *)(uintptr_t)id);
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
void rinzu_socket_finish(void *token) {
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(token);
  if (!op->ready)
    fatal();
  if (op->buffer) {
    free(op->buffer);
    --buffers;
    buffer_bytes -= op->capacity;
    op->account->bytes -= op->capacity;
  }
  --op->account->operations;
  op->account->data_operations -= op->kind != CLOSE;
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
  if (!handle_count && !operation_count) {
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
  case RINZU_SOCKET_CONTROL_OPERATIONS:
    for (size_t i = 0; i < OPS; ++i)
      if (operations[i].id) {
        if ((statistic == RINZU_SOCKET_DATA_OPERATIONS && i < DATA_OPS) ||
            (statistic == RINZU_SOCKET_CONTROL_OPERATIONS && i >= DATA_OPS))
          ++value;
      }
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

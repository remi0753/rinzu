#include "rinzu_io.h"
#include "io_pool.h"
#include "rinzu/posix_write.h"
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

enum Operation {
  OPEN_READ,
  OPEN_WRITE,
  READ,
  WRITE,
  FLUSH,
  CLOSE,
  RENAME,
  REMOVE,
  DUP,
  WRITE_ALL,
  OPEN_TEMPORARY,
  INSTALL_TEMPORARY
};
struct File {
  rinzu_native_completion_v2_t completion;
  pthread_t owner; /* Immutable, checked before any mutable access. */
  int fd;
  int standard;
  bool pipe;
  size_t atomic_write;
  bool owns, busy, ready, cancelled;
  enum Operation operation;
  int32_t status;
  size_t length;
  uint8_t *buffer;
  char *from, *to, *temporary_path;
  struct File *next_owner;
};
enum { THREADS = 2, QUEUE = 32 };
struct Job {
  void (*run)(void *);
  void *context;
  bool reserved;
};
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static pthread_t workers[THREADS];
static struct Job queue[QUEUE];
/* Slots count queued ordinary jobs and all whole-file reservations, even idle ones. */
static size_t head, queued, slots, operations, owners, buffers, retained;
static bool running, stopping;
static _Thread_local struct File *owner_files;

static int32_t check(struct File *file) {
  if (!file)
    fatal();
  return pthread_equal(file->owner, pthread_self()) ? 0 : RINZU_IO_OWNER;
}
static bool cancelled(struct File *file) {
  pthread_mutex_lock(&lock);
  bool value = file->cancelled ||
               atomic_load_explicit(&file->completion.base.cancelled, memory_order_acquire);
  pthread_mutex_unlock(&lock);
  return value;
}
static int32_t pipe_ready(struct File *file, short events) {
  struct pollfd descriptor = {.fd = file->fd, .events = events};
  for (;;) {
    if (cancelled(file))
      return ECANCELED;
    int result = poll(&descriptor, 1, 10);
    if (result < 0) {
      if (errno == EINTR)
        continue;
      return native_error();
    }
    if (!result)
      continue;
    if (descriptor.revents & POLLNVAL)
      return EBADF;
    if (events == POLLOUT && (descriptor.revents & (POLLHUP | POLLERR)))
      return EPIPE;
    if (descriptor.revents & (events | POLLHUP | POLLERR))
      return 0;
  }
}
static void perform(void *context) {
  struct File *file = context;
  int32_t status = 0;
  size_t length = 0;
  if (cancelled(file) && file->operation != CLOSE) {
    status = ECANCELED;
  } else if (file->operation == DUP) {
    struct stat info;
    int fd = -1;
    if (file->standard != STDIN_FILENO && file->standard != STDOUT_FILENO)
      status = RINZU_IO_ARGUMENT;
    else {
      do {
        fd = fcntl(file->standard, F_DUPFD_CLOEXEC, 3);
      } while (fd < 0 && errno == EINTR);
      if (fd < 0)
        status = native_error();
      else if (fstat(fd, &info))
        status = native_error();
      else if (!S_ISREG(info.st_mode) && !S_ISFIFO(info.st_mode))
        status = RINZU_IO_REGULAR;
      else {
        file->pipe = S_ISFIFO(info.st_mode);
        if (file->pipe) {
          long maximum = fpathconf(fd, _PC_PIPE_BUF);
          if (maximum < 1)
            status = EINVAL;
          else
            file->atomic_write =
                (size_t)maximum < RINZU_IO_CHUNK ? (size_t)maximum : RINZU_IO_CHUNK;
        }
      }
      if (status && fd >= 0) {
        (void)close(fd);
        fd = -1;
      }
      pthread_mutex_lock(&lock);
      file->fd = fd;
      pthread_mutex_unlock(&lock);
    }
  } else if (file->operation == OPEN_TEMPORARY) {
    int fd = mkstemp(file->from);
    if (fd < 0)
      status = native_error();
    else if (fcntl(fd, F_SETFD, FD_CLOEXEC)) {
      status = native_error();
      (void)close(fd);
      (void)unlink(file->from);
    } else {
      pthread_mutex_lock(&lock);
      file->fd = fd;
      file->temporary_path = file->from;
      file->from = NULL;
      pthread_mutex_unlock(&lock);
    }
  } else if (file->operation == OPEN_READ || file->operation == OPEN_WRITE) {
    struct stat info;
    /* Reject existing symlinks/devices before any destructive open. */
    if (lstat(file->from, &info) == 0 && !S_ISREG(info.st_mode)) {
      status = RINZU_IO_REGULAR;
    } else {
      int flags = file->operation == OPEN_READ ? O_RDONLY : O_WRONLY | O_CREAT;
      int fd;
      do {
        fd = open(file->from, flags | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK, 0600);
      } while (fd < 0 && errno == EINTR);
      if (fd < 0)
        status = native_error();
      else if (fstat(fd, &info) || !S_ISREG(info.st_mode)) {
        status = RINZU_IO_REGULAR;
        (void)close(fd);
      } else if (file->operation == OPEN_WRITE && ftruncate(fd, 0)) {
        status = native_error();
        (void)close(fd);
      } else {
        pthread_mutex_lock(&lock);
        file->fd = fd;
        pthread_mutex_unlock(&lock);
      }
    }
  } else if (file->operation == WRITE_ALL) {
    while (length < file->length) {
      if (cancelled(file)) {
        status = ECANCELED;
        break;
      }
      size_t maximum = file->length - length;
      if (file->pipe) {
        if (maximum > file->atomic_write)
          maximum = file->atomic_write;
        status = pipe_ready(file, POLLOUT);
        if (status)
          break;
      }
      ssize_t count = rinzu_posix_write(file->fd, file->buffer + length, maximum);
      if (count < 0) {
        if (errno == EINTR || (file->pipe && (errno == EAGAIN || errno == EWOULDBLOCK)))
          continue;
        status = native_error();
        break;
      }
      if (!count) {
        status = RINZU_IO_PROGRESS;
        break;
      }
      length += (size_t)count;
    }
  } else if (file->operation == READ || file->operation == WRITE) {
    size_t maximum = file->length;
    if (file->pipe && file->operation == WRITE && maximum > file->atomic_write)
      maximum = file->atomic_write;
    ssize_t count = 0;
    for (;;) {
      if (file->pipe && maximum)
        status = pipe_ready(file, file->operation == READ ? POLLIN : POLLOUT);
      if (status)
        break;
      count = file->operation == READ ? read(file->fd, file->buffer, file->length)
                                      : rinzu_posix_write(file->fd, file->buffer, maximum);
      if (count >= 0 ||
          (errno != EINTR && !(file->pipe && (errno == EAGAIN || errno == EWOULDBLOCK))))
        break;
    }
    if (!status && count < 0)
      status = native_error();
    else if (!status)
      length = (size_t)count;
  } else if (file->operation == FLUSH) {
    int result = 0;
    do {
      if (!file->pipe)
        result = fsync(file->fd);
    } while (result && errno == EINTR);
    if (result)
      status = native_error();
  } else if (file->operation == CLOSE) {
    /* On Darwin close consumes the descriptor. Never retry an fd number that
       another thread could reuse. Keep the failure even though owner is closed. */
    if (close(file->fd))
      status = native_error();
    if (file->temporary_path) {
      if (unlink(file->temporary_path) && errno != ENOENT && !status)
        status = native_error();
      free(file->temporary_path);
      file->temporary_path = NULL;
    }
    pthread_mutex_lock(&lock);
    file->fd = -1;
    pthread_mutex_unlock(&lock);
  } else if (file->operation == INSTALL_TEMPORARY) {
    if (rename(file->temporary_path, file->to))
      status = native_error();
    else {
      free(file->temporary_path);
      file->temporary_path = NULL;
    }
  } else {
    int result = file->operation == RENAME ? rename(file->from, file->to) : unlink(file->from);
    if (result)
      status = native_error();
  }
  pthread_mutex_lock(&lock);
  file->status = status;
  file->length = length;
  file->ready = true;
  rinzu_native_completion_v2_publish(&file->completion);
  pthread_cond_broadcast(&changed);
  pthread_mutex_unlock(&lock);
}
static void *worker(void *unused) {
  (void)unused;
  for (;;) {
    pthread_mutex_lock(&lock);
    while (!queued && !stopping)
      pthread_cond_wait(&changed, &lock);
    if (!queued && stopping) {
      pthread_mutex_unlock(&lock);
      return NULL;
    }
    struct Job job = queue[head];
    head = (head + 1) % QUEUE;
    queued--;
    if (!job.reserved)
      slots--;
    pthread_mutex_unlock(&lock);
    job.run(job.context);
  }
}
static int32_t admit_job(void) { /* lock held */
  while (stopping)
    pthread_cond_wait(&changed, &lock);
  if (slots == QUEUE)
    return RINZU_IO_SATURATED;
  if (running)
    return 0;
  running = true;
  for (size_t index = 0; index < THREADS; index++) {
    int result = pthread_create(&workers[index], NULL, worker, NULL);
    if (!result)
      continue;
    if (result == ENOMEM)
      _Exit(70);
    stopping = true;
    pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&lock);
    for (size_t started = 0; started < index; started++)
      if (pthread_join(workers[started], NULL))
        fatal();
    pthread_mutex_lock(&lock);
    running = stopping = false;
    pthread_cond_broadcast(&changed);
    return result;
  }
  return 0;
}
static void enqueue(struct Job job) { /* lock held */
  if (queued == QUEUE)
    fatal();
  operations++;
  queue[(head + queued) % QUEUE] = job;
  queued++;
  pthread_cond_signal(&changed);
}

static void unlock_and_stop_if_idle(void) { /* lock held */
  bool stop = running && !stopping && !operations && !owners && !slots;
  if (stop) {
    stopping = true;
    pthread_cond_broadcast(&changed);
  }
  pthread_mutex_unlock(&lock);
  if (stop) {
    for (size_t index = 0; index < THREADS; index++)
      pthread_join(workers[index], NULL);
    pthread_mutex_lock(&lock);
    running = false;
    stopping = false;
    pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&lock);
  }
}

int32_t rinzu_io_reserve(size_t control_bytes) {
  pthread_mutex_lock(&lock);
  int32_t status = admit_job();
  if (!status) {
    slots++;
    retained += control_bytes;
  }
  pthread_mutex_unlock(&lock);
  return status;
}

void rinzu_io_submit_reserved(void (*run)(void *), void *context) {
  pthread_mutex_lock(&lock);
  enqueue((struct Job){.run = run, .context = context, .reserved = true});
  pthread_mutex_unlock(&lock);
}

void rinzu_io_publish_reserved(rinzu_native_completion_v2_t *completion) {
  pthread_mutex_lock(&lock);
  operations--;
  rinzu_native_completion_v2_publish(completion);
  pthread_mutex_unlock(&lock);
}

void rinzu_io_release_reservation(size_t control_bytes) {
  pthread_mutex_lock(&lock);
  slots--;
  retained -= control_bytes;
  unlock_and_stop_if_idle();
}

void rinzu_io_track_resource(bool file, bool acquired) {
  pthread_mutex_lock(&lock);
  size_t *count = file ? &owners : &buffers;
  if (acquired)
    (*count)++;
  else
    (*count)--;
  pthread_mutex_unlock(&lock);
}

static int32_t submit(struct File *file, enum Operation operation) { /* lock held */
  if (file->busy)
    return RINZU_IO_BUSY;
  int32_t pool_status = admit_job();
  if (pool_status)
    return pool_status;
  file->operation = operation;
  file->busy = true;
  file->ready = false;
  file->cancelled = false;
  rinzu_native_completion_v2_init(&file->completion);
  file->status = 0;
  slots++;
  enqueue((struct Job){.run = perform, .context = file});
  return 0;
}
static struct File *new_file(bool owns) {
  struct File *file = allocate(sizeof(*file));
  file->owner = pthread_self();
  file->fd = -1;
  file->standard = -1;
  file->owns = owns;
  file->next_owner = owner_files;
  owner_files = file;
  rinzu_native_completion_v2_init(&file->completion);
  pthread_mutex_lock(&lock);
  retained += sizeof(*file);
  if (owns)
    owners++;
  pthread_mutex_unlock(&lock);
  return file;
}
static bool owner_active(struct File *file) {
  return file->owns || file->busy || file->ready || file->from || file->to || file->buffer;
}
bool rinzu_io_owner_has_file(void) {
  pthread_mutex_lock(&lock);
  bool found = false;
  for (struct File *file = owner_files; file; file = file->next_owner) {
    if (owner_active(file)) {
      found = true;
      break;
    }
  }
  pthread_mutex_unlock(&lock);
  return found;
}
void *rinzu_io_owner_file(void) {
  pthread_mutex_lock(&lock);
  struct File *found = NULL;
  for (struct File *file = owner_files; file; file = file->next_owner) {
    if (owner_active(file)) {
      found = file;
      break;
    }
  }
  pthread_mutex_unlock(&lock);
  if (!found)
    fatal();
  return found;
}
bool rinzu_io_pending(void *value) {
  struct File *file = value;
  if (check(file))
    fatal();
  pthread_mutex_lock(&lock);
  bool pending = file->busy || file->ready;
  pthread_mutex_unlock(&lock);
  return pending;
}
int32_t rinzu_io_ready_status(void *value) {
  struct File *file = value;
  int32_t status = check(file);
  if (status)
    return status;
  pthread_mutex_lock(&lock);
  /* Do not read fd while an OPEN/DUP/CLOSE worker can still write it. */
  status = !file->owns                 ? RINZU_IO_CLOSED
           : file->busy || file->ready ? RINZU_IO_BUSY
           : file->fd < 0              ? RINZU_IO_CLOSED
                                       : 0;
  pthread_mutex_unlock(&lock);
  return status;
}
static void *open_file(const char *path, enum Operation operation) {
  struct File *file = new_file(true);
  file->from = copy_text(path);
  pthread_mutex_lock(&lock);
  int32_t status = submit(file, operation);
  if (status) {
    file->status = status;
    file->ready = true;
    rinzu_native_completion_v2_publish(&file->completion);
  }
  pthread_mutex_unlock(&lock);
  return file;
}
void *rinzu_io_open(const char *path, int32_t writing) {
  return open_file(path, writing ? OPEN_WRITE : OPEN_READ);
}
void *rinzu_io_temporary(const char *template_path) {
  return open_file(template_path, OPEN_TEMPORARY);
}
int32_t rinzu_io_install_temporary(void *value, const char *destination) {
  struct File *file = value;
  int32_t status = check(file);
  if (status)
    return status;
  pthread_mutex_lock(&lock);
  if (!file->owns || file->fd < 0)
    status = RINZU_IO_CLOSED;
  else if (file->busy || file->ready)
    status = RINZU_IO_BUSY;
  else if (!file->temporary_path || !destination || !*destination)
    status = RINZU_IO_ARGUMENT;
  else {
    file->to = copy_text(destination);
    status = submit(file, INSTALL_TEMPORARY);
    if (status) {
      free(file->to);
      file->to = NULL;
    }
  }
  pthread_mutex_unlock(&lock);
  return status;
}
void *rinzu_io_standard(int32_t stream) {
  struct File *file = new_file(true);
  file->standard = stream;
  pthread_mutex_lock(&lock);
  int32_t status = submit(file, DUP);
  if (status) {
    file->status = status;
    file->ready = true;
    rinzu_native_completion_v2_publish(&file->completion);
  }
  pthread_mutex_unlock(&lock);
  return file;
}
static int32_t start(void *value, enum Operation operation, const uint8_t *data, size_t length) {
  struct File *file = value;
  int32_t status = check(file);
  if (status)
    return status;
  if (length > RINZU_IO_CHUNK || (operation == READ && !length))
    return RINZU_IO_ARGUMENT;
  pthread_mutex_lock(&lock);
  if (!file->owns)
    status = RINZU_IO_CLOSED;
  else if (file->busy)
    status = RINZU_IO_BUSY;
  else if (file->fd < 0)
    status = RINZU_IO_CLOSED;
  else if (slots == QUEUE)
    status = RINZU_IO_SATURATED;
  else {
    if (operation == READ || operation == WRITE || operation == WRITE_ALL) {
      file->buffer = allocate(RINZU_IO_CHUNK);
      buffers++;
      if (operation != READ && length)
        memcpy(file->buffer, data, length);
    }
    file->length = length;
    status = submit(file, operation);
    if (status && file->buffer) {
      free(file->buffer);
      file->buffer = NULL;
      buffers--;
    }
  }
  pthread_mutex_unlock(&lock);
  return status;
}
int32_t rinzu_io_read(void *file, int64_t maximum) {
  if (maximum <= 0 || maximum > RINZU_IO_CHUNK)
    return RINZU_IO_ARGUMENT;
  return start(file, READ, NULL, (size_t)maximum);
}
int32_t rinzu_io_write(void *file, const uint8_t *data, size_t length) {
  return start(file, WRITE, data, length);
}
int32_t rinzu_io_write_all(void *file, const uint8_t *data, size_t length) {
  return start(file, WRITE_ALL, data, length);
}
int32_t rinzu_io_flush(void *file) {
  return start(file, FLUSH, NULL, 0);
}
int32_t rinzu_io_close(void *file) {
  return start(file, CLOSE, NULL, 0);
}
void *rinzu_io_path(const char *from, const char *to, int32_t removing) {
  struct File *file = new_file(false);
  file->from = copy_text(from);
  file->to = copy_text(to);
  pthread_mutex_lock(&lock);
  int32_t status = submit(file, removing ? REMOVE : RENAME);
  if (status) {
    file->status = status;
    file->ready = true;
    rinzu_native_completion_v2_publish(&file->completion);
  }
  pthread_mutex_unlock(&lock);
  return file;
}
void *rinzu_io_completion(void *value) {
  struct File *file = value;
  if (check(file))
    fatal();
  return &file->completion;
}
bool rinzu_io_ready(void *value) {
  struct File *file = value;
  if (check(file))
    fatal();
  pthread_mutex_lock(&lock);
  bool ready = file->ready;
  pthread_mutex_unlock(&lock);
  return ready;
}
int32_t rinzu_io_status(void *value) {
  struct File *file = value;
  int32_t status = check(file);
  if (status)
    return status;
  pthread_mutex_lock(&lock);
  status = file->ready ? file->status : RINZU_IO_BUSY;
  pthread_mutex_unlock(&lock);
  return status;
}
int64_t rinzu_io_length(void *value) {
  struct File *file = value;
  if (check(file) || !rinzu_io_ready(file))
    fatal();
  return (int64_t)file->length;
}
const uint8_t *rinzu_io_bytes(void *value) {
  struct File *file = value;
  if (check(file) || !rinzu_io_ready(file) || file->operation != READ || !file->buffer)
    fatal();
  return file->buffer;
}
void rinzu_io_cancel(void *value) {
  struct File *file = value;
  if (check(file))
    fatal();
  pthread_mutex_lock(&lock);
  file->cancelled = true;
  pthread_mutex_unlock(&lock);
}
void rinzu_io_finish(void *value) {
  struct File *file = value;
  if (check(file))
    fatal();
  pthread_mutex_lock(&lock);
  if (!file->ready)
    fatal();
  if (file->busy) {
    operations--;
    file->busy = false;
  }
  free(file->from);
  file->from = NULL;
  free(file->to);
  file->to = NULL;
  if (file->buffer) {
    free(file->buffer);
    file->buffer = NULL;
    buffers--;
  }
  if (file->owns && file->fd < 0) {
    owners--;
    file->owns = false;
  }
  file->ready = false;
  unlock_and_stop_if_idle();
}
static int64_t millis(clockid_t kind) {
  struct timespec time;
  if (clock_gettime(kind, &time) || time.tv_sec > INT64_MAX / 1000)
    fatal();
  return (int64_t)time.tv_sec * 1000 + time.tv_nsec / 1000000;
}
int64_t rinzu_io_monotonic(void) {
  return millis(CLOCK_MONOTONIC);
}
int64_t rinzu_io_unix(void) {
  return millis(CLOCK_REALTIME);
}
int64_t rinzu_io_live_files(void) {
  pthread_mutex_lock(&lock);
  int64_t value = (int64_t)owners;
  pthread_mutex_unlock(&lock);
  return value;
}
int64_t rinzu_io_live_buffers(void) {
  pthread_mutex_lock(&lock);
  int64_t value = (int64_t)buffers;
  pthread_mutex_unlock(&lock);
  return value;
}
int64_t rinzu_io_live_operations(void) {
  pthread_mutex_lock(&lock);
  int64_t value = (int64_t)operations;
  pthread_mutex_unlock(&lock);
  return value;
}
int64_t rinzu_io_retained_controls(void) {
  pthread_mutex_lock(&lock);
  int64_t value = (int64_t)retained;
  pthread_mutex_unlock(&lock);
  return value;
}

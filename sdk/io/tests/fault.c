#include "rinzu/source_async.h"
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static _Atomic int read_fault, write_fault, flush_fault, create_failure, created;
static _Atomic int hold_reads, entered;
static ssize_t probe_read(int fd, void *data, size_t size) {
  atomic_fetch_add(&entered, 1);
  while (atomic_load(&hold_reads)) {
    struct timespec pause = {0, 1000000};
    nanosleep(&pause, NULL);
  }
  int fault = atomic_exchange(&read_fault, 0);
  if (fault) {
    errno = fault;
    return -1;
  }
  return read(fd, data, size > 3 ? 3 : size);
}
static ssize_t probe_write(int fd, const void *data, size_t size) {
  int fault = atomic_exchange(&write_fault, 0);
  if (fault) {
    errno = fault;
    return -1;
  }
  return write(fd, data, size > 3 ? 3 : size);
}
static int probe_flush(int fd) {
  int fault = atomic_exchange(&flush_fault, 0);
  if (fault) {
    errno = fault;
    return -1;
  }
  return fsync(fd);
}
static int probe_open(const char *path, int flags, ...) {
  if (strstr(path, "deny-permission")) {
    errno = EACCES;
    return -1;
  }
  mode_t mode = 0;
  if (flags & O_CREAT) {
    va_list args;
    va_start(args, flags);
    mode = va_arg(args, int);
    va_end(args);
  }
  return open(path, flags, mode);
}
static int probe_create(pthread_t *thread, const pthread_attr_t *attr, void *(*start)(void *),
                        void *context) {
  int index = atomic_fetch_add(&created, 1) + 1;
  if (index == atomic_load(&create_failure))
    return EAGAIN;
  return pthread_create(thread, attr, start, context);
}
#define read probe_read
#define write probe_write
#define fsync probe_flush
#define open probe_open
#define pthread_create probe_create
#include "../native/src/io.c"
#undef read
#undef write
#undef fsync
#undef open
#undef pthread_create
static void ready(void *file) {
  int64_t limit = rinzu_io_monotonic() + 5000;
  while (!rinzu_io_ready(file)) {
    assert(rinzu_io_monotonic() < limit);
    struct timespec pause = {0, 1000000};
    nanosleep(&pause, NULL);
  }
}
static void status_finish(void *file, int32_t expected) {
  ready(file);
  assert(rinzu_io_status(file) == expected);
  rinzu_io_finish(file);
}
static void *opened(const char *path, int writing) {
  void *file = rinzu_io_open(path, writing);
  status_finish(file, 0);
  return file;
}
static void closed(void *file) {
  assert(rinzu_io_close(file) == 0);
  status_finish(file, 0);
  assert(rinzu_io_close(file) == RINZU_IO_CLOSED);
}
static void *wrong_owner(void *file) {
  assert(!rinzu_io_owner_has_file());
  assert(rinzu_io_ready_status(file) == RINZU_IO_OWNER);
  assert(rinzu_io_read(file, 1) == RINZU_IO_OWNER);
  return NULL;
}
static void timer_run(rinzu_async_future_t *future) {
  (void)future;
  struct timespec pause = {0, 1000000};
  nanosleep(&pause, NULL);
}
int main(void) {
  atomic_store(&create_failure, 2); /* A partially created pool must roll back. */
  void *failed = rinzu_io_open("input.bin", 0);
  status_finish(failed, EAGAIN);
  assert(rinzu_io_live_files() == 0 && !running);
  atomic_store(&create_failure, 0);
  void *out = opened("input.bin", 1);
  const uint8_t input[] = {0, 255, 1, 127, 97, 98, 99};
  size_t offset = 0;
  while (offset < sizeof(input)) {
    assert(rinzu_io_write(out, input + offset, sizeof(input) - offset) == 0);
    ready(out);
    assert(rinzu_io_status(out) == 0 && rinzu_io_length(out) > 0 && rinzu_io_length(out) <= 3);
    offset += (size_t)rinzu_io_length(out);
    rinzu_io_finish(out);
  }
  assert(rinzu_io_write(out, NULL, 0) == 0);
  ready(out);
  assert(rinzu_io_length(out) == 0);
  rinzu_io_finish(out);
  for (size_t i = 0; i < 2; i++) {
    atomic_store(&write_fault, i ? ENOSPC : EIO);
    assert(!rinzu_io_write(out, input, 1));
    status_finish(out, i ? ENOSPC : EIO);
  }
  atomic_store(&flush_fault, EIO);
  assert(!rinzu_io_flush(out));
  status_finish(out, EIO);
  assert(!rinzu_io_flush(out));
  status_finish(out, 0);
  closed(out);
  void *file = opened("input.bin", 0);
  assert(rinzu_io_owner_has_file() && !rinzu_io_pending(file));
  assert(rinzu_io_ready_status(file) == 0 && !rinzu_io_live_operations());
  atomic_store(&read_fault, EINTR);
  assert(!rinzu_io_read(file, 7));
  ready(file);
  assert(!rinzu_io_status(file) && rinzu_io_length(file) == 3 &&
         memcmp(rinzu_io_bytes(file), input, 3) == 0);
  assert(rinzu_io_close(file) == RINZU_IO_BUSY);
  rinzu_io_finish(file);
  assert(rinzu_io_read(file, 0) == RINZU_IO_ARGUMENT &&
         rinzu_io_read(file, 65537) == RINZU_IO_ARGUMENT);
  for (size_t i = 0; i < 3; i++) {
    assert(!rinzu_io_read(file, 7));
    ready(file);
    assert(!rinzu_io_status(file));
    if (i == 2)
      assert(!rinzu_io_length(file));
    rinzu_io_finish(file);
  }
  atomic_store(&read_fault, EIO);
  assert(!rinzu_io_read(file, 1));
  status_finish(file, EIO);
  pthread_t other;
  assert(!pthread_create(&other, NULL, wrong_owner, file));
  assert(!pthread_join(other, NULL));
  closed(file);
  failed = rinzu_io_open("missing.bin", 0);
  status_finish(failed, ENOENT);
  failed = rinzu_io_open("deny-permission.bin", 0);
  status_finish(failed, EACCES);
  failed = rinzu_io_open(".", 0);
  status_finish(failed, RINZU_IO_REGULAR);
  assert(!symlink("input.bin", "link.bin"));
  failed = rinzu_io_open("link.bin", 0);
  status_finish(failed, RINZU_IO_REGULAR);
  void *files[35];
  for (size_t i = 0; i < 35; i++)
    files[i] = opened("input.bin", 0);
  atomic_store(&hold_reads, 1);
  atomic_store(&entered, 0);
  assert(!rinzu_io_read(files[0], 1));
  assert(!rinzu_io_read(files[1], 1));
  int64_t limit = rinzu_io_monotonic() + 5000;
  while (atomic_load(&entered) < 2) {
    assert(rinzu_io_monotonic() < limit);
    struct timespec pause = {0, 1000000};
    nanosleep(&pause, NULL);
  }
  for (size_t i = 2; i < 34; i++)
    assert(!rinzu_io_read(files[i], 1));
  assert(rinzu_io_read(files[34], 1) == RINZU_IO_SATURATED);
  assert(rinzu_io_live_buffers() == 34);
  rinzu_io_cancel(files[0]);
  assert(!rinzu_io_ready(files[0]));
  assert(rinzu_io_close(files[0]) == RINZU_IO_BUSY);
  assert(rinzu_io_ready_status(files[0]) == RINZU_IO_BUSY && rinzu_io_pending(files[0]));
  /* Main's independent timer runs while both native file workers are blocked. */
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_async_future_t *timer = rinzu_async_new(&runtime, RINZU_ASYNC_TIMER, RINZU_TRANSFER_UNIT,
                                                (rinzu_source_position_t){0});
  timer->run = timer_run;
  rinzu_async_start(timer);
  while (!rinzu_async_try_collect(timer)) {
    uint64_t epoch = rinzu_async_completion_epoch(&runtime);
    if (!rinzu_async_has_completion(&runtime))
      rinzu_async_wait_progress(&runtime, epoch);
  }
  rinzu_async_release(timer);
  rinzu_async_dispose(&runtime);
  assert(!rinzu_io_ready(files[0]) && rinzu_io_live_buffers() == 34);
  atomic_store(&hold_reads, 0);
  for (size_t i = 0; i < 34; i++) {
    ready(files[i]);
    assert(!rinzu_io_status(files[i]));
    rinzu_io_finish(files[i]);
  }
  assert(!rinzu_io_live_buffers());
  for (size_t i = 0; i < 35; i++)
    closed(files[i]);
  assert(!rinzu_io_live_files() && !operations && !queued && !running);
  assert(!rinzu_io_owner_has_file() && !rinzu_io_live_operations());
  unlink("input.bin");
  unlink("link.bin");
  puts("io-fault-ok");
}

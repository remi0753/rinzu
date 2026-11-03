/* Test-only replacement translation unit for the ordinary source SDK. The
   application, descriptors and native entry stubs are generated normally. */
#include <errno.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <stdio.h>
#include <pthread.h>
#include <unistd.h>
static _Atomic unsigned long long read_bytes, write_bytes;
static _Atomic int close_failed;
static _Atomic int threads_created;
static unsigned long long setting(const char *name, unsigned long long default_value) {
  const char *value = getenv(name);
  return value ? strtoull(value, NULL, 10) : default_value;
}
static size_t limited(size_t length, const char *name) {
  unsigned long long maximum = setting(name, 65536);
  return maximum < length ? (size_t)maximum : length;
}
static ssize_t short_read(int fd, void *bytes, size_t length) {
  unsigned long long limit = setting("RINZU_TEST_READ_FAIL_AFTER", ~0ULL);
  unsigned long long done = atomic_load(&read_bytes);
  if (done >= limit) {
    errno = EIO;
    return -1;
  }
  length = limited(length, "RINZU_TEST_READ_LIMIT");
  if (limit - done < length)
    length = (size_t)(limit - done);
  ssize_t count = read(fd, bytes, length);
  if (count > 0)
    atomic_fetch_add(&read_bytes, (unsigned long long)count);
  return count;
}
static ssize_t short_write(int fd, const void *bytes, size_t length) {
  if (setting("RINZU_TEST_WRITE_ZERO", 0))
    return 0;
  unsigned long long limit = setting("RINZU_TEST_WRITE_FAIL_AFTER", ~0ULL);
  unsigned long long done = atomic_load(&write_bytes);
  if (done >= limit) {
    errno = ENOSPC;
    return -1;
  }
  length = limited(length, "RINZU_TEST_WRITE_LIMIT");
  if (limit - done < length)
    length = (size_t)(limit - done);
  ssize_t count = write(fd, bytes, length);
  if (count > 0)
    atomic_fetch_add(&write_bytes, (unsigned long long)count);
  return count;
}
static int failing_flush(int fd) {
  if (setting("RINZU_TEST_FLUSH_FAIL", 0)) {
    errno = EIO;
    return -1;
  }
  return fsync(fd);
}
static int failing_close(int fd) {
  int status = close(fd);
  int number = atomic_fetch_add(&close_failed, 1) + 1;
  if (!status && (unsigned long long)number <= setting("RINZU_TEST_CLOSE_FAIL", 0)) {
    errno = EIO;
    return -1;
  }
  return status;
}
static int failing_rename(const char *from, const char *to) {
  if (setting("RINZU_TEST_RENAME_FAIL", 0)) {
    errno = EACCES;
    return -1;
  }
  return rename(from, to);
}
static int failing_create(pthread_t *thread, const pthread_attr_t *attr, void *(*body)(void *),
                          void *context) {
  int number = atomic_fetch_add(&threads_created, 1) + 1;
  if ((unsigned long long)number == setting("RINZU_TEST_CREATE_FAIL", 0))
    return EAGAIN;
  return pthread_create(thread, attr, body, context);
}
static void *failing_calloc(size_t count, size_t size) {
  if (setting("RINZU_TEST_OOM", 0))
    return NULL;
  return calloc(count, size);
}
#define read short_read
#define write short_write
#define fsync failing_flush
#define close failing_close
#define rename failing_rename
#define pthread_create failing_create
#define calloc failing_calloc
#include "../native/src/io.c"
#undef read
#undef write
#undef fsync
#undef close
#undef rename
#undef pthread_create
#undef calloc

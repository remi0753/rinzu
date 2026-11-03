#include "process.h"
#include "rinzu/posix_write.h"
#include <stdlib.h>
#include <string.h>

enum { CONFIGURATION_BYTES = 4096 };
static _Thread_local uint8_t snapshot[CONFIGURATION_BYTES];

int64_t rinzu_process_environment_read(const char *name) {
  memset(snapshot, 0, sizeof(snapshot));
  if (!name || !*name || strchr(name, '='))
    return -2;
  const char *value = getenv(name);
  if (!value)
    return -1;
  size_t length = strnlen(value, sizeof(snapshot) + 1);
  if (length > sizeof(snapshot))
    return -2;
  memcpy(snapshot, value, length);
  return (int64_t)length;
}
const uint8_t *rinzu_process_environment_bytes(void) {
  return snapshot;
}
int64_t rinzu_process_identifier(void) {
  return (int64_t)getpid();
}
int32_t rinzu_process_error_line(const uint8_t *bytes, size_t length) {
  if (!pthread_main_np() || length > CONFIGURATION_BYTES)
    return -611;
  size_t offset = 0;
  while (offset < length) {
    ssize_t count = rinzu_posix_write(STDERR_FILENO, bytes + offset, length - offset);
    if (count < 0) {
      if (errno == EINTR)
        continue;
      if (errno == ENOMEM)
        _Exit(70);
      return errno ? errno : EIO;
    }
    if (!count)
      return EIO;
    offset += (size_t)count;
  }
  return 0;
}

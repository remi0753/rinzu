#ifndef RINZU_IO_H
#define RINZU_IO_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
enum { RINZU_IO_CHUNK = 65536 };
enum RinzuIoStatus {
  RINZU_IO_OWNER = -7101,
  RINZU_IO_CLOSED = -7102,
  RINZU_IO_BUSY = -7103,
  RINZU_IO_ARGUMENT = -7104,
  RINZU_IO_SATURATED = -7105,
  RINZU_IO_REGULAR = -7106,
  RINZU_IO_PROGRESS = -7107
};
void *rinzu_io_open(const char *path, int32_t writing);
/* mkstemp template; uninstalled temporary files are removed by close/Owner drain. */
void *rinzu_io_temporary(const char *template_path);
int32_t rinzu_io_install_temporary(void *file, const char *destination);
/* 0: stdin, 1: stdout. Owned CLOEXEC duplicate, regular file or exclusive pipe. */
void *rinzu_io_standard(int32_t stream);
bool rinzu_io_owner_has_file(void);
void *rinzu_io_owner_file(void);
bool rinzu_io_pending(void *file);
int32_t rinzu_io_ready_status(void *file);
int32_t rinzu_io_read(void *file, int64_t maximum);
int32_t rinzu_io_write(void *file, const uint8_t *data, size_t length);
int32_t rinzu_io_write_all(void *file, const uint8_t *data, size_t length);
int32_t rinzu_io_flush(void *file);
int32_t rinzu_io_close(void *file);
void *rinzu_io_path(const char *from, const char *to, int32_t removing);
void *rinzu_io_completion(void *file);
bool rinzu_io_ready(void *file);
int32_t rinzu_io_status(void *file);
int64_t rinzu_io_length(void *file);
const uint8_t *rinzu_io_bytes(void *file);
void rinzu_io_cancel(void *file);
void rinzu_io_finish(void *file);
int64_t rinzu_io_monotonic(void);
int64_t rinzu_io_unix(void);
int64_t rinzu_io_live_files(void);
int64_t rinzu_io_live_buffers(void);
int64_t rinzu_io_live_operations(void);
int64_t rinzu_io_retained_controls(void);
#endif

#ifndef RINZU_PROCESS_H
#define RINZU_PROCESS_H
#include <stdint.h>
#include <stddef.h>
/* Read-only startup configuration. No mutation after managed/worker startup.
   Snapshot is thread-local and copied by the native result ABI immediately. */
int64_t rinzu_process_environment_read(const char *name);
const uint8_t *rinzu_process_environment_bytes(void);
int32_t rinzu_process_error_line(const uint8_t *bytes, size_t length);
int64_t rinzu_process_identifier(void);
#endif

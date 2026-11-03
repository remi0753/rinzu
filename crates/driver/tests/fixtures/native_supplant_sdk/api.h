#ifndef RINZU_SUPPLANT_SDK_H
#define RINZU_SUPPLANT_SDK_H
#include <stdint.h>
void supplant_reset(int32_t);
void supplant_prefix(int32_t);
void supplant_busy(void);
void supplant_mark(int32_t, int32_t);
void *supplant_completion(int32_t, int32_t);
int32_t supplant_finish(void);
int32_t supplant_worker_barrier(int32_t);
const char *supplant_run(int32_t (*)(int32_t), int32_t);
const char *supplant_run_alias(int32_t (*)(int32_t), int32_t);
void supplant_check_reset(int32_t);
void supplant_check_inner(void);
void supplant_check_outer(int32_t);
void supplant_check_child(void);
int32_t supplant_worker_block(const char *, const uint8_t *);
void supplant_wait_worker_entry(void);
void supplant_worker_finally(const char *, const uint8_t *);
const char *supplant_check_run(int32_t (*)(int32_t), const char *, const uint8_t *, int32_t);
#endif

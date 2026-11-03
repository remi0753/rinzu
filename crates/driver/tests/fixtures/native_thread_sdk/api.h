#ifndef THREAD_WITNESS_H
#define THREAD_WITNESS_H
#include <stdint.h>
struct Witness;
struct Witness *thread_create(int32_t);
int32_t thread_ping(struct Witness *);
int32_t thread_invoke(struct Witness *, int32_t (*)(struct Witness *, int32_t), int32_t);
void thread_destroy(struct Witness *);
int32_t thread_live(void);
int32_t thread_clear(void);
int32_t thread_block(struct Witness *, int32_t (*)(struct Witness *, int32_t));
void thread_wait_blocked(void);
int32_t thread_is_blocked(void);
void thread_release_blocked(void);
#endif

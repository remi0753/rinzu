#ifndef CAPSULE_WITNESS_H
#define CAPSULE_WITNESS_H
#include <stdint.h>
int32_t capsule_invoke(int32_t (*)(int32_t), int32_t);
void capsule_mark(int32_t, int32_t);
int32_t capsule_finish(int32_t);
void capsule_cancel_reset(void);
int32_t capsule_cancel_invoke(int32_t (*)(int32_t));
void capsule_cancel_inner(void);
void capsule_cancel_outer(void);
#endif

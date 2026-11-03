#ifndef RINZU_SHUTDOWN_SDK_H
#define RINZU_SHUTDOWN_SDK_H
#include <stdint.h>
int32_t rinzu_shutdown_enable(void);
int32_t rinzu_shutdown_disable(void);
int32_t rinzu_shutdown_signal(void);
int64_t rinzu_shutdown_clock(void);
#endif

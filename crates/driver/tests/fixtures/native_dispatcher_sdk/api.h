#include <stdint.h>
void dispatcher_reset(int32_t);
int32_t dispatcher_run(int32_t (*)(int32_t));
void dispatcher_enter_callback(void);
void dispatcher_nested(void);
void dispatcher_leave_callback(void);
void dispatcher_done(int32_t);
int32_t dispatcher_finish(void);
int32_t dispatcher_delay(int32_t);
int32_t dispatcher_worker_delay(int32_t);

#include <stdint.h>
void pump_reset(void);
int32_t pump_block(const char *, const uint8_t *);
int32_t pump_delay(void);
void pump_wait_entries(void);
void pump_stop(void);
void pump_origin_finally(void);
void pump_worker_finally(const char *, const uint8_t *);

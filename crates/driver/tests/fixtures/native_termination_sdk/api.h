#include <stdint.h>
int32_t termination_scalar(int32_t (*callback)(int32_t), int32_t mode);
int32_t termination_unit(void (*callback)(void));
int32_t termination_text(int32_t (*callback)(const char *), int32_t mode);
int32_t termination_bytes(int32_t (*callback)(const uint8_t *));
void termination_mark(int32_t marker);
void termination_abort(void);
int32_t termination_mode(void);
int32_t termination_forever(const char *input, const uint8_t *bytes);
int32_t termination_run_forever(void (*callback)(void), const char *input, const uint8_t *bytes);

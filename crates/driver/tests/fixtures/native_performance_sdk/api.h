#include <stdint.h>
#include <stddef.h>
uint64_t perf_clock(void);
int64_t perf_ping(int64_t value);
void perf_same(int64_t (*callback)(int64_t), size_t count, int32_t kind);
void perf_report(int32_t kind, uint64_t elapsed, size_t count);
int32_t perf_foreign(uint64_t (*callback)(size_t, uint64_t), void (*ready)(void));
void perf_stop(void);
uint64_t perf_worker_stamp(void);

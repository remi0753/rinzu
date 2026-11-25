#ifndef RINZU_COHORT_API_H
#define RINZU_COHORT_API_H
#include <stdint.h>
#include <stddef.h>
typedef struct cohort_completion CohortCompletion;
CohortCompletion *cohort_start(int64_t, size_t (*)(void));
int cohort_publish(int64_t);
int cohort_retire(int64_t);
int cohort_alias(int64_t);
int cohort_status(void);
int cohort_finish(void);
int cohort_count(void);
int cohort_controlled(void);
int cohort_early(void);
int cohort_polling(void);
CohortCompletion *cohort_poll_start(size_t (*)(void));
int cohort_poll_publish(void);
int cohort_poll_retire(void);
int cohort_recovery_step(int64_t);
int cohort_batch_first(void);
int cohort_staggered(void);
int cohort_capture(int64_t);
int cohort_notify_batch(void);
int cohort_wave_first(int64_t);
int cohort_wave_notify(int64_t);
int cohort_late_arrival(void);
#endif

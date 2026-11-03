#ifndef SAMPLE_LOOP_H
#define SAMPLE_LOOP_H
#include <stdint.h>
struct Event {
  int32_t kind;
  int64_t sequence;
};
void *sample_loop_create(_Bool);
_Bool sample_loop_running(void *);
int32_t sample_loop_stop(void *);
int32_t sample_loop_run(void *, void (*)(void *, const struct Event *));
#endif

#include "api.h"
#include <CoreFoundation/CoreFoundation.h>
#include <pthread.h>
static struct {
  int running;
} loop;
void *sample_loop_create(_Bool valid) {
  return valid && pthread_main_np() && !loop.running ? &loop : NULL;
}
_Bool sample_loop_running(void *handle) {
  return pthread_main_np() && handle == &loop && loop.running;
}
int32_t sample_loop_stop(void *handle) {
  if (!pthread_main_np() || handle != &loop)
    return 1;
  loop.running = 0;
  CFRunLoopStop(CFRunLoopGetMain());
  return 0;
}
int32_t sample_loop_run(void *handle, void (*handler)(void *, const struct Event *)) {
  if (!pthread_main_np() || handle != &loop || !handler || loop.running)
    return 1;
  loop.running = 1;
  const struct Event event = {1, 1};
  handler(handle, &event);
  while (loop.running)
    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, false);
  return 0;
}

#include "api.h"
#include <CoreFoundation/CoreFoundation.h>
#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <errno.h>
#include <time.h>
static pthread_t owner, thread;
static CFRunLoopRef loop;
static CFRunLoopSourceRef receipt;
static int32_t (*event)(int32_t);
static _Atomic int received;
static int active, managed, completed, inner_ticks;
static void require_owner(void) {
  assert(pthread_equal(owner, pthread_self()));
}
static void stop_if_complete(void) {
  require_owner();
  if (completed && atomic_load(&received))
    CFRunLoopStop(loop);
}
static void signal_receipt(void *raw) {
  (void)raw;
  stop_if_complete();
}
static void *invoke(void *raw) {
  (void)raw;
  assert(!pthread_equal(owner, pthread_self()));
  assert(event(7) == 41);
  atomic_store(&received, 1);
  CFRunLoopSourceSignal(receipt);
  CFRunLoopWakeUp(loop);
  return NULL;
}
static void start(CFRunLoopTimerRef timer, void *raw) {
  (void)raw;
  require_owner();
  CFRunLoopTimerInvalidate(timer);
  assert(!pthread_create(&thread, NULL, invoke, NULL));
}
void dispatcher_reset(int32_t process_main) {
  owner = pthread_self();
  if (process_main)
    assert(pthread_main_np());
}
int32_t dispatcher_run(int32_t (*callback)(int32_t)) {
  require_owner();
  assert(!active && !managed && !completed);
  active = 1;
  event = callback;
  loop = CFRunLoopGetCurrent();
  CFRunLoopSourceContext receipt_context = {.perform = signal_receipt};
  receipt = CFRunLoopSourceCreate(NULL, 0, &receipt_context);
  assert(receipt);
  CFRunLoopAddSource(loop, receipt, kCFRunLoopDefaultMode);
  CFRunLoopTimerContext timer_context = {0};
  CFRunLoopTimerRef timer = CFRunLoopTimerCreate(NULL, CFAbsoluteTimeGetCurrent() + 0.001, 0, 0, 0,
                                                 start, &timer_context);
  assert(timer);
  CFRunLoopAddTimer(loop, timer, kCFRunLoopDefaultMode);
  /* No repeating timer drives Main work. Only runtime completion wake can
     resume the parked Rinzu flow and its file/Worker dependencies. */
  (void)CFRunLoopRunInMode(kCFRunLoopDefaultMode, 10, 0);
  assert(completed == 63 && atomic_load(&received) && !managed && inner_ticks == 1);
  assert(!pthread_join(thread, NULL));
  CFRunLoopSourceInvalidate(receipt);
  CFRelease(receipt);
  CFRunLoopTimerInvalidate(timer);
  CFRelease(timer);
  active = 0;
  fputs("DISPATCHER_NATIVE_RETURN\n", stderr);
  return 23;
}
void dispatcher_enter_callback(void) {
  require_owner();
  assert(active && !managed && !completed);
  managed = 1;
}
static void inner_tick(CFRunLoopTimerRef timer, void *raw) {
  (void)raw;
  require_owner();
  assert(managed && !completed);
  inner_ticks++;
  CFRunLoopTimerInvalidate(timer);
  CFRunLoopStop(loop);
}
void dispatcher_nested(void) {
  require_owner();
  assert(managed && !completed);
  CFRunLoopTimerContext context = {0};
  CFRunLoopTimerRef timer =
      CFRunLoopTimerCreate(NULL, CFAbsoluteTimeGetCurrent() + 0.001, 0, 0, 0, inner_tick, &context);
  assert(timer);
  CFRunLoopAddTimer(loop, timer, kCFRunLoopDefaultMode);
  (void)CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1, 0);
  CFRunLoopTimerInvalidate(timer);
  CFRelease(timer);
  assert(managed && !completed && inner_ticks == 1);
}
void dispatcher_leave_callback(void) {
  require_owner();
  assert(managed && !completed);
  managed = 0;
}
void dispatcher_done(int32_t value) {
  require_owner();
  assert(active && !managed && !completed && value == 63);
  completed = value;
  fputs("DISPATCHER_FLOW_DONE_BEFORE_RETURN\n", stderr);
  stop_if_complete();
}
int32_t dispatcher_finish(void) {
  require_owner();
  assert(!active && !managed && completed == 63);
  return completed;
}

static int32_t delay(int32_t milliseconds) {
  assert(milliseconds > 0 && milliseconds < 3000);
  struct timespec duration = {.tv_sec = milliseconds / 1000,
                              .tv_nsec = (long)(milliseconds % 1000) * 1000000L};
  while (nanosleep(&duration, &duration)) {
    assert(errno == EINTR);
  }
  return milliseconds;
}
int32_t dispatcher_delay(int32_t milliseconds) {
  require_owner();
  assert(active);
  return delay(milliseconds);
}
int32_t dispatcher_worker_delay(int32_t milliseconds) {
  assert(!pthread_equal(owner, pthread_self()));
  return delay(milliseconds);
}

#include "../native/src/macos.m"
#include <assert.h>
#include <stdio.h>
#include <objc/runtime.h>

// Only the visibility inputs are simulated; timers, completion and RunLoop are real.
static BOOL testOccluded, testMinimized;
@interface RZFrameStateWindow : RZMacNativeWindow
@end
@implementation RZFrameStateWindow
- (NSWindowOcclusionState)occlusionState {
  return testOccluded ? 0 : NSWindowOcclusionStateVisible;
}
- (BOOL)isMiniaturized {
  return testMinimized;
}
@end

static void *session, *windows[2], *roots[2], *clocks[2];
static int64_t tokens[2], hiddenTicks;
static int phase, animatedFrames;
static double entered;
static void begin(unsigned index) {
  tokens[index] = rinzu_macos_wait_token();
  assert(rinzu_macos_frame_wait_begin(clocks[index], tokens[index]) == 0);
}
static BOOL ready(unsigned index) {
  return rinzu_native_completion_is_done(
      rinzu_macos_frame_completion(clocks[index], tokens[index]));
}
static void finish(unsigned index, int expected) {
  struct MacFrameInfo info;
  assert(rinzu_macos_frame_take(clocks[index], tokens[index], &info) == expected);
  if (expected)
    assert(info.width == (int64_t)(160 * info.scale) &&
           info.height == (int64_t)(100 * info.scale) && info.delta >= 0);
  assert(rinzu_macos_frame_finish(clocks[index], tokens[index]) == 0);
}
static void receive(void *raw, const struct MacEvent *event) {
  assert(raw == session);
  if (event->kind == RZ_STARTED) {
    begin(0);
    begin(1);
    phase = 1;
  }
}
int main(void) {
  @autoreleasepool {
    if (!rinzu_macos_metal_available())
      return 77;
    session = rinzu_macos_session(rinzu_macos_open());
    void *device = rinzu_macos_device(rinzu_macos_device_create(session));
    for (unsigned i = 0; i < 2; ++i) {
      windows[i] = rinzu_macos_window(
          rinzu_macos_window_create(session, (const uint8_t *)"Frames", 6, 160, 100));
      RZMacWindow *window = (RZMacWindow *)lookup(windows[i], RZ_WINDOW);
      object_setClass(window.window, [RZFrameStateWindow class]);
      roots[i] = rinzu_macos_window_content(windows[i]);
      struct MacRect rect = {0, 0, 160, 100};
      void *view = rinzu_macos_view(rinzu_macos_surface_scoped(device, windows[i], &rect));
      assert(rinzu_macos_view_add(roots[i], view) == 0);
      clocks[i] = rinzu_macos_frame(rinzu_macos_frame_create(view, 120));
      assert(rinzu_macos_window_show(windows[i]) == 0);
    }
    assert(rinzu_macos_frame_animating(clocks[1], true) == 0);
    __block unsigned guard = 0;
    NSTimer *driver = [NSTimer
        scheduledTimerWithTimeInterval:0.002
                               repeats:YES
                                 block:^(NSTimer *timer) {
                                   (void)timer;
                                   assert(++guard < 2000);
                                   double now = CACurrentMediaTime();
                                   RZMacFrameClock *first =
                                       (RZMacFrameClock *)lookup(clocks[0], RZ_FRAME_CLOCK);
                                   RZMacFrameClock *second =
                                       (RZMacFrameClock *)lookup(clocks[1], RZ_FRAME_CLOCK);
                                   if (phase == 1 && ready(0) && ready(1)) {
                                     finish(0, 1);
                                     finish(1, 1);
                                     assert(rinzu_macos_view_hidden(roots[0], true) == 0);
                                     hiddenTicks = first.ticks;
                                     begin(0);
                                     begin(1);
                                     phase = 2;
                                   } else if (phase == 2 && ready(1)) {
                                     finish(1, 1);
                                     assert(!ready(0) && !first.timer &&
                                            first.ticks == hiddenTicks);
                                     begin(1);
                                     if (++animatedFrames == 5) {
                                       testOccluded = YES;
                                       refresh_frame(second, NO);
                                       assert(!second.timer);
                                       entered = now;
                                       phase = 3;
                                     }
                                   } else if (phase == 3 && now - entered > 0.05) {
                                     assert(!ready(1) && !second.timer);
                                     testOccluded = NO;
                                     testMinimized = YES;
                                     refresh_frame(second, NO);
                                     assert(!second.timer);
                                     entered = now;
                                     phase = 4;
                                   } else if (phase == 4 && now - entered > 0.05) {
                                     assert(!ready(1) && !second.timer);
                                     testMinimized = NO;
                                     wake_application();
                                     phase = 5;
                                   } else if (phase == 5 && ready(1)) {
                                     finish(1, 1);
                                     assert(rinzu_macos_frame_stop_window(windows[0]) == 0);
                                     assert(ready(0) && !second.stopped);
                                     finish(0, 0);
                                     assert(rinzu_macos_frame_destroy(clocks[0]) == 0);
                                     begin(1);
                                     phase = 6;
                                   } else if (phase == 6 && ready(1)) {
                                     finish(1, 1);
                                     begin(1);
                                     assert(rinzu_macos_stop(session) == 0);
                                     phase = 7;
                                   }
                                 }];
    assert(rinzu_macos_run(session, session, receive) == 0);
    [driver invalidate];
    assert(phase == 7 && ready(1));
    assert(rinzu_macos_close(session) == RZ_BUSY);
    finish(1, 0);
    assert(rinzu_macos_frame_destroy(clocks[1]) == 0);
    assert(rinzu_macos_close(session) == 0);
    struct MacStatistics stats;
    assert(rinzu_macos_statistics(&stats) == 0 && !stats.retainedControls && !stats.frameTimers &&
           !stats.frameWaiters);
    puts("PASS:frames:multiwindow:animation:occlusion:minimized:scoped-stop:closed");
  }
  return 0;
}

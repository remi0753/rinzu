#include "../native/src/macos.m"
#include <assert.h>
#include <stdio.h>
#include <objc/runtime.h>
static BOOL fixture_visible;
@interface RZFrameFixtureWindow : RZMacNativeWindow
@end
@implementation RZFrameFixtureWindow
- (NSWindowOcclusionState)occlusionState {
  return fixture_visible ? NSWindowOcclusionStateVisible : [super occlusionState];
}
@end

static void *session, *surface, *root, *frame_clock;
static int64_t token;
static int phase;
static double entered;
static int64_t idleTicks;
static BOOL skipped_visible;
static void begin(void) {
  token = rinzu_macos_wait_token();
  assert(rinzu_macos_frame_wait_begin(frame_clock, token) == 0);
  assert(rinzu_macos_frame_wait_begin(frame_clock, rinzu_macos_wait_token()) == RZ_BUSY);
}
static BOOL ready(void) {
  return rinzu_native_completion_is_done(rinzu_macos_frame_completion(frame_clock, token));
}
static void finish(int expected) {
  struct MacFrameInfo info;
  assert(rinzu_macos_frame_take(frame_clock, token, &info) == expected);
  if (expected)
    assert(info.width > 0 && info.height > 0 && info.sequence > 0 && info.scale >= 1);
  assert(rinzu_macos_frame_finish(frame_clock, token) == 0);
  assert(!rinzu_macos_frame_wait_owned(frame_clock, token));
}
static void receive(void *raw, const struct MacEvent *event) {
  assert(raw == session);
  if (event->kind == RZ_STARTED) {
    assert(phase == 0);
    begin();
    phase = 1;
  }
}
int main(int argc, const char **argv) {
  @autoreleasepool {
    fixture_visible = argc == 2 && !strcmp(argv[1], "fixture");
    session = rinzu_macos_session(rinzu_macos_open());
    if (!rinzu_macos_metal_available())
      return 77;
    void *device = rinzu_macos_device(rinzu_macos_device_create(session));
    void *window = rinzu_macos_window(
        rinzu_macos_window_create(session, (const uint8_t *)"Frame frame_clock", 11, 320, 180));
    if (fixture_visible)
      object_setClass(((RZMacWindow *)lookup(window, RZ_WINDOW)).window,
                      [RZFrameFixtureWindow class]);
    root = rinzu_macos_window_content(window);
    struct MacRect rect = {0, 0, 320, 180};
    surface = rinzu_macos_view(rinzu_macos_surface_scoped(device, window, &rect));
    assert(rinzu_macos_view_add(root, surface) == 0);
    assert(rinzu_macos_frame_create(surface, 0) == RZ_ARGUMENT);
    frame_clock = rinzu_macos_frame(rinzu_macos_frame_create(surface, 120));
    assert(frame_clock && rinzu_macos_frame_create(surface, 60) == RZ_BUSY);
    assert(rinzu_macos_frame_state(frame_clock) == 0);
    assert(rinzu_macos_frame_wait_begin(frame_clock, rinzu_macos_wait_token()) == RZ_BUSY);
    assert(rinzu_macos_window_show(window) == 0);
    __block unsigned guard = 0;
    NSTimer *driver = [NSTimer
        scheduledTimerWithTimeInterval:0.002
                               repeats:YES
                                 block:^(NSTimer *tick) {
                                   (void)tick;
                                   ++guard;
                                   if (!fixture_visible && guard == 125 && phase == 1 && !ready()) {
                                     skipped_visible = YES;
                                     assert(rinzu_macos_stop(session) == 0);
                                     return;
                                   }
                                   assert(guard < 4000);
                                   double now = CACurrentMediaTime();
                                   if (phase == 1 && ready()) {
                                     finish(1);
                                     begin();
                                     for (unsigned i = 0; i < 1000; ++i)
                                       assert(rinzu_macos_frame_request(frame_clock) == 0);
                                     struct MacFrameStatistics stats;
                                     assert(rinzu_macos_frame_statistics(frame_clock, &stats) ==
                                                0 &&
                                            stats.coalesced >= 999);
                                     phase = 2;
                                   } else if (phase == 2 && ready()) {
                                     finish(1);
                                     idleTicks =
                                         ((RZMacFrameClock *)lookup(frame_clock, RZ_FRAME_CLOCK))
                                             .ticks;
                                     begin();
                                     entered = now;
                                     phase = 3;
                                   } else if (phase == 3 && ready()) {
                                     // AppKit can deliver its initial exposure after the second
                                     // frame, particularly under TSan. Begin the idle interval
                                     // after the last legitimate invalidation has been consumed.
                                     finish(1);
                                     idleTicks =
                                         ((RZMacFrameClock *)lookup(frame_clock, RZ_FRAME_CLOCK))
                                             .ticks;
                                     begin();
                                     entered = now;
                                   } else if (phase == 3 && now - entered > 0.05) {
                                     RZMacFrameClock *state =
                                         (RZMacFrameClock *)lookup(frame_clock, RZ_FRAME_CLOCK);
                                     assert(!ready() && !state.timer && state.ticks == idleTicks);
                                     assert(rinzu_macos_view_hidden(root, true) == 0);
                                     assert(rinzu_macos_frame_request(frame_clock) == 0);
                                     entered = now;
                                     phase = 4;
                                   } else if (phase == 4 && now - entered > 0.05) {
                                     RZMacFrameClock *state =
                                         (RZMacFrameClock *)lookup(frame_clock, RZ_FRAME_CLOCK);
                                     assert(!ready() && !state.timer && state.ticks == idleTicks);
                                     assert(rinzu_macos_view_hidden(root, false) == 0);
                                     phase = 5;
                                   } else if (phase == 5 && ready()) {
                                     finish(1);
                                     begin();
                                     rinzu_native_completion_t *completion =
                                         rinzu_macos_frame_completion(frame_clock, token);
                                     atomic_store_explicit(&completion->cancelled, 1,
                                                           memory_order_release);
                                     wake_application();
                                     phase = 6;
                                   } else if (phase == 6 && ready()) {
                                     finish(0);
                                     assert(rinzu_macos_frame_request(frame_clock) == 0);
                                     begin();
                                     phase = 7;
                                   } else if (phase == 7 && ready()) {
                                     finish(1);
                                     for (unsigned i = 0; i < 8; ++i)
                                       assert(rinzu_macos_frame_retry(frame_clock) == 0);
                                     assert(((RZMacFrameClock *)lookup(frame_clock, RZ_FRAME_CLOCK))
                                                .retryDelay == 0.25);
                                     begin();
                                     entered = now;
                                     phase = 8;
                                   } else if (phase == 8 && ready()) {
                                     assert(now - entered >= 0.20);
                                     finish(1);
                                     assert(rinzu_macos_frame_presented(frame_clock) == 0);
                                     assert(((RZMacFrameClock *)lookup(frame_clock, RZ_FRAME_CLOCK))
                                                .retryDelay == 0);
                                     begin();
                                     assert(rinzu_macos_view_destroy(surface) == 0);
                                     assert(ready());
                                     assert(rinzu_macos_frame_destroy(frame_clock) == RZ_BUSY);
                                     finish(0);
                                     assert(rinzu_macos_frame_destroy(frame_clock) == 0);
                                     assert(rinzu_macos_frame_request(frame_clock) == RZ_INVALID);
                                     phase = 9;
                                     assert(rinzu_macos_stop(session) == 0);
                                   }
                                 }];
    assert(rinzu_macos_run(session, session, receive) == 0);
    [driver invalidate];
    if (skipped_visible) {
      finish(0);
      assert(rinzu_macos_frame_destroy(frame_clock) == 0);
      assert(rinzu_macos_close(session) == 0);
      puts("SKIP:frames:real-window-occluded");
      return 77;
    }
    assert(phase == 9);
    assert(rinzu_macos_close(session) == 0);
    struct MacStatistics stats;
    assert(rinzu_macos_statistics(&stats) == 0 && !stats.retainedControls && !stats.frameTimers &&
           !stats.frameWaiters);
    puts(fixture_visible ? "PASS:frames:fixture-occlusion" : "PASS:frames:real-occlusion");
    puts("PASS:frames:1000-coalesced:idle-zero:hidden-zero:cancel-idle:retry:view-destroy:closed");
  }
  return 0;
}

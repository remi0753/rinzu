// Real AppKit sheets and Window-owned completion receipts; no managed callback.
#include "../native/src/macos.m"
#include <assert.h>
#include <stdio.h>

static int32_t alert(void *window, int64_t token) {
  return rinzu_macos_alert_begin(window, token, (const uint8_t *)"変更を保存", 15,
                                 (const uint8_t *)"日本 e\xcc\x81 \xf0\x9f\x98\x80", 15,
                                 (const uint8_t *)"Save", 4, (const uint8_t *)"Discard", 7,
                                 (const uint8_t *)"Cancel", 6);
}
static void clean(void) {
  struct MacStatistics stats;
  assert(rinzu_macos_statistics(&stats) == 0);
  assert(stats.sessions + stats.windows + stats.views + stats.retainedControls +
             stats.queuedEvents + stats.eventPayloadBytes + stats.snapshotBytes + stats.sheets +
             stats.sheetBytes ==
         0);
}
struct IdleWake {
  rinzu_native_completion_wake_t target;
  void *session;
  BOOL notified;
};
static void idle_notify(rinzu_native_completion_wake_t *raw) {
  struct IdleWake *wake = (struct IdleWake *)raw;
  wake->notified = YES;
  assert(rinzu_macos_stop(wake->session) == 0);
}
static void idle_release(rinzu_native_completion_wake_t *raw) {
  (void)raw; // stack target stays alive until run physically returns
}
static void idle_cancel(void) {
  void *session = rinzu_macos_session(rinzu_macos_open());
  void *window = rinzu_macos_window(
      rinzu_macos_window_create(session, (const uint8_t *)"Idle cancel", 11, 400, 280));
  assert(rinzu_macos_window_show(window) == 0);
  assert(rinzu_macos_prepare_events(session) == 0);
  int64_t token = rinzu_macos_wait_token();
  struct IdleWake wake = {.target = {.notify = idle_notify, .release = idle_release},
                          .session = session};
  struct IdleWake *target = &wake;
  __block BOOL requested = NO;
  // Model cancellation in a Source continuation at the last observer, with no
  // periodic timer or later keyboard/mouse input to finish AppKit's dismissal.
  CFRunLoopObserverRef observer = CFRunLoopObserverCreateWithHandler(
      NULL, kCFRunLoopBeforeWaiting, true, LONG_MAX,
      ^(CFRunLoopObserverRef observing, CFRunLoopActivity activity) {
        (void)activity;
        RZMacWindow *parent = (RZMacWindow *)lookup(window, RZ_WINDOW);
        if (!parent.sheet.window.visible || requested)
          return;
        requested = YES;
        (void)observing;
        atomic_store_explicit(&parent.sheet->completion.notification, (uintptr_t)&target->target,
                              memory_order_release);
        int32_t status = rinzu_macos_sheet_cancel(window, token);
        if (status) {
          const rinzu_native_byte_array_t *message = rinzu_macos_diagnostic();
          fprintf(stderr, "idle cancel: %d: %.*s\n", status, (int)message->length, message->data);
        }
        assert(status == 0);
      });
  CFRunLoopAddObserver(CFRunLoopGetCurrent(), observer, kCFRunLoopCommonModes);
  NSTimer *starter = [NSTimer
      scheduledTimerWithTimeInterval:0.01
                             repeats:NO
                               block:^(NSTimer *timer) {
                                 (void)timer;
                                 assert(rinzu_macos_panel_begin(
                                            window, token, 0, (const uint8_t *)"Idle cancel", 11,
                                            (const uint8_t *)"", 0, (const uint8_t *)"", 0) == 0);
                               }];
  assert(rinzu_macos_run_events(session) == 0);
  [starter invalidate];
  CFRunLoopObserverInvalidate(observer);
  CFRelease(observer);
  assert(requested && wake.notified);
  assert(rinzu_macos_sheet_result(window, token) == 0);
  assert(rinzu_macos_sheet_finish(window, token) == 0);
  assert(rinzu_macos_close(session) == 0);
  clean();
  puts("PASS:sheets:cancel-idle:no-further-input:physical-completion:cleanup");
}
static void exercise(const char *mode) {
  NSString *directory = [NSTemporaryDirectory()
      stringByAppendingPathComponent:[@"rinzu-sheet-"
                                         stringByAppendingString:NSUUID.UUID.UUIDString]];
  NSString *filename = @"selected 日本😀.txt";
  NSString *path = [directory stringByAppendingPathComponent:filename];
  BOOL open = strncmp(mode, "open", 4) == 0, save = strncmp(mode, "save", 4) == 0;
  BOOL accept = strstr(mode, "accept") != NULL;
  BOOL no_selection = strstr(mode, "no-selection") != NULL;
  NSError *error = nil;
  assert([[NSFileManager defaultManager] createDirectoryAtPath:directory
                                   withIntermediateDirectories:YES
                                                    attributes:nil
                                                         error:&error]);
  if (open)
    assert([[@"file 日本😀" dataUsingEncoding:NSUTF8StringEncoding] writeToFile:path
                                                                     atomically:YES]);
  void *session = rinzu_macos_session(rinzu_macos_open());
  int64_t scope = rinzu_macos_wait_token(), other_scope = rinzu_macos_wait_token();
  void *window = rinzu_macos_window(
      rinzu_macos_window_scoped(session, scope, (const uint8_t *)"Sheets", 6, 500, 320));
  void *other = rinzu_macos_window(
      rinzu_macos_window_scoped(session, other_scope, (const uint8_t *)"Other", 5, 320, 200));
  assert(window && other);
  int32_t shown = rinzu_macos_window_show(window);
  if (shown) {
    const rinzu_native_byte_array_t *message = rinzu_macos_diagnostic();
    fprintf(stderr, "show: %d: %.*s\n", shown, (int)message->length, message->data);
  }
  assert(shown == 0);
  assert(rinzu_macos_prepare_events(session) == 0);
  int64_t token = rinzu_macos_wait_token(), second = rinzu_macos_wait_token();
  assert(alert(window, token) == RZ_BUSY); // no OS loop yet
  __block int stage = 0;
  __block unsigned ticks = 0;
  __block BOOL stopped = NO;
  __weak RZMacSheet *released;
  NSTimer *timer = [NSTimer
      timerWithTimeInterval:0.01
                    repeats:YES
                      block:^(NSTimer *tick) {
                        @try {
                          assert(++ticks < 2000);
                          RZMacWindow *parent = (RZMacWindow *)lookup(window, RZ_WINDOW);
                          if (stage == 0) {
                            if (strcmp(mode, "pre-attach") == 0) {
                              NSAlert *native = [NSAlert new];
                              [native addButtonWithTitle:@"OK"];
                              RZMacSheet *sheet = new_sheet(parent, token, native.window, native);
                              assert(rinzu_macos_sheet_cancel(window, token) == 0);
                              assert(sheet.cancelRequested && !sheet.dismissIssued);
                              [native beginSheetModalForWindow:parent.window
                                             completionHandler:^(NSModalResponse response) {
                                               (void)response;
                                               finish_sheet(sheet, 0, nil);
                                             }];
                            } else if (open || save) {
                              NSData *folder = [directory dataUsingEncoding:NSUTF8StringEncoding];
                              NSData *name = [filename dataUsingEncoding:NSUTF8StringEncoding];
                              assert(rinzu_macos_panel_begin(window, token, save,
                                                             (const uint8_t *)"Choose file", 11,
                                                             folder.bytes, folder.length,
                                                             name.bytes, name.length) == 0);
                            } else {
                              assert(alert(window, token) == 0);
                            }
                            assert(rinzu_macos_sheet_owned(window, token));
                            assert(!rinzu_macos_sheet_owned(window, second));
                            assert(alert(window, second) == RZ_BUSY);
                            assert(rinzu_macos_sheet_finish(window, second) == RZ_BUSY);
                            assert(rinzu_macos_sheet_finish(window, token) == RZ_BUSY);
                            assert(rinzu_macos_sheet_result(window, token) == RZ_BUSY);
                            assert(rinzu_macos_sheet_bytes(window, token) == NULL);
                            if (strcmp(mode, "two") == 0)
                              assert(alert(other, second) == 0);
                            struct MacStatistics stats;
                            assert(rinzu_macos_statistics(&stats) == 0);
                            assert(stats.sheets == (strcmp(mode, "two") == 0 ? 2 : 1));
                            stage = 1;
                            return;
                          }
                          if (stage == 1) {
                            if (strcmp(mode, "pre-attach") == 0 && parent.sheet.finished) {
                              stage = 2;
                              return;
                            }
                            if (!parent.sheet.window.visible)
                              return;
                            if (strcmp(mode, "alert") == 0 || strcmp(mode, "two") == 0) {
                              NSAlert *native = parent.sheet.payload;
                              [native.buttons[1] performClick:nil];
                            } else if (accept || no_selection) {
                              [parent.window endSheet:parent.sheet.window
                                           returnCode:NSModalResponseOK];
                            } else if (strcmp(mode, "cancel") == 0) {
                              rinzu_native_completion_t *completion =
                                  rinzu_macos_sheet_completion(window, token);
                              atomic_store_explicit(&completion->cancelled, 1,
                                                    memory_order_release);
                            } else if (strcmp(mode, "window") == 0) {
                              assert(rinzu_macos_window_scope_prepare(session, scope) == RZ_BUSY);
                              assert(rinzu_macos_window_destroy(window) == RZ_BUSY);
                            } else if (strcmp(mode, "external-stop") == 0) {
                              [NSApp stop:nil];
                              [NSApp postEvent:[NSEvent
                                                   otherEventWithType:NSEventTypeApplicationDefined
                                                             location:NSZeroPoint
                                                        modifierFlags:0
                                                            timestamp:0
                                                         windowNumber:0
                                                              context:nil
                                                              subtype:0
                                                                data1:0
                                                                data2:0]
                                       atStart:NO];
                              stopped = YES;
                            } else if (strcmp(mode, "stop") == 0) {
                              assert(rinzu_macos_stop(session) == 0);
                              stopped = YES;
                            } else {
                              assert(rinzu_macos_sheet_dismiss(window) == 0);
                            }
                            stage = 2;
                            return;
                          }
                          rinzu_native_completion_t *completion =
                              rinzu_macos_sheet_completion(window, token);
                          if (!rinzu_native_completion_is_done(completion))
                            return;
                          int expected = no_selection ? RZ_ARGUMENT
                                         : accept     ? 1
                                         : strcmp(mode, "alert") == 0 || strcmp(mode, "two") == 0
                                             ? 2
                                             : 0;
                          assert(rinzu_macos_sheet_result(window, token) == expected);
                          const rinzu_native_byte_array_t *bytes =
                              rinzu_macos_sheet_bytes(window, token);
                          if (accept) {
                            NSData *expected_path = [path dataUsingEncoding:NSUTF8StringEncoding];
                            assert(bytes->length == expected_path.length);
                            assert(!memcmp(bytes->data, expected_path.bytes, bytes->length));
                            if (save)
                              assert(![[NSFileManager defaultManager] fileExistsAtPath:path]);
                          } else
                            assert(bytes->length == 0);
                          assert(alert(window, second) ==
                                 RZ_BUSY); // receipt still owns slot after dismissal
                          assert(rinzu_macos_sheet_finish(window, token) == 0);
                          assert(!rinzu_macos_sheet_owned(window, token));
                          assert(rinzu_macos_sheet_completion(window, token) == NULL);
                          if (strcmp(mode, "two") == 0) {
                            assert(rinzu_macos_window_scope_prepare(session, scope) == 0);
                            assert(rinzu_macos_window_scope_close(session, scope) == 0);
                            assert(rinzu_macos_sheet_owned(other, second));
                            assert(rinzu_macos_stop(session) == 0);
                            stopped = YES;
                          }
                          if (!stopped)
                            assert(rinzu_macos_stop(session) == 0);
                          [tick invalidate];
                          stage = 3;
                        } @catch (NSException *exception) {
                          fprintf(stderr, "sheet test exception: %s\n",
                                  exception.description.UTF8String);
                          abort();
                        }
                      }];
  [[NSRunLoop currentRunLoop] addTimer:timer forMode:NSRunLoopCommonModes];
  assert(rinzu_macos_run_events(session) == 0);
  [timer invalidate];
  // stop may return before our timer observes/acknowledges the ready receipt.
  if (rinzu_macos_sheet_owned(window, token)) {
    assert(rinzu_native_completion_is_done(rinzu_macos_sheet_completion(window, token)));
    assert(rinzu_macos_sheet_result(window, token) == 0);
    assert(rinzu_macos_prepare_close(session) == RZ_BUSY);
    released = ((RZMacWindow *)lookup(window, RZ_WINDOW)).sheet;
    assert(rinzu_macos_sheet_finish(window, token) == 0);
    stage = 3;
  }
  if (rinzu_macos_sheet_owned(other, second)) {
    assert(rinzu_native_completion_is_done(rinzu_macos_sheet_completion(other, second)));
    assert(rinzu_macos_sheet_result(other, second) == 0);
    assert(rinzu_macos_sheet_finish(other, second) == 0);
  }
  assert(stage == 3 && !released);
  assert(rinzu_macos_prepare_close(session) == 0);
  assert(rinzu_macos_close(session) == 0);
  clean();
  assert([[NSFileManager defaultManager] removeItemAtPath:directory error:&error]);
  printf("PASS:sheets:%s:physical-dismissal:owned-receipt:cleanup\n", mode);
}
int main(int argc, const char **argv) {
  assert(argc == 2);
  @autoreleasepool {
    if (strcmp(argv[1], "cancel-idle") == 0)
      idle_cancel();
    else
      exercise(argv[1]);
  }
}

// Appended only to a frozen test SDK. Exercise AppKit's real sheet callbacks.
#include <assert.h>
static NSMutableDictionary<NSNumber *, NSMutableArray<NSDictionary *> *> *test_replies;
int32_t rinzu_macos_test_sheet_reply(void *raw, int32_t choice, const uint8_t *bytes,
                                     size_t length) {
  RZMacWindow *parent = (RZMacWindow *)lookup(raw, RZ_WINDOW);
  NSString *path = text(bytes, length);
  if (!parent || !path)
    return RZ_ARGUMENT;
  if (!test_replies)
    test_replies = [NSMutableDictionary new];
  NSNumber *key = @(parent.identity);
  NSMutableArray<NSDictionary *> *queue = test_replies[key];
  BOOL existing = queue != nil;
  if (!queue) {
    queue = [NSMutableArray new];
    test_replies[key] = queue;
  }
  [queue addObject:@{@"choice" : @(choice), @"path" : path}];
  if (existing)
    return 0;
  __block unsigned attempts = 0;
  __block int64_t previous = 0;
  __block RZMacSheet *configured = nil;
  NSTimer *timer = [NSTimer
      timerWithTimeInterval:0.01
                    repeats:YES
                      block:^(NSTimer *tick) {
                        __attribute__((objc_precise_lifetime)) NSTimer *active = tick;
                        @try {
                          if (++attempts > 2000)
                            abort();
                          RZMacSheet *sheet = parent.sheet;
                          if (!sheet || !sheet.window.visible || sheet.token == previous)
                            return;
                          NSDictionary *reply = queue.firstObject;
                          int32_t button = [reply[@"choice"] intValue];
                          NSString *selected = reply[@"path"];
                          if (button && [sheet.payload isKindOfClass:[NSOpenPanel class]]) {
                            NSSavePanel *panel = sheet.payload;
                            if (configured != sheet) {
                              configured = sheet;
                              panel.directoryURL =
                                  [NSURL fileURLWithPath:selected.stringByDeletingLastPathComponent
                                             isDirectory:YES];
                              panel.nameFieldStringValue = selected.lastPathComponent;
                              return;
                            }
                          } else if (button && [sheet.payload isKindOfClass:[NSSavePanel class]]) {
                            NSSavePanel *panel = sheet.payload;
                            if (![panel.URL.path isEqualToString:selected]) {
                              fprintf(stderr, "preset save path mismatch: %s != %s\n",
                                      panel.URL.path.UTF8String, selected.UTF8String);
                              abort();
                            }
                          }
                          previous = sheet.token;
                          attempts = 0;
                          [queue removeObjectAtIndex:0];
                          if (!queue.count) {
                            [active invalidate];
                            [test_replies removeObjectForKey:key];
                          }
                          if (!button) {
                            cancel_sheet(sheet);
                          } else if ([sheet.payload isKindOfClass:[NSAlert class]]) {
                            NSAlert *alert = sheet.payload;
                            assert(button >= 1 && (NSUInteger)button <= alert.buttons.count);
                            [alert.buttons[(NSUInteger)button - 1] performClick:nil];
                          } else {
                            NSSavePanel *panel = sheet.payload;
                            [parent.window endSheet:panel returnCode:NSModalResponseOK];
                          }
                          configured = nil;
                        } @catch (NSException *exception) {
                          fprintf(stderr, "sheet probe exception: %s\n",
                                  exception.description.UTF8String);
                          abort();
                        }
                      }];
  [[NSRunLoop currentRunLoop] addTimer:timer forMode:NSRunLoopCommonModes];
  return 0;
}
int32_t rinzu_macos_test_sheet_visible(void *raw) {
  RZMacWindow *parent = (RZMacWindow *)lookup(raw, RZ_WINDOW);
  return parent.sheet.window.visible ? 1 : 0;
}

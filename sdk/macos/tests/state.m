#include "../native/src/macos.m"
#include <assert.h>
#include <stdio.h>

int main(void) {
  @autoreleasepool {
    void *session = rinzu_macos_session(rinzu_macos_open());
    void *window = rinzu_macos_window(
        rinzu_macos_window_create(session, (const uint8_t *)"State events", 12, 500, 320));
    assert(window);
    void *root = rinzu_macos_window_content(window);
    struct MacRect frame = {10, 10, 100, 80};
    void *first = rinzu_macos_view(
        rinzu_macos_widget_scoped(window, RZ_TEXT_VIEW, (const uint8_t *)"First", 5, &frame, 0));
    void *second = rinzu_macos_view(
        rinzu_macos_widget_scoped(window, RZ_TEXT_FIELD, (const uint8_t *)"Second", 6, &frame, 0));
    void *scroll = rinzu_macos_view(
        rinzu_macos_widget_scoped(window, RZ_SCROLL_VIEW, (const uint8_t *)"", 0, &frame, 0));
    struct MacRect large = {0, 0, 2000, 2000};
    void *child = rinzu_macos_view(rinzu_macos_view_create(session, &large));
    assert(first && second && scroll && child);
    assert(rinzu_macos_view_add(root, first) == 0);
    assert(rinzu_macos_view_add(root, second) == 0);
    assert(rinzu_macos_view_add(root, scroll) == 0);
    assert(rinzu_macos_scroll_content(scroll, child) == 0);
    assert(rinzu_macos_window_show(window) == 0);
    assert(rinzu_macos_prepare_events(session) == 0);
    __block BOOL tested = NO;
    NSTimer *timer = [NSTimer
        scheduledTimerWithTimeInterval:0.01
                               repeats:YES
                                 block:^(NSTimer *tick) {
                                   RZMacSession *owner =
                                       (RZMacSession *)lookup(session, RZ_SESSION);
                                   [owner clearEvents];
                                   assert(rinzu_macos_view_focus(first) == 0);
                                   assert(rinzu_macos_view_focus(second) == 0);
                                   BOOL gained = NO, lost = NO;
                                   for (unsigned index = 0; index < owner->count; ++index) {
                                     struct MacEvent event =
                                         owner->events[(owner->head + index) % RZ_EVENT_LIMIT];
                                     gained |= event.kind == RZ_FOCUS &&
                                               event.viewId == rinzu_macos_view_id(second) &&
                                               event.value == 1;
                                     lost |= event.kind == RZ_FOCUS &&
                                             event.viewId == rinzu_macos_view_id(first) &&
                                             event.value == 0;
                                   }
                                   assert(gained && lost);
                                   assert(rinzu_macos_text_composing(first) == 0);
                                   assert(rinzu_macos_text_composing(second) == 0);
                                   assert(rinzu_macos_text_composing(scroll) == RZ_ARGUMENT);
                                   [owner clearEvents];
                                   struct MacRect before, after;
                                   assert(rinzu_macos_scroll_rect(scroll, &before) == 0);
                                   assert(rinzu_macos_scroll_to(scroll, 50, 60) == 0);
                                   assert(rinzu_macos_scroll_to(scroll, 80, 90) == 0);
                                   assert(rinzu_macos_scroll_rect(scroll, &after) == 0);
                                   assert(after.x == 80 && after.y == 90 && after.width > 0 &&
                                          after.height > 0);
                                   BOOL scrolled = NO;
                                   for (unsigned index = 0; index < owner->count; ++index) {
                                     struct MacEvent event =
                                         owner->events[(owner->head + index) % RZ_EVENT_LIMIT];
                                     if (event.kind == RZ_SCROLL &&
                                         event.viewId == rinzu_macos_view_id(scroll)) {
                                       assert(event.flags == 2 && event.x == after.x &&
                                              event.y == after.y);
                                       assert(event.deltaX == after.x - before.x &&
                                              event.deltaY == after.y - before.y);
                                       scrolled = YES;
                                     }
                                   }
                                   assert(scrolled);
                                   assert(rinzu_macos_scroll_to(scroll, NAN, 0) == RZ_ARGUMENT);
                                   assert(rinzu_macos_scroll_to(first, 0, 0) == RZ_ARGUMENT);
                                   [owner clearEvents];
                                   assert(rinzu_macos_window_state(window) & 1);
                                   assert(rinzu_macos_window_visibility(window, 1) == 0);
                                   assert(!(rinzu_macos_window_state(window) & 1));
                                   BOOL hidden = NO;
                                   for (unsigned index = 0; index < owner->count; ++index) {
                                     struct MacEvent event =
                                         owner->events[(owner->head + index) % RZ_EVENT_LIMIT];
                                     hidden |= event.kind == RZ_WINDOW_STATE && !(event.flags & 1);
                                   }
                                   assert(hidden);
                                   assert(rinzu_macos_window_visibility(window, 3) == 0);
                                   assert(rinzu_macos_window_state(window) & 1);
                                   assert(rinzu_macos_stop(session) == 0);
                                   [tick invalidate];
                                   tested = YES;
                                 }];
    assert(rinzu_macos_run_events(session) == 0);
    [timer invalidate];
    assert(tested);
    assert(rinzu_macos_close(session) == 0);
    struct MacStatistics stats;
    assert(rinzu_macos_statistics(&stats) == 0);
    assert(stats.retainedControls + stats.eventPayloadBytes + stats.sheets + stats.sheetBytes == 0);
    puts("PASS:state:focus:scroll:window:coalescing:cleanup");
  }
}

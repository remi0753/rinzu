#include "../native/src/macos.m"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>

static void *session, *window, *data_view;
static int events, activations, sorts, expansions;
static void *make_view(int64_t kind, int64_t columns) {
  struct MacRect rect = {8, 8, 400, 240};
  int64_t identity = rinzu_macos_data_create(window, kind, columns, &rect, 201);
  assert(identity > 0);
  void *view = rinzu_macos_view(identity);
  assert(rinzu_macos_view_add(rinzu_macos_window_content(window), view) == 0);
  return view;
}
static void add(void *view, int64_t token, int64_t identity, int64_t parent,
                NSArray<NSString *> *cells) {
  NSMutableString *wire = [NSMutableString new];
  for (NSString *cell in cells)
    [wire appendFormat:@"%lu:%@",
                       (unsigned long)[cell lengthOfBytesUsingEncoding:NSUTF8StringEncoding], cell];
  NSData *bytes = [wire dataUsingEncoding:NSUTF8StringEncoding];
  assert(rinzu_macos_data_append(view, token, identity, parent, bytes.bytes, bytes.length) == 0);
}
static struct MacDataState state(void *view) {
  struct MacDataState value;
  assert(rinzu_macos_data_state(view, &value) == 0);
  return value;
}
static void *foreign(void *view) {
  @autoreleasepool {
    struct MacDataState value;
    assert(rinzu_macos_data_state(view, &value) == RZ_OWNER);
    assert(rinzu_macos_data_begin(view, 1) == RZ_OWNER);
  }
  return NULL;
}
static void model(void) {
  __weak RZMacDataController *released;
  @autoreleasepool {
    data_view = make_view(2, 2);
    assert(rinzu_macos_data_column(data_view, 0, (const uint8_t *)"Name", 4, 180, true) == 0);
    assert(rinzu_macos_data_column(data_view, 1, (const uint8_t *)"Value", 5, 200, true) == 0);
    int64_t token = rinzu_macos_wait_token();
    assert(rinzu_macos_data_begin(data_view, token) == 0);
    assert(rinzu_macos_data_begin(data_view, rinzu_macos_wait_token()) == RZ_BUSY);
    add(data_view, token, 10, 0, @[ @"日本語 😀", @"A\0B" ]);
    add(data_view, token, INT64_C(9007199254740993), 0, @[ @"Item 2", @"second" ]);
    assert(state(data_view).rows == 0 && state(data_view).pendingRows == 2);
    assert(rinzu_macos_data_commit(data_view, token) == 0);
    assert(!rinzu_macos_data_owned(data_view, token));
    assert(state(data_view).rows == 2 && state(data_view).revision == 1);
    int64_t reader = rinzu_macos_wait_token();
    assert(rinzu_macos_data_cell(data_view, reader, 10, 1) == 0);
    const rinzu_native_byte_array_t *bytes = rinzu_macos_widget_bytes(data_view, reader);
    assert(bytes && bytes->length == 3 && !memcmp(bytes->data, "A\0B", 3));
    assert(rinzu_macos_view_destroy(data_view) == RZ_BUSY);
    assert(rinzu_macos_widget_snapshot_finish(data_view, reader) == 0);
    assert(rinzu_macos_data_select(data_view, INT64_C(9007199254740993), true) == 0);
    assert(state(data_view).selectedId == INT64_C(9007199254740993));
    token = rinzu_macos_wait_token();
    assert(rinzu_macos_data_begin(data_view, token) == 0);
    add(data_view, token, 10, 0, @[ @"changed", @"new" ]);
    assert(rinzu_macos_data_append(data_view, token, 10, 0, (const uint8_t *)"1:x1:y", 6) ==
           RZ_ARGUMENT);
    assert(rinzu_macos_data_append(data_view, token, 11, 0,
                                   (const uint8_t *)"1:\xff"
                                                    "1:y",
                                   6) == RZ_ARGUMENT);
    assert(rinzu_macos_data_append(data_view, token, 11, 10, (const uint8_t *)"1:x1:y", 6) ==
           RZ_ARGUMENT);
    assert(rinzu_macos_data_append(data_view, token, 11, 0, (const uint8_t *)"9999999:x", 9) ==
           RZ_ARGUMENT);
    assert(rinzu_macos_data_append(data_view, token, 11, 0, NULL, SIZE_MAX) == RZ_ARGUMENT);
    assert(state(data_view).pendingRows == 1);
    assert(rinzu_macos_data_cancel(data_view, token) == 0);
    assert(state(data_view).rows == 2 && state(data_view).revision == 1);
    void *bounded = make_view(1, 1);
    int64_t bounded_token = rinzu_macos_wait_token();
    assert(rinzu_macos_data_begin(bounded, bounded_token) == 0);
    NSString *block = [@"x" stringByPaddingToLength:65536 withString:@"x" startingAtIndex:0];
    for (int64_t i = 1; i <= 256; ++i)
      add(bounded, bounded_token, i, 0, @[ block ]);
    assert(data_control(bounded).pending.bytes == 16777216);
    assert(rinzu_macos_data_append(bounded, bounded_token, 257, 0, (const uint8_t *)"1:x", 3) ==
           RZ_ARGUMENT);
    assert(rinzu_macos_data_cancel(bounded, bounded_token) == 0);
    assert(rinzu_macos_view_destroy(bounded) == 0);
    assert(rinzu_macos_data_state(window, NULL) == RZ_INVALID);
    assert(rinzu_macos_data_state(data_view, NULL) == RZ_ARGUMENT);
    assert(rinzu_macos_data_column(data_view, 0, (const uint8_t *)"x", 1, NAN, false) ==
           RZ_ARGUMENT);
    pthread_t thread;
    assert(pthread_create(&thread, NULL, foreign, data_view) == 0 &&
           pthread_join(thread, NULL) == 0);
    for (unsigned i = 0; i < 1000; ++i) {
      __weak RZMacDataModel *released;
      @autoreleasepool {
        token = rinzu_macos_wait_token();
        assert(rinzu_macos_data_begin(data_view, token) == 0);
        released = data_control(data_view).pending;
        add(data_view, token, 1, 0, @[ @"temporary", @"row" ]);
        assert(rinzu_macos_data_cancel(data_view, token) == 0);
      }
      assert(!released);
    }
    released = data_control(data_view);
    assert(rinzu_macos_view_destroy(data_view) == 0);
    assert(rinzu_macos_data_begin(data_view, 1) == RZ_INVALID);
  }
  assert(!released);
  puts("PASS:data:model:unicode:nul:int64-id:atomic:bounds:owner:1000-release");
}
static void tree(void) {
  data_view = make_view(3, 1);
  int64_t token = rinzu_macos_wait_token();
  assert(rinzu_macos_data_begin(data_view, token) == 0);
  for (int64_t i = 1; i <= 64; ++i)
    add(data_view, token, i, i - 1, @[ [NSString stringWithFormat:@"Node %lld", (long long)i] ]);
  assert(rinzu_macos_data_append(data_view, token, 65, 64, (const uint8_t *)"1:x", 3) ==
         RZ_ARGUMENT);
  assert(rinzu_macos_data_append(data_view, token, 66, 999, (const uint8_t *)"1:x", 3) ==
         RZ_ARGUMENT);
  assert(rinzu_macos_data_commit(data_view, token) == 0);
  assert(state(data_view).visibleRows == 1);
  assert(rinzu_macos_data_select(data_view, 64, false) == RZ_ARGUMENT);
  assert(rinzu_macos_data_select(data_view, 64, true) == 0);
  assert(state(data_view).visibleRows == 64 && state(data_view).selectedId == 64);
  assert(rinzu_macos_data_expand(data_view, 1, false) == 0);
  assert(state(data_view).visibleRows == 1 && rinzu_macos_data_expanded(data_view, 1) == 0);
  assert(rinzu_macos_data_expand(data_view, 1, true) == 0);
  assert(rinzu_macos_data_sort(data_view, 0, false) == 0);
  token = rinzu_macos_wait_token();
  assert(rinzu_macos_data_begin(data_view, token) == 0);
  add(data_view, token, 1, 0, @[ @"Root" ]);
  add(data_view, token, 2, 1, @[ @"Child" ]);
  assert(rinzu_macos_data_commit(data_view, token) == 0);
  assert(rinzu_macos_data_expanded(data_view, 1) == 1);
  assert(state(data_view).visibleRows == 2);
  puts("PASS:data:tree:depth:parent:reveal:expand:preserve:sort");
}
static void large(void) {
  data_view = make_view(2, 2);
  int64_t token = rinzu_macos_wait_token();
  assert(rinzu_macos_data_begin(data_view, token) == 0);
  for (int64_t i = 1; i <= 100000; ++i)
    add(data_view, token, i, 0,
        @[ [NSString stringWithFormat:@"Item %lld", (long long)i], @"日本語 😀" ]);
  assert(rinzu_macos_data_append(data_view, token, 100001, 0, (const uint8_t *)"1:x1:y", 6) ==
         RZ_ARGUMENT);
  assert(rinzu_macos_data_commit(data_view, token) == 0);
  assert(state(data_view).rows == 100000);
}
static void list(void) {
  data_view = make_view(1, 1);
  assert(rinzu_macos_data_column(data_view, 0, NULL, 0, 160, false) == 0);
  int64_t token = rinzu_macos_wait_token();
  assert(rinzu_macos_data_begin(data_view, token) == 0);
  add(data_view, token, 1, 0, @[ @"A long task title / 日本語 😀 should use the available width" ]);
  add(data_view, token, 2, 0, @[ @"Second row" ]);
  assert(rinzu_macos_data_commit(data_view, token) == 0);
}
static void receive(void *raw, const struct MacEvent *event) {
  assert(raw == session);
  if (event->kind == RZ_DATA) {
    assert(event->action == 201 && event->viewId == (int64_t)(uintptr_t)data_view);
    ++events;
    activations += event->flags == 2;
    sorts += event->flags == 5;
    expansions += event->flags == 3;
  }
}
int main(int argc, const char **argv) {
  @autoreleasepool {
    assert(argc == 2);
    session = rinzu_macos_session(rinzu_macos_open());
    window = rinzu_macos_window(
        rinzu_macos_window_create(session, (const uint8_t *)"Data test", 9, 420, 260));
    if (!strcmp(argv[1], "model"))
      model();
    else if (!strcmp(argv[1], "tree"))
      tree();
    else if (!strcmp(argv[1], "list"))
      list();
    else {
      assert(!strcmp(argv[1], "large"));
      large();
    }
    assert(rinzu_macos_window_theme(window, 2) == 0 &&
           rinzu_macos_window_effective_theme(window) == 2);
    assert(rinzu_macos_window_theme(window, 1) == 0 &&
           rinzu_macos_window_effective_theme(window) == 1);
    assert(rinzu_macos_window_theme(window, 0) == 0 &&
           rinzu_macos_window_theme(window, 3) == RZ_ARGUMENT);
    if (data_control(data_view)) {
      assert(rinzu_macos_view_accessibility(data_view, (const uint8_t *)"Records", 7,
                                            (const uint8_t *)"Navigate rows", 13,
                                            (const uint8_t *)"records", 7) == 0);
      assert([data_control(data_view).table.accessibilityLabel isEqualToString:@"Records"]);
      assert(rinzu_macos_view_tooltip(data_view, (const uint8_t *)"Data rows", 9) == 0);
      assert(rinzu_macos_window_show(window) == 0 && rinzu_macos_view_focus(data_view) == 0);
      __block unsigned phase = 0;
      NSTimer *timer = [NSTimer
          scheduledTimerWithTimeInterval:0.02
                                 repeats:YES
                                   block:^(NSTimer *tick) {
                                     (void)tick;
                                     RZMacDataController *data = data_control(data_view);
                                     [data.table layoutSubtreeIfNeeded];
                                     if (phase == 0) {
                                       struct MacDataState a = state(data_view);
                                       assert(a.cellViews > 0 && a.cellViews < 500);
                                       if (!data.table.headerView) {
                                         assert(data.table.tableColumns.firstObject.width > 300);
                                         struct MacRect larger = {8, 8, 600, 240};
                                         assert(rinzu_macos_window_size(window, 620, 260) == 0);
                                         assert(rinzu_macos_view_set_frame(data_view, &larger) ==
                                                0);
                                       }
                                       int64_t identity = a.rows == 100000 ? 100000 : 2;
                                       assert(rinzu_macos_data_select(data_view, identity, true) ==
                                              0);
                                       phase = 1;
                                     } else if (phase == 1) {
                                       struct MacDataState a = state(data_view);
                                       assert(a.cellViews > 0 && a.cellViews < 1000);
                                       if (!data.table.headerView) {
                                         assert(data.table.tableColumns.firstObject.width > 500);
                                         puts("PASS:data:list:column-fill:resize");
                                       }
                                       if (a.rows == 100000) {
                                         assert(a.firstVisible > 99000 && a.lastVisible == 99999);
                                         assert(rinzu_macos_data_sort(data_view, 0, false) == 0);
                                         assert(state(data_view).selectedId == 100000 &&
                                                state(data_view).selectedRow == 0);
                                       }
                                       [data.table selectRowIndexes:[NSIndexSet indexSetWithIndex:0]
                                               byExtendingSelection:NO];
                                       NSEvent *key = [NSEvent keyEventWithType:NSEventTypeKeyDown
                                                                       location:NSZeroPoint
                                                                  modifierFlags:0
                                                                      timestamp:0
                                                                   windowNumber:data.table.window
                                                                                    .windowNumber
                                                                        context:nil
                                                                     characters:@"\r"
                                                    charactersIgnoringModifiers:@"\r"
                                                                      isARepeat:NO
                                                                        keyCode:36];
                                       [data.table keyDown:key];
                                       data.table.sortDescriptors =
                                           @[ [NSSortDescriptor sortDescriptorWithKey:@"0"
                                                                            ascending:YES] ];
                                       if (data.tree)
                                         [(NSOutlineView *)data.table
                                             expandItem:data.model.roots.firstObject];
                                       phase = 2;
                                     } else {
                                       if (!activations || !sorts || events < 2)
                                         return;
                                       assert(activations == 1 && sorts == 1);
                                       assert(rinzu_macos_stop(session) == 0);
                                     }
                                   }];
      assert(rinzu_macos_run(session, session, receive) == 0);
      [timer invalidate];
      puts("PASS:data:virtual-cells:scroll:keyboard:sort:theme:accessibility");
    }
    assert(rinzu_macos_close(session) == 0);
    struct MacStatistics stats;
    assert(rinzu_macos_statistics(&stats) == 0 && !stats.retainedControls && !stats.dataRows &&
           !stats.dataBytes && !stats.dataUpdates);
    puts("PASS:data:closed");
  }
}

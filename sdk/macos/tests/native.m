// Real framework tests. Include the implementation to inject boundary failures
// without exporting test-only entry points in the production SDK.
#include "../native/src/macos.m"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
#include <time.h>
#include <unistd.h>

static void *session;
static void *first_window;
static void *child_view;
static void *surface;
static void *render_command;
static void *render_pipeline;
static int starts, inputs, closes, quits, frames;
static BOOL in_handler;

// A console host has no AppKit event-loop pool. Call only the public C ABI here.
static void pools(void) {
  void *scope = rinzu_macos_session(rinzu_macos_open());
  assert(scope);
  for (int i = 0; i < 8; ++i) {
    assert(rinzu_macos_window_id(NULL) == RZ_INVALID);
    const rinzu_native_byte_array_t *message = rinzu_macos_diagnostic();
    assert(message && message->data && message->length);
  }
  assert(rinzu_macos_close(scope) == 0);
  puts("PASS:macos-pools:console-error-boundaries");
}

@interface RZMacThrowingView : RZMacNativeView
@property(nonatomic) BOOL throwOnSet;
@property(nonatomic) BOOL throwOOM;
@end
@implementation RZMacThrowingView
- (void)setFrame:(NSRect)rect {
  if (self.throwOOM)
    [NSException raise:NSMallocException format:@"injected allocation failure"];
  if (self.throwOnSet)
    [NSException raise:@"RinzuMacBoundary" format:@"injected frame exception"];
  [super setFrame:rect];
}
@end
static void oom(void) {
  void *scope = rinzu_macos_session(rinzu_macos_open());
  struct MacRect rect = {0, 0, 10, 10};
  void *raw = rinzu_macos_view(rinzu_macos_view_create(scope, &rect));
  RZMacView *control = (RZMacView *)lookup(raw, RZ_VIEW);
  assert(control);
  RZMacThrowingView *view = [[RZMacThrowingView alloc] initWithFrame:NSMakeRect(0, 0, 10, 10)];
  control.view = view;
  view.control = control;
  view.throwOOM = YES;
  rect.width = 11;
  rinzu_macos_view_set_frame(raw, &rect);
  assert(!"NSMallocException must be process-fatal");
}
static void *wrong_thread(void *raw) {
  @autoreleasepool {
    struct MacRect rect = {0, 0, 10, 10};
    assert(rinzu_macos_view_create(raw, &rect) == RZ_OWNER);
    assert(rinzu_macos_close(raw) == RZ_OWNER);
    struct MacStatistics stats;
    assert(rinzu_macos_statistics(&stats) == RZ_OWNER);
  }
  return NULL;
}
static void baseline(void) {
  struct MacStatistics stats = {.sessions = -1};
  assert(rinzu_macos_statistics(&stats) == 0);
  assert(stats.sessions == 0 && stats.windows == 0 && stats.views == 0 && stats.devices == 0 &&
         stats.buffers == 0 && stats.libraries == 0 && stats.pipelines == 0 &&
         stats.commands == 0 && stats.bufferBytes == 0);
  assert(stats.retainedControls == 0);
}
static void core(void) {
  int64_t identity = rinzu_macos_open();
  assert(identity > 0);
  session = rinzu_macos_session(identity);
  assert(session);
  assert(rinzu_macos_open() == RZ_BUSY);
  assert(rinzu_macos_window_id(session) == RZ_INVALID);
  assert(rinzu_macos_window_id((void *)(uintptr_t)1) == RZ_INVALID);
  assert(rinzu_macos_window_id((void *)UINTPTR_MAX) == RZ_INVALID);
  assert(rinzu_macos_close(NULL) == RZ_INVALID);
  pthread_t thread;
  assert(pthread_create(&thread, NULL, wrong_thread, session) == 0);
  assert(pthread_join(thread, NULL) == 0);
  struct MacRect rect = {0, 0, 32, 24}, invalid = {0, 0, NAN, 1};
  assert(rinzu_macos_view_create(session, &invalid) == RZ_ARGUMENT);
  void *parent = rinzu_macos_view(rinzu_macos_view_create(session, &rect));
  assert(parent);
  void *child = rinzu_macos_view(rinzu_macos_view_create(session, &rect));
  assert(child);
  assert(rinzu_macos_view_add(parent, child) == 0);
  assert(rinzu_macos_view_add(child, parent) == RZ_ARGUMENT);
  assert(rinzu_macos_view_add(child, child) == RZ_ARGUMENT);
  RZMacView *control = (RZMacView *)lookup(child, RZ_VIEW);
  RZMacThrowingView *throwing = [[RZMacThrowingView alloc] initWithFrame:NSMakeRect(0, 0, 32, 24)];
  [control.view removeFromSuperview];
  control.view = throwing;
  throwing.control = control;
  throwing.throwOnSet = YES;
  rect.width = 33;
  assert(rinzu_macos_view_set_frame(child, &rect) == RZ_EXCEPTION);
  const rinzu_native_byte_array_t *message = rinzu_macos_diagnostic();
  assert(message && message->length);
  throwing.throwOnSet = NO;
  assert(rinzu_macos_view_set_frame(child, &rect) == 0);
  assert(rinzu_macos_view_destroy(parent) == 0);
  assert(rinzu_macos_view_id(child) == RZ_INVALID);
  assert(rinzu_macos_prepare_close(session) == 0);
  assert(rinzu_macos_view_create(session, &rect) == RZ_BUSY);
  assert(rinzu_macos_close(session) == 0);
  assert(rinzu_macos_close(session) == RZ_INVALID);
  assert(rinzu_macos_session(identity) == NULL);
  baseline();
  int64_t next = rinzu_macos_open();
  assert(next > identity);
  assert(rinzu_macos_close(rinzu_macos_session(next)) == 0);
  baseline();
  void *scope = rinzu_macos_session(rinzu_macos_open());
  void *stale = NULL;
  for (int i = 0; i < 10000; ++i) {
    __weak RZMacView *released;
    @autoreleasepool {
      void *value = rinzu_macos_view(rinzu_macos_view_create(scope, &rect));
      assert(value && value != stale);
      released = (RZMacView *)lookup(value, RZ_VIEW);
      assert(rinzu_macos_view_id(stale) == RZ_INVALID);
      assert(rinzu_macos_view_destroy(value) == 0);
      stale = value;
    }
    assert(!released);
    struct MacStatistics stats;
    assert(rinzu_macos_statistics(&stats) == 0);
    assert(stats.views == 0 && stats.retainedControls == 1);
  }
  assert(rinzu_macos_close(scope) == 0);
  assert(rinzu_macos_view_id(stale) == RZ_INVALID);
  baseline();
  puts("PASS:macos-core:owner:identity:cycle:exception:closed:cleanup");
}
static const char *shader =
    "#include <metal_stdlib>\nusing namespace metal;\n"
    "kernel void increment(device uchar *data [[buffer(0)]], uint i [[thread_position_in_grid]]) { "
    "data[i] += 1; }\n"
    "vertex float4 triangle(uint i [[vertex_id]]) { const float2 p[3] = "
    "{float2(0,0.7),float2(-0.7,-0.7),float2(0.7,-0.7)}; return float4(p[i],0,1); }\n"
    "fragment float4 color() { return float4(0.2,0.7,0.9,1); }\n";
static void wait_native(void *command) {
  int64_t token = rinzu_macos_wait_token(), other = rinzu_macos_wait_token();
  assert(token > 0 && other > token);
  assert(rinzu_macos_command_wait_begin(command, token) == 0);
  assert(rinzu_macos_command_wait_owned(command, token));
  assert(!rinzu_macos_command_wait_owned(command, other));
  assert(rinzu_macos_command_wait_begin(command, other) == RZ_BUSY);
  assert(rinzu_macos_command_completion(command, other) == NULL);
  assert(rinzu_macos_command_finish(command, other) == RZ_BUSY);
  rinzu_native_completion_t *completion = rinzu_macos_command_completion(command, token);
  assert(completion && rinzu_native_completion_valid(completion));
  for (unsigned tick = 0; !rinzu_native_completion_is_done(completion); ++tick) {
    assert(tick < 20000);
    usleep(1000);
  }
  assert(rinzu_macos_command_status(command) == 0);
  assert(rinzu_macos_command_finish(command, token) == 0);
  assert(!rinzu_macos_command_wait_owned(command, token));
  assert(rinzu_macos_command_finish(command, token) == RZ_BUSY);
}
static void *library_for(void *device) {
  int64_t identity = rinzu_macos_library_create(device, (const uint8_t *)shader, strlen(shader), 0);
  if (identity <= 0) {
    const rinzu_native_byte_array_t *message = rinzu_macos_diagnostic();
    if (message)
      fwrite(message->data, 1, message->length, stderr);
  }
  assert(identity > 0);
  return rinzu_macos_library(identity);
}
static void gpu(void) {
  session = rinzu_macos_session(rinzu_macos_open());
  assert(session);
  if (!rinzu_macos_metal_available()) {
    assert(rinzu_macos_device_create(session) == RZ_NO_METAL);
    assert(rinzu_macos_close(session) == 0);
    fputs("SKIP:Metal device unavailable\n", stderr);
    exit(77);
  }
  void *device = rinzu_macos_device(rinzu_macos_device_create(session));
  assert(device);
  void *other = rinzu_macos_device(rinzu_macos_device_create(session));
  assert(other);
  const uint8_t initial[] = {0, 255, 64, 127};
  void *buffer = rinzu_macos_buffer(rinzu_macos_buffer_create(device, initial, sizeof(initial)));
  assert(buffer);
  void *foreign = rinzu_macos_buffer(rinzu_macos_buffer_create(other, initial, sizeof(initial)));
  assert(foreign);
  assert(rinzu_macos_buffer_allocate(device, -1) == RZ_ARGUMENT);
  assert(rinzu_macos_buffer_read(buffer, 4, 1) == RZ_ARGUMENT);
  assert(rinzu_macos_buffer_read(buffer, -1, 0) == RZ_ARGUMENT);
  assert(rinzu_macos_library_create(device, (const uint8_t *)"invalid shader", 14, 0) == RZ_SHADER);
  void *library = library_for(device);
  void *pipeline =
      rinzu_macos_compute(rinzu_macos_compute_pipeline(library, (const uint8_t *)"increment", 9));
  assert(pipeline);
  assert(rinzu_macos_compute_pipeline(library, (const uint8_t *)"missing", 7) == RZ_SHADER);
  void *command = rinzu_macos_command(rinzu_macos_command_create(device));
  assert(command);
  assert(rinzu_macos_command_wait_begin(command, rinzu_macos_wait_token()) == RZ_BUSY);
  struct MacStatistics before, after;
  assert(rinzu_macos_statistics(&before) == 0);
  for (unsigned iteration = 0; iteration < 100; ++iteration) {
    assert(rinzu_macos_command_compute(command, pipeline) == 0);
    assert(rinzu_macos_command_buffer(command, foreign, 0, 0, 0) == RZ_ARGUMENT);
    assert(rinzu_macos_command_buffer(command, buffer, 1, 0, 0) == RZ_ARGUMENT);
    assert(rinzu_macos_command_buffer(command, buffer, 0, 0, 0) == 0);
    assert(rinzu_macos_buffer_destroy(buffer) == RZ_BUSY);
    assert(rinzu_macos_compute_destroy(pipeline) == RZ_BUSY);
    assert(rinzu_macos_command_destroy(command) == RZ_BUSY);
    assert(rinzu_macos_device_destroy(device) == RZ_BUSY);
    assert(rinzu_macos_buffer_write(buffer, 0, initial, 4) == RZ_BUSY);
    assert(rinzu_macos_buffer_read(buffer, 0, 4) == RZ_BUSY);
    assert(rinzu_macos_command_dispatch(command, 4, 1, 1, 0, 1, 1) == RZ_ARGUMENT);
    assert(rinzu_macos_command_dispatch(command, 4, 1, 1, 4, 1, 1) == 0);
    assert(rinzu_macos_command_commit(command) == 0);
    assert(rinzu_macos_command_abandon(command) == RZ_BUSY);
    assert(rinzu_macos_close(session) == RZ_BUSY);
    wait_native(command);
    assert(rinzu_macos_buffer_read(buffer, 0, 4) == 0);
    const rinzu_native_byte_array_t *bytes = rinzu_macos_buffer_bytes(buffer);
    assert(bytes && bytes->length == 4);
    for (unsigned i = 0; i < 4; ++i)
      assert(bytes->data[i] == (uint8_t)(initial[i] + iteration + 1));
  }
  assert(rinzu_macos_statistics(&after) == 0);
  assert(after.retainedControls == before.retainedControls && after.commands == 1 &&
         after.bufferBytes == 8);
  assert(rinzu_macos_command_compute(command, pipeline) == 0);
  assert(rinzu_macos_command_buffer(command, buffer, 0, 0, 0) == 0);
  assert(rinzu_macos_command_abandon(command) == 0);
  assert(rinzu_macos_buffer_destroy(buffer) == 0);
  assert(rinzu_macos_buffer_read(buffer, 0, 0) == RZ_INVALID);
  assert(rinzu_macos_compute_destroy(pipeline) == 0);
  assert(rinzu_macos_library_destroy(library) == 0);
  assert(rinzu_macos_command_destroy(command) == 0);
  assert(rinzu_macos_device_destroy(device) == 0);
  assert(rinzu_macos_close(session) == 0);
  baseline();
  puts("PASS:macos-gpu:compute:100-reuse:copy:range:leases:cleanup");
}
static struct {
  void *session;
  uint64_t marker;
} run_context;
static void receive(void *raw, const struct MacEvent *event) {
  assert(raw == &run_context && run_context.session == session &&
         run_context.marker == UINT64_C(0x13579bdf) && !in_handler);
  in_handler = YES;
  if (event->kind == RZ_STARTED) {
    ++starts;
    assert(rinzu_macos_close(session) == RZ_BUSY);
    assert(rinzu_macos_window_size(first_window, 350, 240) == 0);
    RZMacWindow *window = (RZMacWindow *)lookup(first_window, RZ_WINDOW);
    assert([window windowShouldClose:window.window] == NO);
    RZMacSession *owner = (RZMacSession *)lookup(session, RZ_SESSION);
    if (surface) {
      NSMenuItem *quit = NSApp.mainMenu.itemArray.firstObject.submenu.itemArray.lastObject;
      assert(quit.action == @selector(terminate:));
      assert([NSApp sendAction:quit.action to:quit.target from:quit]);
    } else {
      assert([owner applicationShouldTerminate:NSApp] == NSTerminateCancel);
    }
    RZMacView *view = (RZMacView *)lookup(child_view, RZ_VIEW);
    NSEvent *key = [NSEvent keyEventWithType:NSEventTypeKeyDown
                                    location:NSZeroPoint
                               modifierFlags:0
                                   timestamp:0
                                windowNumber:window.window.windowNumber
                                     context:nil
                                  characters:@"x"
                 charactersIgnoringModifiers:@"x"
                                   isARepeat:NO
                                     keyCode:7];
    [view.view keyDown:key];
    assert(inputs == 0); // setter/input did not synchronously reenter receive.
    if (surface)
      assert(rinzu_macos_view_request_draw(surface) == 0);
  } else if (event->kind == RZ_KEY_DOWN) {
    assert(event->keyCode == 7);
    ++inputs;
  } else if (event->kind == RZ_CLOSE_REQUESTED) {
    assert(event->windowId == rinzu_macos_window_id(first_window));
    ++closes;
  } else if (event->kind == RZ_QUIT_REQUESTED) {
    ++quits;
  } else if (event->kind == RZ_DRAW && surface && starts &&
             event->viewId == rinzu_macos_view_id(surface)) {
    struct MacColor clear = {0.04, 0.06, 0.1, 1};
    assert(rinzu_macos_command_render(render_command, surface, &clear) == 0);
    assert(rinzu_macos_command_pipeline(render_command, render_pipeline) == 0);
    assert(rinzu_macos_command_draw(render_command, 0, 3) == 0);
    assert(rinzu_macos_view_destroy(surface) == RZ_BUSY);
    assert(rinzu_macos_command_commit(render_command) == 0);
    wait_native(render_command);
    ++frames;
    if (frames < 3)
      assert(rinzu_macos_view_request_draw(surface) == 0);
  }
  if (starts && inputs && closes && quits && (!surface || frames >= 3))
    assert(rinzu_macos_stop(session) == 0);
  in_handler = NO;
}
static void ui(BOOL metal) {
  if (metal && !rinzu_macos_metal_available()) {
    fputs("SKIP:Metal device unavailable\n", stderr);
    exit(77);
  }
  session = rinzu_macos_session(rinzu_macos_open());
  assert(session);
  first_window =
      rinzu_macos_window(rinzu_macos_window_create(session, (const uint8_t *)"A\0B", 3, 320, 200));
  void *second = rinzu_macos_window(
      rinzu_macos_window_create(session, (const uint8_t *)"Second", 6, 320, 200));
  assert(first_window && second);
  assert(((RZMacWindow *)lookup(first_window, RZ_WINDOW)).window.title.length == 3);
  struct MacRect rect = {10, 10, 160, 120};
  child_view = rinzu_macos_view(rinzu_macos_view_create(session, &rect));
  assert(child_view);
  void *content = rinzu_macos_window_content(first_window);
  assert(rinzu_macos_view_add(content, child_view) == 0);
  assert(rinzu_macos_view_add(child_view, content) == RZ_ARGUMENT);
  if (metal) {
    void *device = rinzu_macos_device(rinzu_macos_device_create(session));
    assert(device);
    surface = rinzu_macos_view(rinzu_macos_surface_create(device, &rect));
    assert(surface);
    assert(rinzu_macos_view_add(content, surface) == 0);
    render_command = rinzu_macos_command(rinzu_macos_command_create(device));
    assert(render_command);
    void *library = library_for(device);
    render_pipeline = rinzu_macos_render(rinzu_macos_render_pipeline(
        library, (const uint8_t *)"triangle", 8, (const uint8_t *)"color", 5));
    assert(render_pipeline);
  }
  assert(rinzu_macos_window_show(second) == 0);
  assert(rinzu_macos_window_show(first_window) == 0);
  if (!metal) {
    NSMenu *custom = [NSMenu new];
    [custom addItem:[[NSMenuItem alloc] initWithTitle:@"Existing menu"
                                               action:nil
                                        keyEquivalent:@""]];
    NSApp.mainMenu = custom;
  }
  id previous = NSApp.delegate;
  NSMenu *previous_menu = NSApp.mainMenu;
  run_context.session = session;
  run_context.marker = UINT64_C(0x13579bdf);
  assert(rinzu_macos_run(session, &run_context, receive) == 0);
  assert(NSApp.delegate == previous);
  // AppKit may install its default menu when the previous value was nil.
  assert(previous_menu == nil || NSApp.mainMenu == previous_menu);
  RZMacSession *owner = (RZMacSession *)lookup(session, RZ_SESSION);
  assert(owner->handler == NULL && owner->handlerContext == NULL && !owner.running);
  assert(starts == 1 && inputs == 1 && closes == 1 && quits == 1);
  assert(!metal || frames == 3);
  assert(rinzu_macos_prepare_close(session) == 0);
  assert(rinzu_macos_close(session) == 0);
  assert(rinzu_macos_view_id(content) == RZ_INVALID);
  baseline();
  puts(metal ? "PASS:macos-ui-metal:triangle:3-reuse:drawable:retina:cleanup"
             : "PASS:macos-ui:multiwindow:input:close:quit:deferred:cleanup");
}
int main(int argc, char **argv) {
  assert(argc == 2);
  if (!strcmp(argv[1], "pools")) {
    pools();
    return 0;
  }
  @autoreleasepool {
    assert(argc == 2);
    if (!strcmp(argv[1], "core"))
      core();
    else if (!strcmp(argv[1], "oom"))
      oom();
    else if (!strcmp(argv[1], "gpu"))
      gpu();
    else if (!strcmp(argv[1], "ui"))
      ui(NO);
    else if (!strcmp(argv[1], "ui-metal"))
      ui(YES);
    else
      return 2;
  }
  return 0;
}

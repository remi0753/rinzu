#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>
#import <ImageIO/ImageIO.h>
#import <CoreText/CoreText.h>
#include "rinzu_macos.h"
#include "rinzu/native_completion.h"
#include <math.h>
#include <stdlib.h>
#include <string.h>

enum {
  RZ_OWNER = -7801,
  RZ_INVALID = -7802,
  RZ_BUSY = -7803,
  RZ_ARGUMENT = -7804,
  RZ_NO_METAL = -7805,
  RZ_SHADER = -7806,
  RZ_EXCEPTION = -7807,
  RZ_OVERFLOW = -7808,
  RZ_GPU = -7809,
  RZ_NO_DRAWABLE = -7810
};
enum {
  RZ_SESSION = 1,
  RZ_WINDOW,
  RZ_VIEW,
  RZ_DEVICE,
  RZ_BUFFER,
  RZ_LIBRARY,
  RZ_COMPUTE,
  RZ_RENDER,
  RZ_COMMAND,
  RZ_MENU,
  RZ_TEXTURE,
  RZ_SAMPLER,
  RZ_FONT,
  RZ_TEXT,
  RZ_FRAME_CLOCK
};
enum {
  RZ_STARTED = 1,
  RZ_CLOSE_REQUESTED,
  RZ_RESIZED,
  RZ_DRAW,
  RZ_QUIT_REQUESTED,
  RZ_KEY_DOWN,
  RZ_KEY_UP,
  RZ_MOUSE_DOWN,
  RZ_MOUSE_UP,
  RZ_MOUSE_MOVED,
  RZ_STOPPED,
  RZ_ACTION,
  RZ_TEXT_CHANGED,
  RZ_SELECTION_CHANGED,
  RZ_SCROLL,
  RZ_FOCUS,
  RZ_WINDOW_STATE,
  RZ_TASK_FINISHED,
  RZ_INPUT_LIMIT,
  RZ_DATA
};
enum {
  RZ_EVENT_LIMIT = 256,
  RZ_BUFFER_LIMIT = 268435456,
  RZ_TEXT_LIMIT = 1048576,
  RZ_EVENT_BYTES_LIMIT = 8388608
};
@class RZMacSession, RZMacWindow, RZMacView, RZMacDevice, RZMacCommand, RZMacMenu, RZMacSheet,
    RZMacFrameClock, RZMacDataController;
@interface RZMacHandle : NSObject
@property(nonatomic) int kind;
@property(nonatomic) int64_t identity;
@property(nonatomic) BOOL closed;
@property(nonatomic) NSUInteger leases;
@property(nonatomic, weak) RZMacSession *owner;
@end
@implementation RZMacHandle
@end

@interface RZMacSession : RZMacHandle <NSApplicationDelegate> {
@public
  struct MacEvent events[RZ_EVENT_LIMIT];
  unsigned head, count;
  int64_t sequence;
  void (*handler)(void *, const struct MacEvent *);
  void *handlerContext;
  rinzu_native_completion_v2_t eventCompletion;
  rinzu_native_completion_v2_t runCompletion;
  rinzu_native_byte_array_t eventSnapshot;
  rinzu_native_byte_array_t clipboardSnapshot;
  rinzu_native_byte_array_t resourceRootSnapshot;
  struct MacEvent selectedEvent;
}
@property(nonatomic) BOOL running, stopping, overflow, closing, scheduled, delivering;
@property(nonatomic) BOOL streaming, eventSelected, hasRun, streamPrepared, streamCancelled;
@property(nonatomic) int64_t eventWaitToken, runWaitToken, payloadBytes;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, NSData *> *payloads;
@property(nonatomic, strong) NSData *selectedText;
@property(nonatomic, strong) id previousDelegate;
@property(nonatomic, strong) NSMenu *previousMenu;
@property(nonatomic, strong) NSMenu *fallbackMenu;
@property(nonatomic, strong) NSPasteboard *pasteboard;
@property(nonatomic, strong) NSData *clipboardData;
@property(nonatomic, strong) NSData *resourceRootData;
@property(nonatomic) int64_t clipboardToken;
@property(nonatomic, weak) RZMacWindow *activeWindow;
- (void)enqueue:(struct MacEvent)event;
- (void)enqueue:(struct MacEvent)event text:(NSData *)text;
- (void)clearEvents;
- (void)drain;
- (void)started;
@end
@interface RZMacWindow : RZMacHandle <NSWindowDelegate>
@property(nonatomic) int64_t scopeToken;
@property(nonatomic) BOOL closing, sheetsStopping, stateKnown;
@property(nonatomic) int32_t lastState;
@property(nonatomic, strong) NSWindow *window;
@property(nonatomic, strong) RZMacView *content;
@property(nonatomic, strong) RZMacMenu *menu;
@property(nonatomic, strong) RZMacSheet *sheet;
@end
@interface RZMacSheet : NSObject {
@public
  rinzu_native_completion_v2_t completion;
  rinzu_native_byte_array_t snapshot;
}
@property(nonatomic) int64_t token;
@property(nonatomic) int32_t result;
@property(nonatomic) BOOL cancelRequested, dismissIssued, finished;
@property(nonatomic, weak) RZMacWindow *parent;
@property(nonatomic, strong) NSWindow *window;
@property(nonatomic, strong) id payload;
@property(nonatomic, strong) NSData *path;
@property(nonatomic, strong) NSString *failure;
@end
@implementation RZMacSheet
@end
@interface RZMacNativeWindow : NSWindow
@property(nonatomic, weak) RZMacWindow *control;
@end
@interface RZMacMenu : RZMacHandle
@property(nonatomic, strong) NSMenu *menu;
@property(nonatomic, weak) RZMacMenu *parent;
@property(nonatomic, weak) RZMacWindow *scopeWindow;
- (void)activated:(NSMenuItem *)sender;
@end
@interface RZMacView : RZMacHandle <NSTextViewDelegate, NSTextFieldDelegate> {
@public
  rinzu_native_byte_array_t textSnapshot;
}
@property(nonatomic, strong) NSView *view;
@property(nonatomic, weak) RZMacView *parent;
@property(nonatomic, weak) RZMacWindow *rootWindow;
@property(nonatomic, weak) RZMacWindow *scopeWindow;
@property(nonatomic, strong) RZMacDevice *device;
@property(nonatomic) int widgetKind;
@property(nonatomic) int64_t action, snapshotToken;
@property(nonatomic) BOOL updating, enabled, marking;
@property(nonatomic, strong) NSData *snapshotData;
@property(nonatomic, strong) NSTextView *fieldEditor;
@property(nonatomic) NSPoint scrollOrigin;
@property(nonatomic, weak) RZMacFrameClock *clock;
@property(nonatomic, strong) RZMacDataController *dataController;
@end
@interface RZMacDataNode : NSObject
@property(nonatomic) int64_t identity;
@property(nonatomic) NSUInteger depth;
@property(nonatomic, weak) RZMacDataNode *parent;
@property(nonatomic, strong) NSArray<NSString *> *cells;
@property(nonatomic, strong) NSMutableArray<RZMacDataNode *> *children;
@end
@interface RZMacDataModel : NSObject
@property(nonatomic, strong) NSMutableArray<RZMacDataNode *> *rows, *roots;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, RZMacDataNode *> *byId;
@property(nonatomic) int64_t bytes;
@end
@interface RZMacDataController : NSObject <NSTableViewDataSource, NSTableViewDelegate,
                                           NSOutlineViewDataSource, NSOutlineViewDelegate>
@property(nonatomic, weak) RZMacView *view;
@property(nonatomic, weak) NSTableView *table;
@property(nonatomic, strong) RZMacDataModel *model, *pending;
@property(nonatomic, strong) NSMutableSet<NSNumber *> *expanded;
@property(nonatomic, strong) NSHashTable<NSTableCellView *> *cells;
@property(nonatomic) int64_t token, revision, columnBytes;
@property(nonatomic) NSInteger sortColumn;
@property(nonatomic) BOOL tree, updating, ascending;
- (RZMacDataNode *)nodeAtRow:(NSInteger)row;
- (NSInteger)rowForNode:(RZMacDataNode *)node;
- (int64_t)selectedId;
- (void)reloadKeeping:(int64_t)identity;
- (void)sortModel;
- (void)emit:(int64_t)kind;
- (void)activated:(id)sender;
@end
@interface RZMacNativeView : NSView
@property(nonatomic, weak) RZMacView *control;
@end
@interface RZMacDevice : RZMacHandle
@property(nonatomic, strong) id<MTLDevice> device;
@property(nonatomic, strong) id<MTLCommandQueue> queue;
@end
@interface RZMacBuffer : RZMacHandle {
@public
  rinzu_native_byte_array_t snapshot;
}
@property(nonatomic, strong) RZMacDevice *device;
@property(nonatomic, strong) id<MTLBuffer> buffer;
@property(nonatomic) NSUInteger length;
@end
@interface RZMacLibrary : RZMacHandle
@property(nonatomic, strong) RZMacDevice *device;
@property(nonatomic, strong) id<MTLLibrary> library;
@end
@interface RZMacTexture : RZMacHandle {
@public
  rinzu_native_byte_array_t snapshot;
}
@property(nonatomic, strong) RZMacDevice *device;
@property(nonatomic, strong) id<MTLTexture> texture;
@property(nonatomic, strong) NSData *readback;
@property(nonatomic) int format, usage;
@end
@interface RZMacSampler : RZMacHandle
@property(nonatomic, strong) RZMacDevice *device;
@property(nonatomic, strong) id<MTLSamplerState> sampler;
@end
@interface RZMacFont : RZMacHandle {
@public
  CTFontRef font;
}
@end
@interface RZMacText : RZMacHandle {
@public
  CTFrameRef frame;
  struct MacTextMetrics metrics;
}
@property(nonatomic, strong) RZMacFont *font;
@property(nonatomic, strong) NSAttributedString *attributed;
@property(nonatomic) int64_t bytes;
@property(nonatomic) double frameHeight;
@end
@interface RZMacFrameClock : RZMacHandle {
@public
  rinzu_native_completion_v2_t completion;
  struct MacFrameInfo info;
}
@property(nonatomic, weak) RZMacView *view;
@property(nonatomic, strong) NSTimer *timer;
@property(nonatomic) BOOL dirty, animating, stopped, available, wasVisible;
@property(nonatomic) int64_t token, requests, coalesced, ticks, retries;
@property(nonatomic) int32_t failure;
@property(nonatomic) double interval, started, lastTick, retryDelay;
@end
@interface RZMacPipeline : RZMacHandle
@property(nonatomic, strong) RZMacDevice *device;
@property(nonatomic, strong) id<MTLComputePipelineState> compute;
@property(nonatomic, strong) id<MTLRenderPipelineState> render;
@property(nonatomic) int format;
@end
@interface RZMacCommand : RZMacHandle {
@public
  rinzu_native_completion_v2_t completion;
  int32_t result;
  _Atomic uint64_t submitted;
  uint64_t generation;
}
@property(nonatomic, strong) RZMacDevice *device;
@property(nonatomic, strong) id<MTLCommandBuffer> command;
@property(nonatomic, strong) id<MTLComputeCommandEncoder> compute;
@property(nonatomic, strong) id<MTLRenderCommandEncoder> render;
@property(nonatomic, strong) id<CAMetalDrawable> drawable;
@property(nonatomic, strong) RZMacPipeline *pipeline;
@property(nonatomic, strong) RZMacTexture *target;
@property(nonatomic) NSUInteger targetWidth, targetHeight;
@property(nonatomic) int targetFormat;
@property(nonatomic, strong) NSMutableArray<RZMacHandle *> *pins;
@property(nonatomic, strong) NSString *failure;
@property(nonatomic) int state; // 0 idle, 1 recording, 2 submitted; reusable after finish.
@property(nonatomic) int64_t waiterToken;
@end
@implementation RZMacView
@end
@implementation RZMacDevice
@end
@implementation RZMacBuffer
@end
@implementation RZMacLibrary
@end
@implementation RZMacTexture
@end
@implementation RZMacSampler
@end
@implementation RZMacFont
- (void)dealloc {
  if (font)
    CFRelease(font);
}
@end
@implementation RZMacText
- (void)dealloc {
  if (frame)
    CFRelease(frame);
}
@end
@implementation RZMacFrameClock
@end
@implementation RZMacPipeline
@end
@implementation RZMacCommand
@end

// Opaque values carry monotonic logical identities, never NSObject addresses.
// Removing an active entry releases its control; an old alias cannot acquire a
// subsequently allocated object at the same address (ABA).
static NSMutableDictionary<NSNumber *, RZMacHandle *> *handles;
static int64_t next_identity;
static int64_t next_waiter;
static NSData *diagnostic_data;
static rinzu_native_byte_array_t diagnostic_snapshot;
static int32_t fail(int32_t code, NSString *message) {
  @autoreleasepool {
    @try {
      if ([NSThread isMainThread])
        diagnostic_data =
            [(message ?: @"macOS operation failed") dataUsingEncoding:NSUTF8StringEncoding];
    } @catch (NSException *exception) {
      (void)exception;
      // Failure to allocate/encode the diagnostic must not unwind across C.
      _Exit(70);
    }
  }
  return code;
}
static int32_t caught(NSException *exception) {
  if ([exception.name isEqualToString:NSMallocException])
    _Exit(70);
  return fail(RZ_EXCEPTION, exception.reason ?: exception.name);
}
static RZMacHandle *lookup(void *raw, int kind) {
  if (![NSThread isMainThread] || !raw)
    return nil;
  uintptr_t identity = (uintptr_t)raw;
  if (identity > (uint64_t)next_identity)
    return nil;
  RZMacHandle *handle = handles[@(identity)];
  return handle && !handle.closed && handle.kind == kind ? handle : nil;
}
static void *reference(RZMacHandle *handle) {
  return handle ? (void *)(uintptr_t)handle.identity : NULL;
}
static void retire(RZMacHandle *handle) {
  handle.closed = YES;
  [handles removeObjectForKey:@(handle.identity)];
}
static int32_t missing(void) {
  return fail([NSThread isMainThread] ? RZ_INVALID : RZ_OWNER,
              @"invalid, closed, wrong-kind or wrong-thread handle");
}
static BOOL accepting(RZMacHandle *handle) {
  if (handle.owner.closing)
    return NO;
  if (handle.kind == RZ_WINDOW)
    return !((RZMacWindow *)handle).closing;
  if (handle.kind == RZ_VIEW)
    return !((RZMacView *)handle).scopeWindow.closing;
  if (handle.kind == RZ_MENU)
    return !((RZMacMenu *)handle).scopeWindow.closing;
  return YES;
}
static int64_t publish(RZMacHandle *handle, int kind, RZMacSession *owner) {
  if (!handle || next_identity == INT64_MAX)
    _Exit(70);
  if (!handles)
    handles = [NSMutableDictionary new];
  if (!handles)
    _Exit(70);
  handle.kind = kind;
  handle.identity = ++next_identity;
  handle.owner = owner;
  handles[@(handle.identity)] = handle;
  return handle.identity;
}
static void *by_id(int64_t identity, int kind) {
  if (![NSThread isMainThread] || identity <= 0)
    return NULL;
  RZMacHandle *handle = lookup((void *)(uintptr_t)identity, kind);
  return reference(handle);
}
static NSString *text(const uint8_t *bytes, size_t length) {
  if ((length && !bytes) || length > RZ_BUFFER_LIMIT)
    return nil;
  return [[NSString alloc] initWithBytes:bytes length:length encoding:NSUTF8StringEncoding];
}
static BOOL rect_valid(const struct MacRect *rect) {
  return rect && isfinite(rect->x) && isfinite(rect->y) && isfinite(rect->width) &&
         isfinite(rect->height) && fabs(rect->x) <= 1000000 && fabs(rect->y) <= 1000000 &&
         rect->width >= 0 && rect->height >= 0 && rect->width <= 1000000 && rect->height <= 1000000;
}
static NSRect nsrect(const struct MacRect *rect) {
  return NSMakeRect(rect->x, rect->y, rect->width, rect->height);
}
static RZMacWindow *view_window(RZMacView *view) {
  while (view.parent)
    view = view.parent;
  return view.rootWindow;
}
static struct MacEvent view_event(RZMacView *view, int kind) {
  NSRect bounds = view.view.bounds;
  double scale = view.view.window ? view.view.window.backingScaleFactor : 1.0;
  RZMacWindow *window = view_window(view);
  struct MacEvent event = {.kind = kind,
                           .windowId = window.identity,
                           .viewId = view.identity,
                           .width = bounds.size.width,
                           .height = bounds.size.height,
                           .scale = scale};
  return event;
}
static void cancel_sheet(RZMacSheet *sheet);
static void stop_after_sheets(RZMacSession *session);
static void drain_sheets(RZMacSession *session);
static void wake_application(void) {
  NSEvent *wake = [NSEvent otherEventWithType:NSEventTypeApplicationDefined
                                     location:NSZeroPoint
                                modifierFlags:0
                                    timestamp:0
                                 windowNumber:0
                                      context:nil
                                      subtype:0
                                        data1:0
                                        data2:0];
  [NSApp postEvent:wake atStart:NO];
}
static void refresh_frame(RZMacFrameClock *clock, BOOL dirty);
static void stop_frame(RZMacFrameClock *clock);
static void stop_frames(RZMacSession *session, RZMacWindow *window);
static int32_t window_flags(RZMacWindow *window) {
  NSWindow *native = window.window;
  return (native.visible ? 1 : 0) | (native.keyWindow ? 2 : 0) | (native.mainWindow ? 4 : 0) |
         (native.miniaturized ? 8 : 0) | (native.zoomed ? 16 : 0) |
         ((native.styleMask & NSWindowStyleMaskFullScreen) ? 32 : 0);
}
static void window_state(RZMacWindow *window) {
  if (!window || window.closed)
    return;
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_FRAME_CLOCK && (((RZMacFrameClock *)handle).view.rootWindow == window ||
                                          ((RZMacFrameClock *)handle).view.scopeWindow == window))
      refresh_frame((RZMacFrameClock *)handle, YES);
  int32_t flags = window_flags(window);
  if (window.stateKnown && flags == window.lastState)
    return;
  window.stateKnown = YES;
  window.lastState = flags;
  [window.owner enqueue:(struct MacEvent){
                            .kind = RZ_WINDOW_STATE, .windowId = window.identity, .flags = flags}];
}
static RZMacView *responder_view(NSResponder *responder) {
  if (!responder)
    return nil;
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_VIEW && (((RZMacView *)handle).view == responder ||
                                   ((RZMacView *)handle).fieldEditor == responder ||
                                   ((RZMacView *)handle).dataController.table == responder))
      return (RZMacView *)handle;
  return nil;
}
@implementation RZMacNativeWindow
- (BOOL)makeFirstResponder:(NSResponder *)responder {
  RZMacView *before = responder_view(self.firstResponder);
  BOOL accepted = [super makeFirstResponder:responder];
  RZMacView *after = responder_view(self.firstResponder);
  if (before != after) {
    if (before && !before.closed) {
      struct MacEvent event = view_event(before, RZ_FOCUS);
      event.value = 0;
      [before.owner enqueue:event];
    }
    if (after && !after.closed) {
      struct MacEvent event = view_event(after, RZ_FOCUS);
      event.value = 1;
      [after.owner enqueue:event];
    }
  }
  return accepted;
}
- (void)orderWindow:(NSWindowOrderingMode)place relativeTo:(NSInteger)other {
  [super orderWindow:place relativeTo:other];
  window_state(self.control);
}
- (void)zoom:(id)sender {
  [super zoom:sender];
  window_state(self.control);
}
@end
static void resized(RZMacView *view) {
  if (!view || view.closed)
    return;
  if (view.device) {
    CAMetalLayer *layer = (CAMetalLayer *)view.view.layer;
    NSSize size = [view.view convertRectToBacking:view.view.bounds].size;
    layer.contentsScale = view.view.window ? view.view.window.backingScaleFactor : 1.0;
    layer.drawableSize = CGSizeMake(size.width, size.height);
  }
  [view.owner enqueue:view_event(view, RZ_RESIZED)];
  if (view.device)
    [view.owner enqueue:view_event(view, RZ_DRAW)];
  refresh_frame(view.clock, YES);
}
static void input(RZMacView *view, NSEvent *input_event, int kind) {
  if (!view || view.closed)
    return;
  struct MacEvent event = view_event(view, kind);
  if (kind == RZ_KEY_DOWN || kind == RZ_KEY_UP)
    event.keyCode = input_event.keyCode;
  else {
    NSPoint point = [view.view convertPoint:input_event.locationInWindow fromView:nil];
    event.x = point.x;
    event.y = point.y;
  }
  event.modifiers = (int64_t)input_event.modifierFlags;
  [view.owner enqueue:event];
}
@implementation RZMacNativeView
- (BOOL)acceptsFirstResponder {
  return YES;
}
- (void)setFrameSize:(NSSize)size {
  NSSize previous = self.frame.size;
  [super setFrameSize:size];
  if (!NSEqualSizes(previous, self.frame.size))
    resized(self.control);
}
- (void)viewDidMoveToWindow {
  [super viewDidMoveToWindow];
  resized(self.control);
}
- (void)viewDidChangeBackingProperties {
  [super viewDidChangeBackingProperties];
  resized(self.control);
}
- (void)setHidden:(BOOL)hidden {
  BOOL changed = self.hidden != hidden;
  [super setHidden:hidden];
  if (changed)
    for (RZMacHandle *handle in handles.allValues)
      if (handle.kind == RZ_FRAME_CLOCK)
        refresh_frame((RZMacFrameClock *)handle, YES);
}
- (void)drawRect:(NSRect)rect {
  if (self.control.device) {
    [self.control.owner enqueue:view_event(self.control, RZ_DRAW)];
    refresh_frame(self.control.clock, YES);
  } else
    [super drawRect:rect];
}
- (void)keyDown:(NSEvent *)event {
  input(self.control, event, RZ_KEY_DOWN);
}
- (void)keyUp:(NSEvent *)event {
  input(self.control, event, RZ_KEY_UP);
}
- (void)mouseDown:(NSEvent *)event {
  [self.window makeFirstResponder:self];
  input(self.control, event, RZ_MOUSE_DOWN);
}
- (void)mouseUp:(NSEvent *)event {
  input(self.control, event, RZ_MOUSE_UP);
}
- (void)mouseMoved:(NSEvent *)event {
  input(self.control, event, RZ_MOUSE_MOVED);
}
- (void)scrollWheel:(NSEvent *)input_event {
  struct MacEvent event = view_event(self.control, RZ_SCROLL);
  event.deltaX = input_event.scrollingDeltaX;
  event.deltaY = input_event.scrollingDeltaY;
  event.flags = input_event.hasPreciseScrollingDeltas ? 1 : 0;
  [self.control.owner enqueue:event];
}
@end

static void complete_event_wait(RZMacSession *session) {
  if (session.eventWaitToken && !rinzu_native_completion_is_done(&session->eventCompletion.base))
    rinzu_native_completion_v2_publish(&session->eventCompletion);
}
static BOOL event_live(struct MacEvent event) {
  return (!event.viewId || lookup((void *)(uintptr_t)event.viewId, RZ_VIEW)) &&
         (!event.windowId || lookup((void *)(uintptr_t)event.windowId, RZ_WINDOW));
}
static void event_wait_observer(CFRunLoopObserverRef observer, CFRunLoopActivity activity,
                                void *raw) {
  (void)observer;
  (void)activity;
  RZMacSession *session = (__bridge RZMacSession *)raw;
  if (session.eventWaitToken &&
      atomic_load_explicit(&session->eventCompletion.base.cancelled, memory_order_acquire))
    complete_event_wait(session);
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_WINDOW && handle.owner == session) {
      RZMacSheet *sheet = ((RZMacWindow *)handle).sheet;
      if (sheet && (sheet.cancelRequested ||
                    atomic_load_explicit(&sheet->completion.base.cancelled, memory_order_acquire)))
        cancel_sheet(sheet);
    }
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_FRAME_CLOCK && handle.owner == session) {
      RZMacFrameClock *clock = (RZMacFrameClock *)handle;
      if (clock.token &&
          atomic_load_explicit(&clock->completion.base.cancelled, memory_order_acquire))
        rinzu_macos_frame_cancel(reference(clock), clock.token);
      else
        // Observe visibility on existing RunLoop turns; this creates no wakeups.
        refresh_frame(clock, NO);
    }
  stop_after_sheets(session);
}
@implementation RZMacSession
- (void)clearEvents {
  [self.payloads removeAllObjects];
  self.payloadBytes = (int64_t)self.selectedText.length;
  head = count = 0;
}
- (void)enqueue:(struct MacEvent)event {
  [self enqueue:event text:nil];
}
- (void)enqueue:(struct MacEvent)event text:(NSData *)text {
  if ((!handler && !self.streaming) || self.stopping)
    return;
  if (text.length > RZ_TEXT_LIMIT) {
    self.overflow = YES;
    rinzu_macos_stop(reference(self));
    return;
  }
  // A text replacement can notify selection before its edit. Preserve the edit
  // and its resulting selection together, rather than an intermediate selection.
  if (count && event.kind == RZ_TEXT_CHANGED) {
    struct MacEvent last = events[(head + count - 1) % RZ_EVENT_LIMIT];
    if (last.kind == RZ_SELECTION_CHANGED && last.viewId == event.viewId) {
      self.payloadBytes -= (int64_t)self.payloads[@(last.sequence)].length;
      [self.payloads removeObjectForKey:@(last.sequence)];
      --count;
    }
  }
  struct MacEvent *last = count ? &events[(head + count - 1) % RZ_EVENT_LIMIT] : NULL;
  BOOL replaceable = event.kind == RZ_DRAW || event.kind == RZ_RESIZED ||
                     event.kind == RZ_MOUSE_MOVED || event.kind == RZ_TEXT_CHANGED ||
                     event.kind == RZ_SELECTION_CHANGED || event.kind == RZ_TASK_FINISHED ||
                     event.kind == RZ_WINDOW_STATE || event.kind == RZ_SCROLL ||
                     (event.kind == RZ_DATA && event.flags == 1);
  BOOL replace = last && replaceable && last->kind == event.kind && last->viewId == event.viewId &&
                 last->windowId == event.windowId && last->flags == event.flags;
  if (replace && event.kind == RZ_SCROLL) {
    event.deltaX += last->deltaX;
    event.deltaY += last->deltaY;
  }
  int64_t previous = replace ? (int64_t)self.payloads[@(last->sequence)].length : 0;
  if ((!replace && count == RZ_EVENT_LIMIT) || sequence == INT64_MAX ||
      self.payloadBytes - previous + (int64_t)text.length > RZ_EVENT_BYTES_LIMIT) {
    self.overflow = YES;
    rinzu_macos_stop(reference(self));
    return;
  }
  // A coalesced payload is still newer input. Snapshot barriers must distinguish
  // it from the previous contents of the same queue slot.
  event.sequence = ++sequence;
  if (replace) {
    [self.payloads removeObjectForKey:@(last->sequence)];
    *last = event;
  } else {
    events[(head + count++) % RZ_EVENT_LIMIT] = event;
  }
  if (text)
    self.payloads[@(event.sequence)] = [text copy];
  self.payloadBytes += (int64_t)text.length - previous;
  if (self.streaming) {
    complete_event_wait(self);
  } else if (!self.scheduled) {
    self.scheduled = YES;
    [self performSelector:@selector(drain) withObject:nil afterDelay:0];
  }
}
- (void)drain {
  self.scheduled = NO;
  if (!handler || self.stopping || self.delivering)
    return;
  if (!NSApp.running) {
    self.scheduled = YES;
    [self performSelector:@selector(drain) withObject:nil afterDelay:0.001];
    return;
  }
  self.delivering = YES;
  @try {
    unsigned budget = 64;
    while (handler && !self.stopping && count && budget--) {
      struct MacEvent event = events[head];
      head = (head + 1) % RZ_EVENT_LIMIT;
      --count;
      self.payloadBytes -= (int64_t)self.payloads[@(event.sequence)].length;
      [self.payloads removeObjectForKey:@(event.sequence)];
      if (event_live(event))
        handler(handlerContext, &event);
    }
  } @finally {
    self.delivering = NO;
  }
  if (count && handler && !self.stopping && !self.scheduled) {
    self.scheduled = YES;
    [self performSelector:@selector(drain) withObject:nil afterDelay:0];
  }
}
- (void)started {
  if ((!handler && !self.streaming) || self.stopping)
    return;
  if (!NSApp.running) {
    [self performSelector:@selector(started) withObject:nil afterDelay:0.001];
    return;
  }
  [self enqueue:(struct MacEvent){.kind = RZ_STARTED}];
  // A new app can receive its first show request before AppKit finishes launching.
  // Fulfil that request once the OS loop is running, without repeatedly stealing focus.
  if (self.activeWindow.window.visible && !self.activeWindow.window.miniaturized) {
    [self.activeWindow.window makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];
  }
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_WINDOW && handle.owner == self) {
      ((RZMacWindow *)handle).stateKnown = NO;
      window_state((RZMacWindow *)handle);
    }
}
- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender {
  (void)sender;
  [self enqueue:(struct MacEvent){.kind = RZ_QUIT_REQUESTED}];
  return NSTerminateCancel;
}
@end
static void refresh_menu(RZMacSession *session) {
  if (!session.running)
    return;
  NSApp.mainMenu = session.activeWindow.menu.menu ?: session.fallbackMenu;
}
@implementation RZMacWindow
- (BOOL)windowShouldClose:(NSWindow *)sender {
  (void)sender;
  [self.owner enqueue:(struct MacEvent){.kind = RZ_CLOSE_REQUESTED, .windowId = self.identity}];
  return NO;
}
- (void)windowDidResize:(NSNotification *)notification {
  (void)notification;
  resized(self.content);
  window_state(self);
}
- (void)windowDidChangeOcclusionState:(NSNotification *)notification {
  (void)notification;
  window_state(self);
}
- (void)windowDidBecomeKey:(NSNotification *)notification {
  (void)notification;
  self.owner.activeWindow = self;
  refresh_menu(self.owner);
  window_state(self);
}
- (void)windowDidResignKey:(NSNotification *)notification {
  (void)notification;
  window_state(self);
}
- (void)windowDidBecomeMain:(NSNotification *)notification {
  (void)notification;
  window_state(self);
}
- (void)windowDidResignMain:(NSNotification *)notification {
  (void)notification;
  window_state(self);
}
- (void)windowDidMiniaturize:(NSNotification *)notification {
  (void)notification;
  window_state(self);
}
- (void)windowDidDeminiaturize:(NSNotification *)notification {
  (void)notification;
  window_state(self);
}
- (void)windowDidEnterFullScreen:(NSNotification *)notification {
  (void)notification;
  window_state(self);
}
- (void)windowDidExitFullScreen:(NSNotification *)notification {
  (void)notification;
  window_state(self);
}
- (id)windowWillReturnFieldEditor:(NSWindow *)sender toObject:(id)client {
  (void)sender;
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_VIEW && ((RZMacView *)handle).view == client)
      return ((RZMacView *)handle).fieldEditor;
  return nil;
}
@end

int64_t rinzu_macos_open(void) {
  @autoreleasepool {
    @try {
      if (![NSThread isMainThread])
        return missing();
      if (rinzu_macos_has_owner_session())
        return fail(RZ_BUSY, @"a Session is already open");
      RZMacSession *session = [RZMacSession new];
      session.payloads = [NSMutableDictionary new];
      NSBundle *bundle = [NSBundle mainBundle];
      if ([[bundle objectForInfoDictionaryKey:@"RinzuResourceDirectory"] isEqual:@"app"] &&
          [[bundle objectForInfoDictionaryKey:@"CFBundlePackageType"] isEqual:@"APPL"]) {
        NSString *root =
            [bundle.resourceURL URLByAppendingPathComponent:@"app" isDirectory:YES].path;
        session.resourceRootData = [root dataUsingEncoding:NSUTF8StringEncoding];
        if (!session.resourceRootData)
          _Exit(70);
        session->resourceRootSnapshot =
            (rinzu_native_byte_array_t){.data = (uint8_t *)session.resourceRootData.bytes,
                                        .length = session.resourceRootData.length};
      }
      if (!session.payloads)
        _Exit(70);
      rinzu_native_completion_v2_init(&session->runCompletion);
      rinzu_native_completion_v2_publish(&session->runCompletion);
      int64_t identity = publish(session, RZ_SESSION, nil);
      session.owner = session;
      return identity;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_session(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_SESSION);
  }
}
_Bool rinzu_macos_has_owner_session(void) {
  @autoreleasepool {
    return rinzu_macos_owner_session() != NULL;
  }
}
void *rinzu_macos_owner_session(void) {
  @autoreleasepool {
    if (![NSThread isMainThread])
      return NULL;
    for (RZMacHandle *handle in handles.allValues)
      if (handle.kind == RZ_SESSION && !handle.closed)
        return reference(handle);
    return NULL;
  }
}
const rinzu_native_byte_array_t *rinzu_macos_diagnostic(void) {
  @autoreleasepool {
    if (![NSThread isMainThread])
      return NULL;
    diagnostic_snapshot.data = (uint8_t *)diagnostic_data.bytes;
    diagnostic_snapshot.length = diagnostic_data.length;
    return &diagnostic_snapshot;
  }
}
int64_t rinzu_macos_resource_root_size(void *raw) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session ? (int64_t)session.resourceRootData.length : missing();
  }
}
const rinzu_native_byte_array_t *rinzu_macos_resource_root_bytes(void *raw) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session ? &session->resourceRootSnapshot : NULL;
  }
}
int32_t rinzu_macos_resource_relative(const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    if (!length || length > 4096 || !bytes || bytes[0] == '/' || memchr(bytes, 0, length) ||
        !text(bytes, length))
      return fail(RZ_ARGUMENT, @"resource path must be normalized and relative");
    size_t start = 0;
    for (size_t i = 0; i <= length; ++i) {
      if (i < length && bytes[i] != '/')
        continue;
      size_t size = i - start;
      if (!size || (size == 1 && bytes[start] == '.') ||
          (size == 2 && bytes[start] == '.' && bytes[start + 1] == '.'))
        return fail(RZ_ARGUMENT, @"resource path cannot contain empty, dot or parent components");
      start = i + 1;
    }
    return 0;
  }
}
int32_t rinzu_macos_statistics(struct MacStatistics *output) {
  @autoreleasepool {
    if (![NSThread isMainThread])
      return missing();
    if (!output)
      return fail(RZ_ARGUMENT, @"missing statistics output");
    *output = (struct MacStatistics){0};
    output->retainedControls = (int64_t)handles.count;
    for (RZMacHandle *handle in handles.allValues) {
      if (handle.closed)
        continue;
      switch (handle.kind) {
      case RZ_SESSION:
        ++output->sessions;
        output->queuedEvents += ((RZMacSession *)handle)->count;
        output->eventPayloadBytes += ((RZMacSession *)handle).payloadBytes;
        output->snapshotBytes += (int64_t)((RZMacSession *)handle).clipboardData.length;
        break;
      case RZ_WINDOW:
        ++output->windows;
        if (((RZMacWindow *)handle).sheet) {
          ++output->sheets;
          output->sheetBytes += (int64_t)((RZMacWindow *)handle).sheet.path.length;
        }
        break;
      case RZ_MENU:
        break;
      case RZ_VIEW:
        ++output->views;
        output->snapshotBytes += (int64_t)((RZMacView *)handle).snapshotData.length;
        if (((RZMacView *)handle).dataController) {
          RZMacDataController *data = ((RZMacView *)handle).dataController;
          output->dataRows += (int64_t)(data.model.rows.count + data.pending.rows.count);
          output->dataBytes += data.model.bytes + data.pending.bytes + data.columnBytes;
          output->dataUpdates += data.token != 0;
        }
        break;
      case RZ_DEVICE:
        ++output->devices;
        break;
      case RZ_BUFFER:
        ++output->buffers;
        output->bufferBytes += (int64_t)((RZMacBuffer *)handle).length;
        break;
      case RZ_TEXTURE: {
        RZMacTexture *texture = (RZMacTexture *)handle;
        ++output->textures;
        output->textureBytes += (int64_t)(texture.texture.width * texture.texture.height * 4);
        output->snapshotBytes += (int64_t)texture.readback.length;
        break;
      }
      case RZ_SAMPLER:
        ++output->samplers;
        break;
      case RZ_FONT:
        ++output->fonts;
        break;
      case RZ_TEXT:
        ++output->textLayouts;
        output->textBytes += ((RZMacText *)handle).bytes;
        break;
      case RZ_FRAME_CLOCK: {
        RZMacFrameClock *clock = (RZMacFrameClock *)handle;
        ++output->frameClocks;
        output->frameTimers += clock.timer.valid;
        output->frameWaiters += clock.token != 0;
        break;
      }
      case RZ_LIBRARY:
        ++output->libraries;
        break;
      case RZ_COMPUTE:
      case RZ_RENDER:
        ++output->pipelines;
        break;
      case RZ_COMMAND:
        ++output->commands;
        break;
      default:
        _Exit(86);
      }
    }
    return 0;
  }
}

static BOOL descendant(RZMacView *view, RZMacView *root) {
  for (RZMacView *cursor = view; cursor; cursor = cursor.parent)
    if (cursor == root)
      return YES;
  return NO;
}
static BOOL view_busy(RZMacView *root) {
  for (RZMacHandle *handle in handles.allValues)
    if (!handle.closed && handle.kind == RZ_VIEW && handle.leases &&
        descendant((RZMacView *)handle, root))
      return YES;
  return NO;
}
static void close_menu(RZMacMenu *root) {
  if (!root || root.closed)
    return;
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_MENU && ((RZMacMenu *)handle).parent == root)
      close_menu((RZMacMenu *)handle);
  for (NSMenuItem *item in root.menu.itemArray)
    item.target = nil;
  if (root.scopeWindow.menu == root) {
    root.scopeWindow.menu = nil;
    refresh_menu(root.owner);
  }
  if (root.parent)
    for (NSMenuItem *item in root.parent.menu.itemArray)
      if (item.submenu == root.menu)
        [root.parent.menu removeItem:item];
  [root.menu removeAllItems];
  root.menu = nil;
  root.parent = nil;
  root.scopeWindow = nil;
  retire(root);
}
static void close_view(RZMacView *root) {
  if (!root || root.closed)
    return;
  stop_frame(root.clock);
  root.clock.view = nil;
  root.clock = nil;
  if (root.dataController) {
    RZMacDataController *data = root.dataController;
    data.table.delegate = nil;
    data.table.dataSource = nil;
    data.table.target = nil;
    data.model = nil;
    data.pending = nil;
    data.token = 0;
    data.view = nil;
    root.dataController = nil;
  }
  [[NSNotificationCenter defaultCenter] removeObserver:root];
  for (RZMacHandle *handle in handles.allValues)
    if (!handle.closed && handle.kind == RZ_VIEW && ((RZMacView *)handle).parent == root)
      close_view((RZMacView *)handle);
  if ([root.view respondsToSelector:@selector(setControl:)])
    ((RZMacNativeView *)root.view).control = nil;
  if ([root.view isKindOfClass:[NSTextView class]])
    ((NSTextView *)root.view).delegate = nil;
  if ([root.view isKindOfClass:[NSTextField class]])
    ((NSTextField *)root.view).delegate = nil;
  if ([root.view isKindOfClass:[NSControl class]])
    ((NSControl *)root.view).target = nil;
  if (root.fieldEditor) {
    root.fieldEditor.delegate = nil;
    [(id)root.fieldEditor setControl:nil];
    root.fieldEditor = nil;
  }
  [root.view removeFromSuperview];
  root.snapshotData = nil;
  root.view = nil;
  root.device = nil;
  root.parent = nil;
  root.rootWindow = nil;
  root.scopeWindow = nil;
  retire(root);
}
int32_t rinzu_macos_close(void *raw) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      if (session.running || session.eventWaitToken || session.runWaitToken ||
          session.clipboardToken)
        return fail(RZ_BUSY, @"stop run and join event readers before closing Session");
      for (RZMacHandle *handle in handles.allValues)
        if (handle.owner == session && handle.kind == RZ_WINDOW && ((RZMacWindow *)handle).sheet)
          return fail(RZ_BUSY, @"join sheet results before Session close");
      for (RZMacHandle *handle in handles.allValues)
        if (handle.owner == session && handle.kind == RZ_VIEW &&
            ((RZMacView *)handle).snapshotToken)
          return fail(RZ_BUSY, @"join text snapshots before Session close");
      stop_frames(session, nil);
      for (RZMacHandle *handle in handles.allValues)
        if (handle.owner == session && handle.kind == RZ_FRAME_CLOCK &&
            ((RZMacFrameClock *)handle).token)
          return fail(RZ_BUSY, @"join frame readers before Session close");
      for (RZMacHandle *handle in handles.allValues)
        if (!handle.closed && handle.owner == session && handle.kind == RZ_COMMAND &&
            ((RZMacCommand *)handle).state != 0)
          return fail(RZ_BUSY, @"RZ_GPU command must be physically settled before Session close");
      for (RZMacHandle *handle in handles.allValues) {
        if (handle.closed || handle.owner != session || handle.kind != RZ_WINDOW)
          continue;
        int32_t status = rinzu_macos_window_destroy(reference(handle));
        if (status)
          return status;
      }
      for (RZMacHandle *handle in handles.allValues) {
        if (handle.closed || handle.owner != session)
          continue;
        switch (handle.kind) {
        case RZ_MENU:
          close_menu((RZMacMenu *)handle);
          break;
        case RZ_VIEW:
          close_view((RZMacView *)handle);
          break;
        case RZ_BUFFER:
          ((RZMacBuffer *)handle).buffer = nil;
          ((RZMacBuffer *)handle).device = nil;
          break;
        case RZ_TEXTURE:
          ((RZMacTexture *)handle).texture = nil;
          ((RZMacTexture *)handle).device = nil;
          ((RZMacTexture *)handle).readback = nil;
          break;
        case RZ_SAMPLER:
          ((RZMacSampler *)handle).sampler = nil;
          ((RZMacSampler *)handle).device = nil;
          break;
        case RZ_FONT: {
          RZMacFont *font = (RZMacFont *)handle;
          if (font->font)
            CFRelease(font->font);
          font->font = NULL;
          break;
        }
        case RZ_TEXT: {
          RZMacText *text = (RZMacText *)handle;
          if (text->frame)
            CFRelease(text->frame);
          text->frame = NULL;
          text.attributed = nil;
          text.font = nil;
          break;
        }
        case RZ_FRAME_CLOCK:
          stop_frame((RZMacFrameClock *)handle);
          ((RZMacFrameClock *)handle).view.clock = nil;
          ((RZMacFrameClock *)handle).view = nil;
          break;
        case RZ_LIBRARY:
          ((RZMacLibrary *)handle).library = nil;
          ((RZMacLibrary *)handle).device = nil;
          break;
        case RZ_COMPUTE:
        case RZ_RENDER: {
          RZMacPipeline *pipeline = (RZMacPipeline *)handle;
          pipeline.compute = nil;
          pipeline.render = nil;
          pipeline.device = nil;
          break;
        }
        case RZ_COMMAND:
          ((RZMacCommand *)handle).device = nil;
          ((RZMacCommand *)handle).pins = nil;
          ((RZMacCommand *)handle).failure = nil;
          break;
        case RZ_DEVICE:
          ((RZMacDevice *)handle).queue = nil;
          ((RZMacDevice *)handle).device = nil;
          break;
        case RZ_SESSION:
          [session clearEvents];
          session.payloads = nil;
          session.clipboardData = nil;
          session.resourceRootData = nil;
          session->resourceRootSnapshot = (rinzu_native_byte_array_t){0};
          session.pasteboard = nil;
          break;
        default:
          _Exit(86);
        }
        if (handle != session)
          retire(handle);
      }
      retire(session);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_prepare_close(void *raw) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (session.running || session.eventWaitToken || session.runWaitToken || session.clipboardToken)
      return fail(RZ_BUSY, @"stop run and join event readers before closing Session");
    stop_frames(session, nil);
    for (RZMacHandle *handle in handles.allValues)
      if (handle.owner == session && handle.kind == RZ_FRAME_CLOCK &&
          ((RZMacFrameClock *)handle).token)
        return fail(RZ_BUSY, @"join frame readers before Session close");
    for (RZMacHandle *handle in handles.allValues)
      if (handle.owner == session && handle.kind == RZ_WINDOW && ((RZMacWindow *)handle).sheet)
        return fail(RZ_BUSY, @"join sheet results before Session close");
    for (RZMacHandle *handle in handles.allValues)
      if (!handle.closed && handle.owner == session && handle.kind == RZ_COMMAND &&
          ((RZMacCommand *)handle).waiterToken)
        return fail(RZ_BUSY, @"join command waiters before closing Session");
    session.closing = YES;
    return 0;
  }
}

static void stop_after_sheets(RZMacSession *session) {
  if (!session.running || !session.stopping)
    return;
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_WINDOW && handle.owner == session && ((RZMacWindow *)handle).sheet &&
        !((RZMacWindow *)handle).sheet.finished)
      return;
  [NSApp stop:nil];
  wake_application();
}
int32_t rinzu_macos_stop(void *raw) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      if (!session.running)
        return fail(RZ_BUSY, @"Session is not running");
      session.stopping = YES;
      stop_frames(session, nil);
      complete_event_wait(session);
      for (RZMacHandle *handle in handles.allValues)
        if (handle.kind == RZ_WINDOW && handle.owner == session)
          cancel_sheet(((RZMacWindow *)handle).sheet);
      stop_after_sheets(session);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
static int32_t run_session(void *raw, void *context,
                           void (*handler)(void *, const struct MacEvent *), BOOL stream) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      if (stream && session.streamCancelled)
        return 0;
      if ((stream && session.hasRun) || (!stream && session.streamPrepared))
        return fail(RZ_BUSY, @"event-stream Session cannot restart or switch loop modes");
      if (!handler && !stream)
        return fail(RZ_ARGUMENT, @"missing event handler");
      if (session.running || session.closing || session.eventWaitToken || session.runWaitToken)
        return fail(RZ_BUSY, @"Session is running or closing");
      BOOL has_window = NO;
      @autoreleasepool {
        for (RZMacHandle *handle in handles.allValues)
          if (!handle.closed && handle.owner == session && handle.kind == RZ_WINDOW)
            has_window = YES;
      }
      if (!has_window)
        return fail(RZ_ARGUMENT, @"run requires a live Window");
      NSApplication *application = [NSApplication sharedApplication];
      if (application.running)
        return fail(RZ_BUSY, @"NSApplication is already running");
      session.previousDelegate = application.delegate;
      session.previousMenu = application.mainMenu;
      rinzu_native_completion_v2_init(&session->runCompletion);
      session.running = YES;
      session.hasRun = YES;
      session.stopping = NO;
      session.overflow = NO;
      [session clearEvents];
      session.streaming = stream;
      session->handler = handler;
      session->handlerContext = context;
      CFRunLoopObserverRef eventObserver = NULL;
      int32_t result = 0;
      @try {
        {
          CFRunLoopObserverContext context = {.info = (__bridge void *)session};
          eventObserver =
              CFRunLoopObserverCreate(NULL, kCFRunLoopBeforeWaiting | kCFRunLoopAfterWaiting, true,
                                      0, event_wait_observer, &context);
          if (!eventObserver)
            _Exit(70);
          CFRunLoopAddObserver(CFRunLoopGetCurrent(), eventObserver, kCFRunLoopCommonModes);
        }
        application.delegate = session;
        if (!application.mainMenu) {
          NSMenu *menu = [NSMenu new];
          NSMenuItem *item = [NSMenuItem new];
          item.submenu = [NSMenu new];
          NSMenuItem *quit = [[NSMenuItem alloc] initWithTitle:@"Quit"
                                                        action:@selector(terminate:)
                                                 keyEquivalent:@"q"];
          if (!menu || !item || !item.submenu || !quit)
            _Exit(70);
          quit.target = application;
          [item.submenu addItem:quit];
          [menu addItem:item];
          application.mainMenu = menu;
        }
        session.fallbackMenu = application.mainMenu;
        refresh_menu(session);
        [session performSelector:@selector(started) withObject:nil afterDelay:0];
        [application run];
      } @finally {
        session->handler = NULL;
        session->handlerContext = NULL;
        session.streaming = NO;
        // AppKit can also return/throw without our stop entry point. Keep the
        // native carriers pinned and physically dismiss their sheets before
        // removing the loop observer or publishing run completion.
        session.stopping = YES;
        drain_sheets(session);
        if (eventObserver) {
          CFRunLoopObserverInvalidate(eventObserver);
          CFRelease(eventObserver);
        }
        complete_event_wait(session);
        [NSObject cancelPreviousPerformRequestsWithTarget:session];
        session.running = NO;
        stop_frames(session, nil);
        application.delegate = session.previousDelegate;
        // AppKit may synthesize a menu when set to nil. Keep the native-only
        // fallback instead of letting an installed Window menu survive its scope.
        application.mainMenu = session.previousMenu ?: session.fallbackMenu;
        session.previousDelegate = nil;
        session.previousMenu = nil;
        session.fallbackMenu = nil;
        session.stopping = NO;
        session.scheduled = NO;
        [session clearEvents];
        result = session.overflow ? fail(RZ_OVERFLOW, @"macOS event queue overflow") : 0;
        // LAST Session access; awaiting stop may now release its registry entry.
        rinzu_native_completion_v2_publish(&session->runCompletion);
      }
      return result;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_run(void *raw, void *context,
                        void (*handler)(void *, const struct MacEvent *)) {
  return run_session(raw, context, handler, NO);
}
int32_t rinzu_macos_stream_state(void *raw) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session ? (session.running ? 1 : session.hasRun ? 2 : 0) : missing();
  }
}
int32_t rinzu_macos_prepare_events(void *raw) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (session.streamPrepared || (session.hasRun && !session.streamCancelled) || session.running ||
        session.closing || session.runWaitToken)
      return fail(RZ_BUSY, @"one event-stream run is allowed per Session");
    session.streamPrepared = YES;
    if (session.streamCancelled) {
      session.hasRun = YES;
    } else {
      rinzu_native_completion_v2_init(&session->runCompletion);
    }
    return 0;
  }
}
int32_t rinzu_macos_request_stop(void *raw) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (session.running)
      return rinzu_macos_stop(raw);
    stop_frames(session, nil);
    session.streamCancelled = YES;
    session.hasRun = YES;
    complete_event_wait(session);
    if (!rinzu_native_completion_is_done(&session->runCompletion.base))
      rinzu_native_completion_v2_publish(&session->runCompletion);
    return 0;
  }
}
_Bool rinzu_macos_has_run(void *raw) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session && session.hasRun;
  }
}
int32_t rinzu_macos_run_events(void *raw) {
  return run_session(raw, NULL, NULL, YES);
}
_Bool rinzu_macos_running(void *raw) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session && session.running;
  }
}
int32_t rinzu_macos_run_wait_begin(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (token <= 0 || token > next_waiter || session.runWaitToken)
      return fail(RZ_BUSY, @"run completion requires one valid waiter");
    session.runWaitToken = token;
    return 0;
  }
}
_Bool rinzu_macos_run_wait_owned(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session && token > 0 && session.runWaitToken == token;
  }
}
void *rinzu_macos_run_completion(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session && token > 0 && session.runWaitToken == token ? &session->runCompletion.base
                                                                 : NULL;
  }
}
int32_t rinzu_macos_run_wait_finish(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (token <= 0 || session.runWaitToken != token ||
        !rinzu_native_completion_is_done(&session->runCompletion.base))
      return fail(RZ_BUSY, @"run must physically return before releasing its waiter");
    session.runWaitToken = 0;
    return 0;
  }
}
int32_t rinzu_macos_event_wait_begin(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if ((!session.running && !session.hasRun) || (session.running && !session.streaming) ||
        session.eventWaitToken || token <= 0 || token > next_waiter)
      return fail(RZ_BUSY, @"event wait requires a running stream and one reader");
    rinzu_native_completion_v2_init(&session->eventCompletion);
    session.eventWaitToken = token;
    if (session->count || session.stopping || !session.running)
      complete_event_wait(session);
    return 0;
  }
}
_Bool rinzu_macos_event_wait_owned(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session && token > 0 && session.eventWaitToken == token;
  }
}
void *rinzu_macos_event_completion(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session && token > 0 && session.eventWaitToken == token ? &session->eventCompletion.base
                                                                   : NULL;
  }
}
int32_t rinzu_macos_event_take(void *raw, int64_t token, struct MacEvent *output) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      if (!output)
        return fail(RZ_ARGUMENT, @"missing event output");
      if (session.eventWaitToken != token || token <= 0 || session.eventSelected ||
          !rinzu_native_completion_is_done(&session->eventCompletion.base))
        return fail(RZ_BUSY, @"event snapshot requires a completed reader");
      *output = (struct MacEvent){.kind = RZ_STOPPED};
      if (!session.stopping && session.running) {
        *output = (struct MacEvent){0};
        while (session->count) {
          struct MacEvent event = session->events[session->head];
          session->head = (session->head + 1) % RZ_EVENT_LIMIT;
          --session->count;
          NSData *text = session.payloads[@(event.sequence)];
          [session.payloads removeObjectForKey:@(event.sequence)];
          if (!event_live(event)) {
            session.payloadBytes -= (int64_t)text.length;
            continue;
          }
          *output = event;
          session.selectedText = text;
          break;
        }
      }
      session->selectedEvent = *output;
      session.eventSelected = YES;
      return output->kind ? 1 : 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
const rinzu_native_byte_array_t *rinzu_macos_event_bytes(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session || token <= 0 || session.eventWaitToken != token || !session.eventSelected)
      return NULL;
    session->eventSnapshot = (rinzu_native_byte_array_t){
        .data = (uint8_t *)session.selectedText.bytes, .length = session.selectedText.length};
    return &session->eventSnapshot;
  }
}
int32_t rinzu_macos_event_cancel(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (session.eventWaitToken != token || token <= 0)
      return fail(RZ_BUSY, @"event reader identity mismatch");
    complete_event_wait(session);
    return 0;
  }
}
int32_t rinzu_macos_event_finish(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (session.eventWaitToken != token || token <= 0 ||
        !rinzu_native_completion_is_done(&session->eventCompletion.base))
      return fail(RZ_BUSY, @"event reader must physically complete before finish");
    session.payloadBytes -= (int64_t)session.selectedText.length;
    session.selectedText = nil;
    session.eventSelected = NO;
    session.eventWaitToken = 0;
    session->eventSnapshot = (rinzu_native_byte_array_t){0};
    return 0;
  }
}

static int64_t window_create(void *raw, int64_t token, const uint8_t *title, size_t length,
                             double width, double height) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      if (!accepting(session))
        return fail(RZ_BUSY, @"Session is closing");
      struct MacRect rect = {0, 0, width, height};
      NSString *name = text(title, length);
      if (!name || !rect_valid(&rect) || width == 0 || height == 0)
        return fail(RZ_ARGUMENT, @"invalid window title or content size");
      NSApplication *application = [NSApplication sharedApplication];
      [application setActivationPolicy:NSApplicationActivationPolicyRegular];
      RZMacWindow *window = [RZMacWindow new];
      window.scopeToken = token;
      int64_t identity = publish(window, RZ_WINDOW, session);
      window.window = [[RZMacNativeWindow alloc]
          initWithContentRect:nsrect(&rect)
                    styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                              NSWindowStyleMaskResizable | NSWindowStyleMaskMiniaturizable
                      backing:NSBackingStoreBuffered
                        defer:NO];
      if (!window.window)
        _Exit(70);
      ((RZMacNativeWindow *)window.window).control = window;
      RZMacView *view = [RZMacView new];
      view.enabled = YES;
      view.view = [[RZMacNativeView alloc] initWithFrame:nsrect(&rect)];
      if (!view.view)
        _Exit(70);
      ((RZMacNativeView *)view.view).control = view;
      view.rootWindow = window;
      view.scopeWindow = window;
      window.content = view;
      publish(view, RZ_VIEW, session);
      window.window.releasedWhenClosed = NO;
      window.window.title = name;
      window.window.contentView = view.view;
      window.window.delegate = window;
      window.window.acceptsMouseMovedEvents = YES;
      [window.window center];
      return identity;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int64_t rinzu_macos_window_create(void *raw, const uint8_t *title, size_t length, double width,
                                  double height) {
  return window_create(raw, 0, title, length, width, height);
}
int64_t rinzu_macos_window_scoped(void *raw, int64_t token, const uint8_t *title, size_t length,
                                  double width, double height) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (token <= 0 || token > next_waiter)
      return fail(RZ_ARGUMENT, @"invalid window scope identity");
    for (RZMacHandle *handle in handles.allValues)
      if (handle.kind == RZ_WINDOW && handle.owner == session &&
          ((RZMacWindow *)handle).scopeToken == token)
        return fail(RZ_BUSY, @"window scope is already active");
    return window_create(raw, token, title, length, width, height);
  }
}
int32_t rinzu_macos_window_scope_close(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (token <= 0 || token > next_waiter)
      return fail(RZ_ARGUMENT, @"invalid window scope identity");
    for (RZMacHandle *handle in handles.allValues)
      if (handle.kind == RZ_WINDOW && handle.owner == session &&
          ((RZMacWindow *)handle).scopeToken == token)
        return rinzu_macos_window_destroy(reference(handle));
    return 0;
  }
}
int32_t rinzu_macos_scope_notify(void *raw, int64_t window_id) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      RZMacWindow *window = (RZMacWindow *)lookup((void *)(uintptr_t)window_id, RZ_WINDOW);
      if (window && window.owner == session)
        [session enqueue:(struct MacEvent){.kind = RZ_TASK_FINISHED, .windowId = window_id}];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int64_t rinzu_macos_event_sequence(void *raw) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session ? session->sequence : missing();
  }
}
void *rinzu_macos_window(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_WINDOW);
  }
}
int64_t rinzu_macos_window_id(void *raw) {
  @autoreleasepool {
    RZMacHandle *handle = lookup(raw, RZ_WINDOW);
    return handle ? handle.identity : missing();
  }
}
int32_t rinzu_macos_window_show(void *raw) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      if (!window)
        return missing();
      [window.window makeKeyAndOrderFront:nil];
      [NSApp activateIgnoringOtherApps:YES];
      window.owner.activeWindow = window;
      refresh_menu(window.owner);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_window_title(void *raw, const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      if (!window)
        return missing();
      NSString *title = text(bytes, length);
      if (!title)
        return fail(RZ_ARGUMENT, @"invalid UTF-8 title");
      window.window.title = title;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_window_size(void *raw, double width, double height) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      if (!window)
        return missing();
      struct MacRect rect = {0, 0, width, height};
      if (!rect_valid(&rect) || width == 0 || height == 0)
        return fail(RZ_ARGUMENT, @"invalid content size");
      if (NSEqualSizes(window.window.contentView.bounds.size, NSMakeSize(width, height)))
        return 0;
      if (view_busy(window.content))
        return fail(RZ_BUSY, @"window has a leased Metal surface");
      [window.window setContentSize:NSMakeSize(width, height)];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_window_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      if (!window)
        return missing();
      if (window.sheet) {
        window.sheetsStopping = YES;
        cancel_sheet(window.sheet);
        return fail(RZ_BUSY, @"join sheet result before Window destruction");
      }
      for (RZMacHandle *handle in handles.allValues)
        if (handle.kind == RZ_VIEW && ((RZMacView *)handle).scopeWindow == window && handle.leases)
          return fail(RZ_BUSY, @"window scope has a leased Metal surface");
      window.window.delegate = nil;
      for (RZMacHandle *handle in handles.allValues)
        if (handle.kind == RZ_MENU && ((RZMacMenu *)handle).scopeWindow == window)
          close_menu((RZMacMenu *)handle);
      close_view(window.content);
      for (RZMacHandle *handle in handles.allValues)
        if (handle.kind == RZ_VIEW && ((RZMacView *)handle).scopeWindow == window)
          close_view((RZMacView *)handle);
      [window.window orderOut:nil];
      [window.window close];
      window.content = nil;
      window.window = nil;
      retire(window);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_window_content(void *raw) {
  @autoreleasepool {
    RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
    return window ? reference(window.content) : NULL;
  }
}
static int64_t create_view(RZMacSession *session, const struct MacRect *rect, RZMacDevice *device) {
  if (!accepting(session))
    return fail(RZ_BUSY, @"Session is closing");
  if (!rect_valid(rect))
    return fail(RZ_ARGUMENT, @"invalid view frame");
  RZMacView *view = [RZMacView new];
  view.enabled = YES;
  view.view = [[RZMacNativeView alloc] initWithFrame:nsrect(rect)];
  if (!view.view)
    _Exit(70);
  view.device = device;
  ((RZMacNativeView *)view.view).control = view;
  int64_t identity = publish(view, RZ_VIEW, session);
  if (device) {
    CAMetalLayer *layer = [CAMetalLayer layer];
    if (!layer)
      _Exit(70);
    layer.device = device.device;
    layer.pixelFormat = MTLPixelFormatBGRA8Unorm;
    layer.framebufferOnly = YES;
    view.view.wantsLayer = YES;
    view.view.layer = layer;
    resized(view);
  }
  return identity;
}
int64_t rinzu_macos_view_create(void *raw, const struct MacRect *rect) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      return session ? create_view(session, rect, nil) : missing();
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_view(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_VIEW);
  }
}
int64_t rinzu_macos_view_id(void *raw) {
  @autoreleasepool {
    RZMacHandle *handle = lookup(raw, RZ_VIEW);
    return handle ? handle.identity : missing();
  }
}
int32_t rinzu_macos_view_frame(void *raw, struct MacRect *rect) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (!rect)
        return fail(RZ_ARGUMENT, @"missing frame output");
      NSRect frame = view.view.frame;
      *rect = (struct MacRect){frame.origin.x, frame.origin.y, frame.size.width, frame.size.height};
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_view_set_frame(void *raw, const struct MacRect *rect) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (!rect_valid(rect) || view.rootWindow)
        return fail(RZ_ARGUMENT, @"invalid frame or root view");
      if (NSEqualRects(view.view.frame, nsrect(rect)))
        return 0;
      if (view_busy(view))
        return fail(RZ_BUSY, @"view has a leased Metal surface");
      view.view.frame = nsrect(rect);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_view_add(void *parent_raw, void *child_raw) {
  @autoreleasepool {
    @try {
      RZMacView *parent = (RZMacView *)lookup(parent_raw, RZ_VIEW);
      RZMacView *child = (RZMacView *)lookup(child_raw, RZ_VIEW);
      if (!parent || !child)
        return missing();
      if (!accepting(parent) || !accepting(child))
        return fail(RZ_BUSY, @"Window scope is closing");
      if (parent.owner != child.owner || child.rootWindow || descendant(parent, child))
        return fail(RZ_ARGUMENT, @"view cycle, root reparent or different Session");
      if (child.parent == parent)
        return 0;
      if (view_busy(child))
        return fail(RZ_BUSY, @"view has a leased Metal surface");
      child.parent = parent;
      RZMacWindow *scope = parent.scopeWindow ?: view_window(parent);
      if (scope)
        for (RZMacHandle *handle in handles.allValues)
          if (handle.kind == RZ_VIEW && descendant((RZMacView *)handle, child))
            ((RZMacView *)handle).scopeWindow = scope;
      [parent.view addSubview:child.view];
      resized(child);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_view_remove(void *raw) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (view.rootWindow)
        return fail(RZ_ARGUMENT, @"cannot remove root view");
      if (view_busy(view))
        return fail(RZ_BUSY, @"view has a leased Metal surface");
      [view.view removeFromSuperview];
      view.parent = nil;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_view_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (view.rootWindow)
        return fail(RZ_ARGUMENT, @"destroy Window to close its content view");
      if (view_busy(view))
        return fail(RZ_BUSY, @"view has a leased Metal surface");
      close_view(view);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_view_request_draw(void *raw) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      [view.owner enqueue:view_event(view, RZ_DRAW)];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}

_Bool rinzu_macos_metal_available(void) {
  @autoreleasepool {
    @try {
      return [NSThread isMainThread] && MTLCreateSystemDefaultDevice() != nil;
    } @catch (NSException *exception) {
      (void)exception;
      return 0;
    }
  }
}
int64_t rinzu_macos_device_create(void *raw) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      if (!accepting(session))
        return fail(RZ_BUSY, @"Session is closing");
      id<MTLDevice> metal = MTLCreateSystemDefaultDevice();
      if (!metal)
        return fail(RZ_NO_METAL, @"no default Metal device is available");
      RZMacDevice *device = [RZMacDevice new];
      device.device = metal;
      device.queue = [metal newCommandQueue];
      if (!device.queue)
        _Exit(70);
      return publish(device, RZ_DEVICE, session);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_device(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_DEVICE);
  }
}
static int64_t create_buffer(RZMacDevice *device, const uint8_t *bytes, size_t length, BOOL copy) {
  if (!accepting(device))
    return fail(RZ_BUSY, @"Session is closing");
  if (length > RZ_BUFFER_LIMIT || length > device.device.maxBufferLength ||
      (copy && length && !bytes))
    return fail(RZ_ARGUMENT, @"buffer size or input is invalid (maximum 256 MiB)");
  RZMacBuffer *buffer = [RZMacBuffer new];
  buffer.device = device;
  buffer.length = length;
  buffer.buffer = [device.device newBufferWithLength:MAX(length, (size_t)1)
                                             options:MTLResourceStorageModeShared];
  if (!buffer.buffer)
    _Exit(70);
  if (copy && length)
    memcpy(buffer.buffer.contents, bytes, length);
  return publish(buffer, RZ_BUFFER, device.owner);
}
int64_t rinzu_macos_buffer_create(void *raw, const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(raw, RZ_DEVICE);
      return device ? create_buffer(device, bytes, length, YES) : missing();
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int64_t rinzu_macos_buffer_allocate(void *raw, int64_t length) {
  @autoreleasepool {
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(raw, RZ_DEVICE);
      if (!device)
        return missing();
      if (length < 0)
        return fail(RZ_ARGUMENT, @"negative buffer size");
      return create_buffer(device, NULL, (size_t)length, NO);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_buffer(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_BUFFER);
  }
}
static BOOL range_valid(NSUInteger capacity, int64_t offset, uint64_t length) {
  return offset >= 0 && (uint64_t)offset <= capacity && length <= capacity - (uint64_t)offset;
}
int32_t rinzu_macos_buffer_write(void *raw, int64_t offset, const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacBuffer *buffer = (RZMacBuffer *)lookup(raw, RZ_BUFFER);
      if (!buffer)
        return missing();
      if (buffer.leases)
        return fail(RZ_BUSY, @"buffer is leased by a command");
      if (!range_valid(buffer.length, offset, length) || (length && !bytes))
        return fail(RZ_ARGUMENT, @"buffer write range is invalid");
      if (length)
        memcpy((uint8_t *)buffer.buffer.contents + offset, bytes, length);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_buffer_read(void *raw, int64_t offset, int64_t length) {
  @autoreleasepool {
    @try {
      RZMacBuffer *buffer = (RZMacBuffer *)lookup(raw, RZ_BUFFER);
      if (!buffer)
        return missing();
      if (buffer.leases)
        return fail(RZ_BUSY, @"buffer is leased by a command");
      if (length < 0 || !range_valid(buffer.length, offset, (uint64_t)length))
        return fail(RZ_ARGUMENT, @"buffer read range is invalid");
      buffer->snapshot.data = (uint8_t *)buffer.buffer.contents + offset;
      buffer->snapshot.length = (size_t)length;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
const rinzu_native_byte_array_t *rinzu_macos_buffer_bytes(void *raw) {
  @autoreleasepool {
    RZMacBuffer *buffer = (RZMacBuffer *)lookup(raw, RZ_BUFFER);
    return buffer && !buffer.leases ? &buffer->snapshot : NULL;
  }
}
int32_t rinzu_macos_buffer_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacBuffer *buffer = (RZMacBuffer *)lookup(raw, RZ_BUFFER);
      if (!buffer)
        return missing();
      if (buffer.leases)
        return fail(RZ_BUSY, @"buffer is leased by a command");
      buffer.buffer = nil;
      buffer.device = nil;
      buffer->snapshot = (rinzu_native_byte_array_t){0};
      retire(buffer);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int64_t rinzu_macos_library_create(void *raw, const uint8_t *bytes, size_t length, _Bool file) {
  @autoreleasepool {
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(raw, RZ_DEVICE);
      if (!device)
        return missing();
      if (!accepting(device))
        return fail(RZ_BUSY, @"Session is closing");
      NSString *source = text(bytes, length);
      if (!source || !length)
        return fail(RZ_ARGUMENT, @"invalid shader source or library path");
      NSError *error = nil;
      id<MTLLibrary> metal =
          file ? [device.device newLibraryWithURL:[NSURL fileURLWithPath:source] error:&error]
               : [device.device newLibraryWithSource:source options:nil error:&error];
      if (!metal)
        return fail(RZ_SHADER, error.localizedDescription ?: @"Metal library creation failed");
      RZMacLibrary *library = [RZMacLibrary new];
      library.device = device;
      library.library = metal;
      return publish(library, RZ_LIBRARY, device.owner);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_library(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_LIBRARY);
  }
}
int64_t rinzu_macos_compute_pipeline(void *raw, const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacLibrary *library = (RZMacLibrary *)lookup(raw, RZ_LIBRARY);
      if (!library)
        return missing();
      if (!accepting(library))
        return fail(RZ_BUSY, @"Session is closing");
      NSString *name = text(bytes, length);
      if (!name || !length)
        return fail(RZ_ARGUMENT, @"invalid function name");
      id<MTLFunction> function = [library.library newFunctionWithName:name];
      if (!function)
        return fail(RZ_SHADER, @"compute function was not found");
      NSError *error = nil;
      id<MTLComputePipelineState> metal =
          [library.device.device newComputePipelineStateWithFunction:function error:&error];
      if (!metal)
        return fail(RZ_SHADER, error.localizedDescription ?: @"compute pipeline creation failed");
      RZMacPipeline *pipeline = [RZMacPipeline new];
      pipeline.device = library.device;
      pipeline.compute = metal;
      return publish(pipeline, RZ_COMPUTE, library.owner);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_compute(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_COMPUTE);
  }
}
int64_t rinzu_macos_render_pipeline(void *raw, const uint8_t *vertex, size_t vertex_length,
                                    const uint8_t *fragment, size_t fragment_length) {
  return rinzu_macos_render_pipeline_options(raw, vertex, vertex_length, fragment, fragment_length,
                                             2, 0);
}
int64_t rinzu_macos_render_pipeline_options(void *raw, const uint8_t *vertex, size_t vertex_length,
                                            const uint8_t *fragment, size_t fragment_length,
                                            int64_t format, int64_t blend) {
  @autoreleasepool {
    @try {
      RZMacLibrary *library = (RZMacLibrary *)lookup(raw, RZ_LIBRARY);
      if (!library)
        return missing();
      if (!accepting(library))
        return fail(RZ_BUSY, @"Session is closing");
      if ((format != 1 && format != 2) || blend < 0 || blend > 2)
        return fail(RZ_ARGUMENT, @"invalid pipeline pixel format or blend mode");
      NSString *vertex_name = text(vertex, vertex_length),
               *fragment_name = text(fragment, fragment_length);
      if (!vertex_name || !fragment_name || !vertex_length || !fragment_length)
        return fail(RZ_ARGUMENT, @"invalid vertex or fragment function name");
      MTLRenderPipelineDescriptor *descriptor = [MTLRenderPipelineDescriptor new];
      if (!descriptor)
        _Exit(70);
      descriptor.vertexFunction = [library.library newFunctionWithName:vertex_name];
      descriptor.fragmentFunction = [library.library newFunctionWithName:fragment_name];
      if (!descriptor.vertexFunction || !descriptor.fragmentFunction)
        return fail(RZ_SHADER, @"render function was not found");
      MTLRenderPipelineColorAttachmentDescriptor *color = descriptor.colorAttachments[0];
      color.pixelFormat = format == 1 ? MTLPixelFormatRGBA8Unorm : MTLPixelFormatBGRA8Unorm;
      if (blend) {
        color.blendingEnabled = YES;
        color.sourceRGBBlendFactor = blend == 1 ? MTLBlendFactorOne : MTLBlendFactorSourceAlpha;
        color.destinationRGBBlendFactor = MTLBlendFactorOneMinusSourceAlpha;
        color.sourceAlphaBlendFactor = MTLBlendFactorOne;
        color.destinationAlphaBlendFactor = MTLBlendFactorOneMinusSourceAlpha;
      }
      NSError *error = nil;
      id<MTLRenderPipelineState> metal =
          [library.device.device newRenderPipelineStateWithDescriptor:descriptor error:&error];
      if (!metal)
        return fail(RZ_SHADER, error.localizedDescription ?: @"render pipeline creation failed");
      RZMacPipeline *pipeline = [RZMacPipeline new];
      pipeline.device = library.device;
      pipeline.render = metal;
      pipeline.format = format;
      return publish(pipeline, RZ_RENDER, library.owner);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_render(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_RENDER);
  }
}
int64_t rinzu_macos_surface_create(void *raw, const struct MacRect *rect) {
  @autoreleasepool {
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(raw, RZ_DEVICE);
      return device ? create_view(device.owner, rect, device) : missing();
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}

static void pin(RZMacCommand *command, RZMacHandle *handle) {
  if ([command.pins containsObject:handle])
    return;
  if (handle.leases == NSUIntegerMax)
    _Exit(70);
  ++handle.leases;
  [command.pins addObject:handle];
}
static void release_command(RZMacCommand *command) {
  for (RZMacHandle *handle in command.pins) {
    if (!handle.leases)
      _Exit(86);
    --handle.leases;
  }
  [command.pins removeAllObjects];
  command.compute = nil;
  command.render = nil;
  command.command = nil;
  command.drawable = nil;
  command.pipeline = nil;
  command.target = nil;
  command.targetWidth = command.targetHeight = 0;
  command.targetFormat = 0;
  command.failure = nil;
  command.state = 0;
}
static int32_t begin_command(RZMacCommand *command) {
  if (!accepting(command))
    return fail(RZ_BUSY, @"Session is closing");
  if (command.state || command.waiterToken)
    return fail(RZ_BUSY, @"command is not idle; wait or abandon first");
  command.command = [command.device.queue commandBuffer];
  if (!command.command)
    _Exit(70);
  command.state = 1;
  command->result = 0;
  command.failure = nil;
  rinzu_native_completion_v2_init(&command->completion);
  return 0;
}
int64_t rinzu_macos_command_create(void *raw) {
  @autoreleasepool {
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(raw, RZ_DEVICE);
      if (!device)
        return missing();
      if (!accepting(device))
        return fail(RZ_BUSY, @"Session is closing");
      RZMacCommand *command = [RZMacCommand new];
      if (!command)
        _Exit(70);
      atomic_init(&command->submitted, 0);
      command.device = device;
      command.pins = [NSMutableArray new];
      if (!command.pins)
        _Exit(70);
      return publish(command, RZ_COMMAND, device.owner);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_command(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_COMMAND);
  }
}
int32_t rinzu_macos_command_compute(void *raw, void *pipeline_raw) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      RZMacPipeline *pipeline = (RZMacPipeline *)lookup(pipeline_raw, RZ_COMPUTE);
      if (!command || !pipeline)
        return missing();
      if (command.device != pipeline.device)
        return fail(RZ_ARGUMENT, @"pipeline belongs to another Device");
      int32_t status = begin_command(command);
      if (status)
        return status;
      command.compute = [command.command computeCommandEncoder];
      if (!command.compute)
        _Exit(70);
      command.pipeline = pipeline;
      [command.compute setComputePipelineState:pipeline.compute];
      pin(command, pipeline);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_render(void *raw, void *view_raw, const struct MacColor *clear) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      RZMacView *view = (RZMacView *)lookup(view_raw, RZ_VIEW);
      if (!command || !view)
        return missing();
      if (!accepting(view))
        return fail(RZ_BUSY, @"surface Window is closing");
      if (view.device != command.device || !clear || !isfinite(clear->red) ||
          !isfinite(clear->green) || !isfinite(clear->blue) || !isfinite(clear->alpha))
        return fail(RZ_ARGUMENT, @"invalid surface, Device or clear color");
      if (view.leases)
        return fail(RZ_BUSY, @"surface already has an unfinished command");
      if (!view.view.window || !view.view.window.visible || view.view.hidden ||
          view.view.bounds.size.width <= 0 || view.view.bounds.size.height <= 0)
        return fail(RZ_NO_DRAWABLE, @"surface is not visible");
      if (command.state || command.waiterToken)
        return fail(RZ_BUSY, @"command is not idle");
      CAMetalLayer *layer = (CAMetalLayer *)view.view.layer;
      id<CAMetalDrawable> drawable = [layer nextDrawable];
      if (!drawable)
        return fail(RZ_NO_DRAWABLE, @"Metal drawable is temporarily unavailable");
      int32_t status = begin_command(command);
      if (status)
        return status;
      command.drawable = drawable;
      command.targetWidth = drawable.texture.width;
      command.targetHeight = drawable.texture.height;
      command.targetFormat = 2;
      MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
      if (!pass)
        _Exit(70);
      pass.colorAttachments[0].texture = drawable.texture;
      pass.colorAttachments[0].loadAction = MTLLoadActionClear;
      pass.colorAttachments[0].storeAction = MTLStoreActionStore;
      pass.colorAttachments[0].clearColor =
          MTLClearColorMake(clear->red, clear->green, clear->blue, clear->alpha);
      command.render = [command.command renderCommandEncoderWithDescriptor:pass];
      if (!command.render)
        _Exit(70);
      pin(command, view);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_pipeline(void *raw, void *pipeline_raw) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      RZMacPipeline *pipeline = (RZMacPipeline *)lookup(pipeline_raw, RZ_RENDER);
      if (!command || !pipeline)
        return missing();
      if (command.state != 1 || !command.render)
        return fail(RZ_BUSY, @"command is not encoding a render pass");
      if (command.device != pipeline.device)
        return fail(RZ_ARGUMENT, @"pipeline belongs to another Device");
      if (pipeline.format != command.targetFormat)
        return fail(RZ_ARGUMENT, @"pipeline pixel format disagrees with the render target");
      [command.render setRenderPipelineState:pipeline.render];
      command.pipeline = pipeline;
      pin(command, pipeline);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_buffer(void *raw, void *buffer_raw, int64_t offset, int64_t index,
                                   int32_t stage) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      RZMacBuffer *buffer = (RZMacBuffer *)lookup(buffer_raw, RZ_BUFFER);
      if (!command || !buffer)
        return missing();
      if (command.state != 1)
        return fail(RZ_BUSY, @"command is not recording");
      if (buffer.device != command.device || !range_valid(buffer.length, offset, 0) || offset % 4 ||
          index < 0 || index > 30 || stage < 0 || stage > 2)
        return fail(RZ_ARGUMENT, @"invalid Device, aligned buffer offset, binding index or stage");
      if (buffer.leases && ![command.pins containsObject:buffer])
        return fail(RZ_BUSY, @"buffer is leased by another command");
      if ((stage == 0 && !command.compute) || (stage != 0 && !command.render))
        return fail(RZ_ARGUMENT, @"buffer stage disagrees with command encoder");
      if (stage == 0)
        [command.compute setBuffer:buffer.buffer
                            offset:(NSUInteger)offset
                           atIndex:(NSUInteger)index];
      else if (stage == 1)
        [command.render setVertexBuffer:buffer.buffer
                                 offset:(NSUInteger)offset
                                atIndex:(NSUInteger)index];
      else
        [command.render setFragmentBuffer:buffer.buffer
                                   offset:(NSUInteger)offset
                                  atIndex:(NSUInteger)index];
      pin(command, buffer);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_dispatch(void *raw, int64_t x, int64_t y, int64_t z, int64_t tx,
                                     int64_t ty, int64_t tz) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      if (!command)
        return missing();
      if (command.state != 1 || !command.compute)
        return fail(RZ_BUSY, @"command is not encoding compute");
      MTLSize max = command.device.device.maxThreadsPerThreadgroup;
      uint64_t limit = command.pipeline.compute.maxTotalThreadsPerThreadgroup;
      if (x <= 0 || y <= 0 || z <= 0 || tx <= 0 || ty <= 0 || tz <= 0 || (uint64_t)tx > max.width ||
          (uint64_t)ty > max.height || (uint64_t)tz > max.depth || (uint64_t)tx > limit ||
          (uint64_t)ty > limit / (uint64_t)tx ||
          (uint64_t)tz > limit / ((uint64_t)tx * (uint64_t)ty))
        return fail(RZ_ARGUMENT, @"invalid dispatch size or threadgroup limit");
      [command.compute dispatchThreads:MTLSizeMake((NSUInteger)x, (NSUInteger)y, (NSUInteger)z)
                 threadsPerThreadgroup:MTLSizeMake((NSUInteger)tx, (NSUInteger)ty, (NSUInteger)tz)];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_draw(void *raw, int64_t start, int64_t count) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      if (!command)
        return missing();
      if (command.state != 1 || !command.render || !command.pipeline)
        return fail(RZ_BUSY, @"render pipeline must be bound before drawing");
      if (start < 0 || count <= 0 || start > INT64_MAX - count)
        return fail(RZ_ARGUMENT, @"invalid vertex range");
      [command.render drawPrimitives:MTLPrimitiveTypeTriangle
                         vertexStart:(NSUInteger)start
                         vertexCount:(NSUInteger)count];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_commit(void *raw) {
  @autoreleasepool {
    RZMacCommand *command = nil;
    @try {
      command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      if (!command)
        return missing();
      if (command.state != 1)
        return fail(RZ_BUSY, @"command is not recording");
      [command.compute endEncoding];
      [command.render endEncoding];
      command.compute = nil;
      command.render = nil;
      if (command.drawable)
        [command.command presentDrawable:command.drawable];
      // The block retains the carrier through its last access. finish breaks
      // command -> MTLCommandBuffer -> block -> command after publication.
      RZMacCommand *carrier = command;
      if (command->generation == UINT64_MAX)
        _Exit(70);
      uint64_t generation = ++command->generation;
      [command.command addCompletedHandler:^(id<MTLCommandBuffer> completed) {
        @autoreleasepool {
          // Establish SDK-owned publication even when Metal's dispatch internals
          // are uninstrumented. The callback must belong to this submission.
          if (atomic_load_explicit(&carrier->submitted, memory_order_acquire) != generation)
            _Exit(86);
          carrier->result = completed.status == MTLCommandBufferStatusCompleted ? 0 : RZ_GPU;
          carrier.failure = completed.error.localizedDescription;
          // LAST carrier/payload access. No Main registry or managed entry here.
          rinzu_native_completion_v2_publish(&carrier->completion);
        }
      }];
      command.state = 2;
      atomic_store_explicit(&command->submitted, generation, memory_order_release);
      [command.command commit];
      return 0;
    } @catch (NSException *exception) {
      // If Metal rejected commit before submission, there will be no completion.
      // Submitted work keeps its pins and is drained through the normal path.
      if (command && command.state == 2 && command.command.status < MTLCommandBufferStatusCommitted)
        release_command(command);
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_abandon(void *raw) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      if (!command)
        return missing();
      if (command.state == 2 || command.waiterToken)
        return fail(RZ_BUSY, @"submitted command requires physical completion");
      [command.compute endEncoding];
      [command.render endEncoding];
      release_command(command);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int64_t rinzu_macos_wait_token(void) {
  @autoreleasepool {
    if (![NSThread isMainThread])
      return missing();
    if (next_waiter == INT64_MAX)
      _Exit(70);
    return ++next_waiter;
  }
}
int32_t rinzu_macos_command_wait_begin(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
    if (!command)
      return missing();
    if (command.state != 2 || command.waiterToken || token <= 0 || token > next_waiter)
      return fail(RZ_BUSY, @"wait requires a submitted command and one waiter");
    command.waiterToken = token;
    return 0;
  }
}
_Bool rinzu_macos_command_wait_owned(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
    return command && token > 0 && command.waiterToken == token;
  }
}
int32_t rinzu_macos_command_state(void *raw) {
  @autoreleasepool {
    RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
    return command ? command.state : missing();
  }
}
void *rinzu_macos_command_completion(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
    return command && command.state == 2 && token > 0 && command.waiterToken == token
               ? &command->completion.base
               : NULL;
  }
}
int32_t rinzu_macos_command_status(void *raw) {
  @autoreleasepool {
    RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
    if (!command)
      return missing();
    if (command.state != 2 || !rinzu_native_completion_is_done(&command->completion.base))
      return fail(RZ_BUSY, @"GPU command is not physically completed");
    return command->result ? fail(command->result, command.failure ?: @"GPU command failed") : 0;
  }
}
int32_t rinzu_macos_command_finish(void *raw, int64_t token) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      if (!command)
        return missing();
      if (command.state != 2 || token <= 0 || command.waiterToken != token ||
          !rinzu_native_completion_is_done(&command->completion.base))
        return fail(RZ_BUSY, @"finish requires a waiter and physical completion");
      release_command(command);
      command.waiterToken = 0;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
static RZMacCommand *pending_command(void *raw) {
  RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
  if (!session)
    return nil;
  for (RZMacHandle *handle in handles.allValues)
    if (!handle.closed && handle.owner == session && handle.kind == RZ_COMMAND &&
        ((RZMacCommand *)handle).state)
      return (RZMacCommand *)handle;
  return nil;
}
_Bool rinzu_macos_has_pending_command(void *raw) {
  @autoreleasepool {
    return pending_command(raw) != nil;
  }
}
void *rinzu_macos_pending_command(void *raw) {
  @autoreleasepool {
    return reference(pending_command(raw));
  }
}

int32_t rinzu_macos_library_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacLibrary *library = (RZMacLibrary *)lookup(raw, RZ_LIBRARY);
      if (!library)
        return missing();
      library.library = nil;
      library.device = nil;
      retire(library);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
static int32_t destroy_pipeline(void *raw, int kind) {
  RZMacPipeline *pipeline = (RZMacPipeline *)lookup(raw, kind);
  if (!pipeline)
    return missing();
  if (pipeline.leases)
    return fail(RZ_BUSY, @"pipeline is leased by a command");
  pipeline.compute = nil;
  pipeline.render = nil;
  pipeline.device = nil;
  retire(pipeline);
  return 0;
}
int32_t rinzu_macos_compute_destroy(void *raw) {
  @autoreleasepool {
    @try {
      return destroy_pipeline(raw, RZ_COMPUTE);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_render_destroy(void *raw) {
  @autoreleasepool {
    @try {
      return destroy_pipeline(raw, RZ_RENDER);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      if (!command)
        return missing();
      if (command.state || command.waiterToken)
        return fail(RZ_BUSY, @"wait or abandon command before destroy");
      command.device = nil;
      command.pins = nil;
      command.failure = nil;
      retire(command);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
static RZMacDevice *owning_device(RZMacHandle *handle) {
  switch (handle.kind) {
  case RZ_VIEW:
    return ((RZMacView *)handle).device;
  case RZ_BUFFER:
    return ((RZMacBuffer *)handle).device;
  case RZ_TEXTURE:
    return ((RZMacTexture *)handle).device;
  case RZ_SAMPLER:
    return ((RZMacSampler *)handle).device;
  case RZ_LIBRARY:
    return ((RZMacLibrary *)handle).device;
  case RZ_COMPUTE:
  case RZ_RENDER:
    return ((RZMacPipeline *)handle).device;
  case RZ_COMMAND:
    return ((RZMacCommand *)handle).device;
  default:
    return nil;
  }
}
int32_t rinzu_macos_device_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(raw, RZ_DEVICE);
      if (!device)
        return missing();
      for (RZMacHandle *handle in handles.allValues)
        if (!handle.closed && owning_device(handle) == device)
          return fail(RZ_BUSY, @"Device still owns live resources");
      device.queue = nil;
      device.device = nil;
      retire(device);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}

// Standard widgets share the checked View registry and the Session event queue.
enum { RZ_LABEL = 1, RZ_BUTTON, RZ_CHECKBOX, RZ_TEXT_FIELD, RZ_TEXT_VIEW, RZ_SCROLL_VIEW };
static NSUInteger scalar_count(NSString *text, NSUInteger end) {
  NSUInteger result = 0;
  for (NSUInteger index = 0; index < MIN(end, text.length); ++index) {
    unichar value = [text characterAtIndex:index];
    if (CFStringIsSurrogateHighCharacter(value) && index + 1 < text.length &&
        CFStringIsSurrogateLowCharacter([text characterAtIndex:index + 1]))
      ++index;
    ++result;
  }
  return result;
}
static BOOL utf16_offset(NSString *text, int64_t scalar, NSUInteger *output) {
  if (scalar < 0)
    return NO;
  NSUInteger index = 0;
  while (scalar && index < text.length) {
    unichar value = [text characterAtIndex:index++];
    if (CFStringIsSurrogateHighCharacter(value) && index < text.length &&
        CFStringIsSurrogateLowCharacter([text characterAtIndex:index]))
      ++index;
    --scalar;
  }
  if (scalar)
    return NO;
  *output = index;
  return YES;
}
static NSString *widget_string(RZMacView *view) {
  switch (view.widgetKind) {
  case RZ_LABEL:
  case RZ_TEXT_FIELD:
    return ((NSTextField *)view.view).stringValue;
  case RZ_BUTTON:
  case RZ_CHECKBOX:
    return ((NSButton *)view.view).title;
  case RZ_TEXT_VIEW:
    return ((NSTextView *)view.view).string;
  default:
    return nil;
  }
}
static NSTextView *text_editor(RZMacView *view) {
  if (view.widgetKind == RZ_TEXT_VIEW)
    return (NSTextView *)view.view;
  if (view.widgetKind == RZ_TEXT_FIELD)
    return (NSTextView *)((NSTextField *)view.view).currentEditor;
  return nil;
}
static struct MacSelection widget_selection(RZMacView *view) {
  NSString *text = widget_string(view);
  NSTextView *editor = text_editor(view);
  NSRange range = editor ? editor.selectedRange : NSMakeRange(0, 0);
  if (range.location == NSNotFound || range.location > text.length)
    range = NSMakeRange(0, 0);
  range.length = MIN(range.length, text.length - range.location);
  return (struct MacSelection){(int64_t)scalar_count(text, range.location),
                               (int64_t)scalar_count(text, NSMaxRange(range))};
}
static void widget_text_event(RZMacView *view, int kind) {
  if (view.updating || !view.enabled || view.closed)
    return;
  NSString *text = widget_string(view);
  if ([text lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > RZ_TEXT_LIMIT) {
    [view.owner enqueue:view_event(view, RZ_INPUT_LIMIT)];
    return;
  }
  NSData *bytes = [text dataUsingEncoding:NSUTF8StringEncoding];
  if (!bytes)
    _Exit(70);
  struct MacEvent event = view_event(view, kind);
  event.flags = (view.marking || text_editor(view).hasMarkedText) ? 1 : 0;
  struct MacSelection selection = widget_selection(view);
  event.selectionStart = selection.start;
  event.selectionEnd = selection.end;
  [view.owner enqueue:event text:bytes];
}
@interface RZMacTextView : NSTextView
@property(nonatomic, weak) RZMacView *control;
@end
@implementation RZMacTextView
- (void)setMarkedText:(id)value
        selectedRange:(NSRange)selection
     replacementRange:(NSRange)replacement {
  BOOL before = self.control.marking;
  self.control.marking = YES;
  @try {
    [super setMarkedText:value selectedRange:selection replacementRange:replacement];
  } @finally {
    self.control.marking = before;
  }
}
- (void)insertText:(id)value replacementRange:(NSRange)replacement {
  BOOL composing = self.hasMarkedText;
  [super insertText:value replacementRange:replacement];
  if (composing && !self.hasMarkedText)
    widget_text_event(self.control, RZ_TEXT_CHANGED);
}
- (void)unmarkText {
  BOOL composing = self.hasMarkedText;
  [super unmarkText];
  if (composing)
    widget_text_event(self.control, RZ_TEXT_CHANGED);
}
- (BOOL)shouldChangeTextInRange:(NSRange)range replacementString:(NSString *)replacement {
  if (!self.control.enabled)
    return NO;
  if (replacement && [[self.string stringByReplacingCharactersInRange:range withString:replacement]
                         lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > RZ_TEXT_LIMIT) {
    [self.control.owner enqueue:view_event(self.control, RZ_INPUT_LIMIT)];
    return NO;
  }
  return [super shouldChangeTextInRange:range replacementString:replacement];
}
- (void)undo:(id)sender {
  (void)sender;
  if (self.isFieldEditor) {
    [self.undoManager undo];
    return;
  }
  struct MacEvent event = view_event(self.control, RZ_ACTION);
  event.action = -1;
  [self.control.owner enqueue:event];
}
- (void)redo:(id)sender {
  (void)sender;
  if (self.isFieldEditor) {
    [self.undoManager redo];
    return;
  }
  struct MacEvent event = view_event(self.control, RZ_ACTION);
  event.action = -2;
  [self.control.owner enqueue:event];
}
@end
@implementation RZMacView (Widgets)
- (void)scrolled:(NSNotification *)notification {
  (void)notification;
  if (self.closed)
    return;
  NSRect bounds = ((NSScrollView *)self.view).contentView.bounds;
  struct MacEvent event = view_event(self, RZ_SCROLL);
  event.x = bounds.origin.x;
  event.y = bounds.origin.y;
  event.width = bounds.size.width;
  event.height = bounds.size.height;
  event.deltaX = bounds.origin.x - self.scrollOrigin.x;
  event.deltaY = bounds.origin.y - self.scrollOrigin.y;
  event.flags = 2;
  self.scrollOrigin = bounds.origin;
  [self.owner enqueue:event];
}
- (void)activated:(id)sender {
  (void)sender;
  if (!self.enabled || self.closed)
    return;
  struct MacEvent event = view_event(self, RZ_ACTION);
  event.action = self.action;
  event.value = self.widgetKind == RZ_CHECKBOX
                    ? (((NSButton *)self.view).state == NSControlStateValueOn)
                    : 1.0;
  [self.owner enqueue:event];
}
- (void)textDidChange:(NSNotification *)notification {
  (void)notification;
  widget_text_event(self, RZ_TEXT_CHANGED);
}
- (void)textViewDidChangeSelection:(NSNotification *)notification {
  (void)notification;
  widget_text_event(self, RZ_SELECTION_CHANGED);
}
- (void)controlTextDidChange:(NSNotification *)notification {
  (void)notification;
  widget_text_event(self, RZ_TEXT_CHANGED);
}
- (BOOL)textView:(NSTextView *)view
    shouldChangeTextInRange:(NSRange)range
          replacementString:(NSString *)replacement {
  if (!self.enabled)
    return NO;
  if (replacement && [[[view string] stringByReplacingCharactersInRange:range
                                                             withString:replacement]
                         lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > RZ_TEXT_LIMIT) {
    [self.owner enqueue:view_event(self, RZ_INPUT_LIMIT)];
    return NO;
  }
  return YES;
}
@end
static int64_t widget_create(void *raw, RZMacWindow *scope, int32_t kind, const uint8_t *bytes,
                             size_t length, const struct MacRect *rect, int64_t action) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      if (!accepting(session))
        return fail(RZ_BUSY, @"Session is closing");
      NSString *title = text(bytes, length);
      if (!title || length > RZ_TEXT_LIMIT || !rect_valid(rect) || kind < RZ_LABEL ||
          kind > RZ_SCROLL_VIEW || action < 0)
        return fail(RZ_ARGUMENT, @"invalid widget kind, text, frame or action");
      RZMacView *view = [RZMacView new];
      view.enabled = YES;
      view.widgetKind = kind;
      view.scopeWindow = scope;
      view.action = action;
      int64_t identity = publish(view, RZ_VIEW, session);
      switch (kind) {
      case RZ_LABEL:
        view.view = [NSTextField labelWithString:title];
        break;
      case RZ_BUTTON:
      case RZ_CHECKBOX: {
        NSButton *button = [NSButton buttonWithTitle:title
                                              target:view
                                              action:@selector(activated:)];
        if (kind == RZ_CHECKBOX)
          [button setButtonType:NSButtonTypeSwitch];
        view.view = button;
        break;
      }
      case RZ_TEXT_FIELD: {
        NSTextField *field = [NSTextField new];
        field.stringValue = title;
        field.delegate = view;
        field.target = view;
        field.action = @selector(activated:);
        RZMacTextView *editor = [[RZMacTextView alloc] initWithFrame:NSZeroRect];
        if (!editor)
          _Exit(70);
        editor.fieldEditor = YES;
        editor.control = view;
        view.fieldEditor = editor;
        view.view = field;
        break;
      }
      case RZ_TEXT_VIEW: {
        RZMacTextView *editor = [RZMacTextView new];
        editor.control = view;
        editor.delegate = view;
        editor.richText = NO;
        editor.allowsUndo = NO;
        editor.font = [NSFont monospacedSystemFontOfSize:13 weight:NSFontWeightRegular];
        editor.automaticQuoteSubstitutionEnabled = NO;
        editor.automaticDashSubstitutionEnabled = NO;
        editor.automaticTextReplacementEnabled = NO;
        editor.verticallyResizable = NO;
        editor.horizontallyResizable = NO;
        editor.autoresizingMask = NSViewWidthSizable;
        editor.textContainer.widthTracksTextView = YES;
        editor.textContainer.containerSize = NSMakeSize(rect->width, CGFLOAT_MAX);
        view.updating = YES;
        editor.string = title;
        view.updating = NO;
        view.view = editor;
        break;
      }
      case RZ_SCROLL_VIEW: {
        NSScrollView *scroll = [NSScrollView new];
        scroll.hasVerticalScroller = YES;
        scroll.borderType = NSBezelBorder;
        view.view = scroll;
        scroll.contentView.postsBoundsChangedNotifications = YES;
        [[NSNotificationCenter defaultCenter] addObserver:view
                                                 selector:@selector(scrolled:)
                                                     name:NSViewBoundsDidChangeNotification
                                                   object:scroll.contentView];
        break;
      }
      default:
        _Exit(86);
      }
      if (!view.view)
        _Exit(70);
      view.view.frame = nsrect(rect);
      return identity;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int64_t rinzu_macos_widget_create(void *raw, int32_t kind, const uint8_t *bytes, size_t length,
                                  const struct MacRect *rect, int64_t action) {
  return widget_create(raw, nil, kind, bytes, length, rect, action);
}
int64_t rinzu_macos_widget_scoped(void *raw, int32_t kind, const uint8_t *bytes, size_t length,
                                  const struct MacRect *rect, int64_t action) {
  @autoreleasepool {
    RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
    if (window && !accepting(window))
      return fail(RZ_BUSY, @"Window scope is closing");
    return window
               ? widget_create(reference(window.owner), window, kind, bytes, length, rect, action)
               : missing();
  }
}
int32_t rinzu_macos_widget_set_text(void *raw, const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      NSString *value = text(bytes, length), *previous = widget_string(view);
      if (!value || !previous || length > RZ_TEXT_LIMIT)
        return fail(RZ_ARGUMENT, @"widget does not accept this text");
      if ([previous isEqualToString:value])
        return 0;
      if (text_editor(view).hasMarkedText)
        return fail(RZ_BUSY, @"text composition is active; defer programmatic replacement");
      view.updating = YES;
      @try {
        if (view.widgetKind == RZ_TEXT_VIEW)
          ((NSTextView *)view.view).string = value;
        else if (view.widgetKind == RZ_BUTTON || view.widgetKind == RZ_CHECKBOX)
          ((NSButton *)view.view).title = value;
        else
          ((NSTextField *)view.view).stringValue = value;
      } @finally {
        view.updating = NO;
      }
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_widget_snapshot(void *raw, int64_t token) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      NSString *value = widget_string(view);
      if (!value || token <= 0 || token > next_waiter)
        return fail(RZ_ARGUMENT, @"widget has no text or invalid snapshot identity");
      if (view.snapshotToken)
        return fail(RZ_BUSY, @"widget already has a text reader");
      if ([value lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > RZ_TEXT_LIMIT)
        return fail(RZ_ARGUMENT, @"widget text exceeds snapshot limit");
      view.snapshotToken = token;
      ++view.leases;
      view.snapshotData = [value dataUsingEncoding:NSUTF8StringEncoding];
      if (!view.snapshotData)
        _Exit(70);
      view->textSnapshot = (rinzu_native_byte_array_t){.data = (uint8_t *)view.snapshotData.bytes,
                                                       .length = view.snapshotData.length};
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
_Bool rinzu_macos_widget_snapshot_owned(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
    return view && token > 0 && view.snapshotToken == token;
  }
}
const rinzu_native_byte_array_t *rinzu_macos_widget_bytes(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
    return view && token > 0 && view.snapshotToken == token ? &view->textSnapshot : NULL;
  }
}
int32_t rinzu_macos_widget_snapshot_finish(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
    if (!view)
      return missing();
    if (token <= 0 || view.snapshotToken != token)
      return fail(RZ_BUSY, @"widget snapshot identity mismatch");
    view.snapshotData = nil;
    view.snapshotToken = 0;
    --view.leases;
    view->textSnapshot = (rinzu_native_byte_array_t){0};
    return 0;
  }
}
int32_t rinzu_macos_widget_state(void *raw, struct MacWidgetState *output) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (!output)
        return fail(RZ_ARGUMENT, @"missing widget state output");
      *output = (struct MacWidgetState){.enabled = view.enabled, .hidden = view.view.hidden};
      if (view.widgetKind == RZ_CHECKBOX)
        output->value = ((NSButton *)view.view).state == NSControlStateValueOn;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_widget_value(void *raw, double value) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (view.widgetKind != RZ_CHECKBOX || (value != 0 && value != 1))
        return fail(RZ_ARGUMENT, @"widget does not accept this value");
      ((NSButton *)view.view).state = value ? NSControlStateValueOn : NSControlStateValueOff;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_view_enabled(void *raw, _Bool enabled) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      view.enabled = enabled;
      if ([view.view isKindOfClass:[NSControl class]])
        ((NSControl *)view.view).enabled = enabled;
      if (view.dataController)
        view.dataController.table.enabled = enabled;
      if (view.widgetKind == RZ_TEXT_VIEW)
        ((NSTextView *)view.view).editable = enabled;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_view_hidden(void *raw, _Bool hidden) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      view.view.hidden = hidden;
      for (RZMacHandle *handle in handles.allValues)
        if (handle.kind == RZ_FRAME_CLOCK && handle.owner == view.owner)
          refresh_frame((RZMacFrameClock *)handle, YES);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_view_focus(void *raw) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      NSView *target = view.dataController ? view.dataController.table : view.view;
      return [view.view.window makeFirstResponder:target]
                 ? 0
                 : fail(RZ_ARGUMENT, @"view cannot receive focus");
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_text_selection(void *raw, struct MacSelection *output) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (!output || (view.widgetKind != RZ_TEXT_VIEW && view.widgetKind != RZ_TEXT_FIELD))
        return fail(RZ_ARGUMENT, @"selection requires a text widget and output");
      *output = widget_selection(view);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_text_select(void *raw, int64_t start, int64_t end) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (view.widgetKind != RZ_TEXT_VIEW && view.widgetKind != RZ_TEXT_FIELD)
        return fail(RZ_ARGUMENT, @"selection requires a text widget");
      NSString *value = widget_string(view);
      NSUInteger from, to;
      if (end < start || !utf16_offset(value, start, &from) || !utf16_offset(value, end, &to))
        return fail(RZ_ARGUMENT, @"invalid Unicode scalar selection");
      NSTextView *editor = text_editor(view);
      if (!editor)
        return fail(RZ_BUSY, @"TextField must have focus before selection");
      if (editor.hasMarkedText)
        return fail(RZ_BUSY, @"text composition is active");
      view.updating = YES;
      @try {
        editor.selectedRange = NSMakeRange(from, to - from);
      } @finally {
        view.updating = NO;
      }
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_scroll_content(void *raw, void *child_raw) {
  @autoreleasepool {
    @try {
      RZMacView *scroll = (RZMacView *)lookup(raw, RZ_VIEW);
      RZMacView *child = (RZMacView *)lookup(child_raw, RZ_VIEW);
      if (!scroll || !child)
        return missing();
      if (scroll.widgetKind != RZ_SCROLL_VIEW || scroll.owner != child.owner || child.rootWindow ||
          scroll == child || descendant(scroll, child))
        return fail(RZ_ARGUMENT, @"invalid ScrollView content");
      if (view_busy(scroll) || view_busy(child))
        return fail(RZ_BUSY, @"scroll content is leased");
      for (RZMacHandle *handle in handles.allValues)
        if (handle.kind == RZ_VIEW && ((RZMacView *)handle).parent == scroll)
          ((RZMacView *)handle).parent = nil;
      child.parent = scroll;
      if (scroll.scopeWindow)
        for (RZMacHandle *handle in handles.allValues)
          if (handle.kind == RZ_VIEW && descendant((RZMacView *)handle, child))
            ((RZMacView *)handle).scopeWindow = scroll.scopeWindow;
      NSScrollView *native = (NSScrollView *)scroll.view;
      native.documentView = child.view;
      if (child.widgetKind == RZ_TEXT_VIEW) {
        NSTextView *editor = (NSTextView *)child.view;
        editor.verticallyResizable = YES;
        editor.minSize = NSMakeSize(0, native.contentSize.height);
        editor.frame = NSMakeRect(0, 0, native.contentSize.width,
                                  MAX(editor.frame.size.height, native.contentSize.height));
        editor.textContainer.containerSize = NSMakeSize(native.contentSize.width, CGFLOAT_MAX);
      }
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}

int32_t rinzu_macos_view_autoresizing(void *raw, int64_t mask) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (mask < 0 || mask > 63)
        return fail(RZ_ARGUMENT, @"invalid autoresizing mask");
      view.view.autoresizingMask = (NSAutoresizingMaskOptions)mask;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
static SEL standard_command(int32_t command) {
  switch (command) {
  case 1:
    return @selector(cut:);
  case 2:
    return @selector(copy:);
  case 3:
    return @selector(paste:);
  case 4:
    return @selector(selectAll:);
  case 5:
    return @selector(undo:);
  case 6:
    return @selector(redo:);
  case 7:
    return @selector(terminate:);
  default:
    return NULL;
  }
}
@implementation RZMacMenu
- (void)activated:(NSMenuItem *)sender {
  if (self.closed || !self.scopeWindow || !sender.enabled || !self.owner.running)
    return;
  [self.owner enqueue:(struct MacEvent){.kind = RZ_ACTION,
                                        .windowId = self.scopeWindow.identity,
                                        .action = sender.tag}];
}
@end
int64_t rinzu_macos_menu_create(void *raw, const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      if (!window)
        return missing();
      NSString *title = text(bytes, length);
      if (!accepting(window) || !title || length > RZ_TEXT_LIMIT)
        return fail(RZ_ARGUMENT, @"invalid menu title or closing Window");
      RZMacMenu *menu = [RZMacMenu new];
      // Publish the scope before allocating payload, also covering unreceived constructors.
      menu.scopeWindow = window;
      int64_t identity = publish(menu, RZ_MENU, window.owner);
      menu.menu = [[NSMenu alloc] initWithTitle:title];
      menu.menu.autoenablesItems = NO;
      if (!menu.menu)
        _Exit(70);
      return identity;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_menu(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_MENU);
  }
}
int32_t rinzu_macos_menu_item(void *raw, const uint8_t *bytes, size_t length,
                              const uint8_t *keyBytes, size_t keyLength, int64_t modifiers,
                              int64_t action, int32_t standard) {
  @autoreleasepool {
    @try {
      RZMacMenu *menu = (RZMacMenu *)lookup(raw, RZ_MENU);
      if (!menu)
        return missing();
      NSString *title = text(bytes, length), *key = text(keyBytes, keyLength);
      int64_t supported = NSEventModifierFlagCommand | NSEventModifierFlagShift |
                          NSEventModifierFlagControl | NSEventModifierFlagOption;
      SEL selector = standard ? standard_command(standard) : @selector(activated:);
      if (!accepting(menu) || !title || length > RZ_TEXT_LIMIT || !key || key.length > 8 ||
          modifiers < 0 || (modifiers & ~supported) || !selector ||
          menu.menu.numberOfItems >= RZ_EVENT_LIMIT)
        return fail(RZ_ARGUMENT, @"invalid menu item, shortcut or item limit");
      if (!standard)
        for (NSMenuItem *item in menu.menu.itemArray)
          if (item.target == menu && item.tag == action)
            return fail(RZ_ARGUMENT, @"duplicate command identity in menu");
      NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:title action:selector keyEquivalent:key];
      if (!item)
        _Exit(70);
      item.keyEquivalentModifierMask = (NSEventModifierFlags)modifiers;
      item.target = standard ? nil : menu;
      item.tag = action;
      [menu.menu addItem:item];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_menu_submenu(void *raw, void *childRaw) {
  @autoreleasepool {
    @try {
      RZMacMenu *parent = (RZMacMenu *)lookup(raw, RZ_MENU);
      RZMacMenu *child = (RZMacMenu *)lookup(childRaw, RZ_MENU);
      if (!parent || !child)
        return missing();
      if (!accepting(parent) || parent.scopeWindow != child.scopeWindow || child.parent ||
          child.scopeWindow.menu == child || parent.menu.numberOfItems >= RZ_EVENT_LIMIT)
        return fail(RZ_ARGUMENT, @"submenu must be detached and belong to the same Window");
      for (RZMacMenu *cursor = parent; cursor; cursor = cursor.parent)
        if (cursor == child)
          return fail(RZ_ARGUMENT, @"cyclic submenu");
      NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:child.menu.title
                                                    action:NULL
                                             keyEquivalent:@""];
      if (!item)
        _Exit(70);
      item.submenu = child.menu;
      [parent.menu addItem:item];
      child.parent = parent;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_menu_separator(void *raw) {
  @autoreleasepool {
    @try {
      RZMacMenu *menu = (RZMacMenu *)lookup(raw, RZ_MENU);
      if (!menu)
        return missing();
      if (!accepting(menu) || menu.menu.numberOfItems >= RZ_EVENT_LIMIT)
        return fail(RZ_ARGUMENT, @"menu item limit or closing Window");
      [menu.menu addItem:[NSMenuItem separatorItem]];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_menu_state(void *raw, int64_t action, _Bool enabled, _Bool checked) {
  @autoreleasepool {
    @try {
      RZMacMenu *menu = (RZMacMenu *)lookup(raw, RZ_MENU);
      if (!menu)
        return missing();
      for (NSMenuItem *item in menu.menu.itemArray) {
        if (item.target != menu || item.tag != action)
          continue;
        item.enabled = enabled;
        item.state = checked ? NSControlStateValueOn : NSControlStateValueOff;
        return 0;
      }
      return fail(RZ_ARGUMENT, @"command identity not in menu");
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_menu_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacMenu *menu = (RZMacMenu *)lookup(raw, RZ_MENU);
      if (!menu)
        return missing();
      close_menu(menu);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_window_menu(void *raw, void *menuRaw) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      RZMacMenu *menu = (RZMacMenu *)lookup(menuRaw, RZ_MENU);
      if (!window || !menu)
        return missing();
      if (!accepting(window) || menu.scopeWindow != window || menu.parent)
        return fail(RZ_ARGUMENT, @"main menu must be a root of this Window");
      window.menu = menu;
      refresh_menu(window.owner);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_window_command(void *raw, int32_t command) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      if (!window)
        return missing();
      SEL selector = standard_command(command);
      if (!selector)
        return fail(RZ_ARGUMENT, @"unknown standard command");
      if (command == 7) {
        [window.owner enqueue:(struct MacEvent){.kind = RZ_QUIT_REQUESTED}];
        return 0;
      }
      // Explicit targeting must not accidentally edit a different key Window.
      NSResponder *target = window.window.firstResponder;
      while (target && ![target respondsToSelector:selector])
        target = target.nextResponder;
      if (!target)
        return fail(RZ_ARGUMENT, @"Window responder does not handle this command");
      [NSApp sendAction:selector to:target from:nil];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_clipboard_write(void *raw, const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      NSString *value = text(bytes, length);
      if (!value || length > RZ_TEXT_LIMIT || !accepting(session))
        return fail(RZ_ARGUMENT, @"invalid clipboard text or closing Session");
      NSPasteboard *pasteboard = session.pasteboard ?: [NSPasteboard generalPasteboard];
      [pasteboard clearContents];
      return [pasteboard setString:value forType:NSPasteboardTypeString]
                 ? 0
                 : fail(RZ_EXCEPTION, @"clipboard write failed");
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_clipboard_snapshot(void *raw, int64_t token) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      if (token <= 0 || token > next_waiter)
        return fail(RZ_ARGUMENT, @"invalid clipboard snapshot identity");
      if (session.clipboardToken || !accepting(session))
        return fail(RZ_BUSY, @"clipboard reader active or Session closing");
      NSPasteboard *pasteboard = session.pasteboard ?: [NSPasteboard generalPasteboard];
      NSString *value = [pasteboard stringForType:NSPasteboardTypeString] ?: @"";
      if ([value lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > RZ_TEXT_LIMIT)
        return fail(RZ_ARGUMENT, @"clipboard text exceeds 1 MiB");
      session.clipboardToken = token;
      session.clipboardData = [value dataUsingEncoding:NSUTF8StringEncoding];
      if (!session.clipboardData)
        _Exit(70);
      session->clipboardSnapshot = (rinzu_native_byte_array_t){
          .data = (uint8_t *)session.clipboardData.bytes, .length = session.clipboardData.length};
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
_Bool rinzu_macos_clipboard_owned(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session && token > 0 && session.clipboardToken == token;
  }
}
const rinzu_native_byte_array_t *rinzu_macos_clipboard_bytes(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    return session && token > 0 && session.clipboardToken == token ? &session->clipboardSnapshot
                                                                   : NULL;
  }
}
int32_t rinzu_macos_clipboard_finish(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (token <= 0 || session.clipboardToken != token)
      return fail(RZ_BUSY, @"clipboard reader does not own snapshot");
    session.clipboardToken = 0;
    session.clipboardData = nil;
    session->clipboardSnapshot = (rinzu_native_byte_array_t){0};
    return 0;
  }
}

static RZMacWindow *scoped_window(RZMacSession *session, int64_t token) {
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_WINDOW && handle.owner == session &&
        ((RZMacWindow *)handle).scopeToken == token)
      return (RZMacWindow *)handle;
  return nil;
}
static BOOL command_uses_window(RZMacCommand *command, RZMacWindow *window) {
  if (!window || !command.state)
    return NO;
  for (RZMacHandle *pin in command.pins)
    if (pin.kind == RZ_VIEW && ((RZMacView *)pin).scopeWindow == window)
      return YES;
  return NO;
}
static RZMacCommand *window_pending_command(void *raw, int64_t token) {
  RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
  if (!session)
    return nil;
  RZMacWindow *window = scoped_window(session, token);
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_COMMAND && handle.owner == session &&
        command_uses_window((RZMacCommand *)handle, window))
      return (RZMacCommand *)handle;
  return nil;
}
int32_t rinzu_macos_window_scope_prepare(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
    if (!session)
      return missing();
    if (token <= 0 || token > next_waiter)
      return fail(RZ_ARGUMENT, @"invalid Window scope identity");
    RZMacWindow *window = scoped_window(session, token);
    if (!window)
      return 0;
    if (window.sheet) {
      window.sheetsStopping = YES;
      cancel_sheet(window.sheet);
      return fail(RZ_BUSY, @"join sheet result before closing Window scope");
    }
    for (RZMacHandle *handle in handles.allValues) {
      if (handle.kind == RZ_VIEW && ((RZMacView *)handle).scopeWindow == window &&
          ((RZMacView *)handle).snapshotToken)
        return fail(RZ_BUSY, @"join text readers before closing Window scope");
      if (handle.kind == RZ_COMMAND && command_uses_window((RZMacCommand *)handle, window) &&
          ((RZMacCommand *)handle).waiterToken)
        return fail(RZ_BUSY, @"join GPU waiters before closing Window scope");
    }
    window.closing = YES;
    return 0;
  }
}
_Bool rinzu_macos_window_scope_has_command(void *raw, int64_t token) {
  @autoreleasepool {
    return window_pending_command(raw, token) != nil;
  }
}
void *rinzu_macos_window_scope_command(void *raw, int64_t token) {
  @autoreleasepool {
    return reference(window_pending_command(raw, token));
  }
}

// Sheets have a separate, Window-owned receipt. They never consume nextEvent
// and their completion blocks retain only native state. A slot survives physical
// dismissal until the Source waiter copies its result and acknowledges it.
static RZMacSheet *owned_sheet(void *raw, int64_t token) {
  RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
  return window && token > 0 && window.sheet.token == token ? window.sheet : nil;
}
static void cancel_sheet(RZMacSheet *sheet) {
  if (!sheet || sheet.finished || sheet.dismissIssued)
    return;
  BOOL first = !sheet.cancelRequested;
  sheet.cancelRequested = YES;
  NSWindow *parent = sheet.window.sheetParent;
  if (parent) {
    sheet.dismissIssued = YES;
    @try {
      [parent endSheet:sheet.window returnCode:NSModalResponseCancel];
      // A Source cancellation may run inside a CFRunLoop observer with no
      // further input/timer. Let NSApplication finish the dismissal even when
      // nextEvent would otherwise remain asleep after the observer returns.
      wake_application();
    } @catch (NSException *exception) {
      sheet.dismissIssued = NO;
      @throw exception;
    }
  } else if (first) {
    wake_application();
  }
}
static void drain_sheets(RZMacSession *session) {
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_WINDOW && handle.owner == session)
      cancel_sheet(((RZMacWindow *)handle).sheet);
  while (true) {
    BOOL pending = NO;
    for (RZMacHandle *handle in handles.allValues)
      if (handle.kind == RZ_WINDOW && handle.owner == session && ((RZMacWindow *)handle).sheet &&
          !((RZMacWindow *)handle).sheet.finished)
        pending = YES;
    if (!pending)
      return;
    // Native-only cleanup of already admitted sheets, not a synchronous modal
    // wrapper around a managed callback. Their completion blocks publish done.
    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
  }
}
static void finish_sheet(RZMacSheet *sheet, int32_t result, NSString *path) {
  if (sheet.finished)
    return;
  if (sheet.cancelRequested) {
    result = 0;
    path = nil;
  }
  if (path) {
    sheet.path = [path dataUsingEncoding:NSUTF8StringEncoding];
    if (!sheet.path)
      _Exit(70);
    if (sheet.path.length > RZ_TEXT_LIMIT) {
      sheet.path = nil;
      result = RZ_ARGUMENT;
      sheet.failure = @"selected path exceeds snapshot limit";
    }
  }
  sheet->snapshot =
      (rinzu_native_byte_array_t){.data = (uint8_t *)sheet.path.bytes, .length = sheet.path.length};
  sheet.result = result;
  sheet.finished = YES;
  // AppKit has completed its dismissal. Drop our panel/alert references before
  // physical publication, which permits the consumer to release this slot.
  sheet.window = nil;
  sheet.payload = nil;
  stop_after_sheets(sheet.parent.owner);
  rinzu_native_completion_v2_publish(&sheet->completion);
}
static BOOL window_composing(RZMacWindow *window) {
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_VIEW && ((RZMacView *)handle).scopeWindow == window &&
        text_editor((RZMacView *)handle).hasMarkedText)
      return YES;
  return NO;
}
static int32_t sheet_admission(RZMacWindow *window, int64_t token) {
  if (!window)
    return missing();
  if (token <= 0 || token > next_waiter)
    return fail(RZ_ARGUMENT, @"invalid sheet identity");
  if (!accepting(window) || window.sheetsStopping || window.sheet || !window.owner.running ||
      window.owner.stopping || window.window.attachedSheet)
    return fail(RZ_BUSY, @"Window has a sheet, is stopping, or has no running application");
  if (window_composing(window))
    return fail(RZ_BUSY, @"finish text composition before presenting a sheet");
  return 0;
}
static RZMacSheet *new_sheet(RZMacWindow *window, int64_t token, NSWindow *native, id payload) {
  RZMacSheet *sheet = [RZMacSheet new];
  if (!sheet || !native || !payload)
    _Exit(70);
  rinzu_native_completion_v2_init(&sheet->completion);
  sheet.token = token;
  sheet.parent = window;
  sheet.window = native;
  sheet.payload = payload;
  window.sheet = sheet;
  return sheet;
}
static int32_t sheet_exception(RZMacWindow *window, NSException *exception) {
  int32_t result = caught(exception);
  RZMacSheet *sheet = window.sheet;
  if (sheet && !sheet.finished) {
    sheet.failure = exception.reason ?: exception.name;
    if (sheet.window.sheetParent)
      cancel_sheet(sheet);
    else
      finish_sheet(sheet, result, nil);
  }
  return result;
}
int32_t rinzu_macos_sheet_ready(void *raw) {
  @autoreleasepool {
    RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
    if (!window)
      return missing();
    if (!accepting(window) || window.sheetsStopping || window.owner.stopping ||
        (window.owner.hasRun && !window.owner.running))
      return fail(RZ_BUSY, @"Window or application is stopping");
    return window.owner.running ? 1 : 0;
  }
}
int32_t rinzu_macos_panel_begin(void *raw, int64_t token, int32_t save, const uint8_t *title,
                                size_t title_length, const uint8_t *directory,
                                size_t directory_length, const uint8_t *name, size_t name_length) {
  @autoreleasepool {
    RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
    int32_t admission = sheet_admission(window, token);
    if (admission)
      return admission;
    @try {
      if (title_length > 4096 || directory_length > RZ_TEXT_LIMIT || name_length > 4096)
        return fail(RZ_ARGUMENT, @"panel text exceeds its limit");
      NSString *heading = text(title, title_length);
      NSString *folder = text(directory, directory_length);
      NSString *filename = text(name, name_length);
      if (!heading || !folder || !filename || title_length > 4096 ||
          directory_length > RZ_TEXT_LIMIT || name_length > 4096 || (save != 0 && save != 1) ||
          (folder.length && !folder.isAbsolutePath) ||
          [folder rangeOfString:@"\0"].location != NSNotFound ||
          [filename rangeOfString:@"\0"].location != NSNotFound ||
          [filename rangeOfString:@"/"].location != NSNotFound || [filename isEqualToString:@"."] ||
          [filename isEqualToString:@".."])
        return fail(RZ_ARGUMENT, @"invalid panel text, absolute directory or suggested filename");
      NSSavePanel *panel = save ? [NSSavePanel savePanel] : [NSOpenPanel openPanel];
      panel.title = heading;
      panel.canCreateDirectories = save;
      if (!save) {
        NSOpenPanel *open = (NSOpenPanel *)panel;
        open.canChooseFiles = YES;
        open.canChooseDirectories = NO;
        open.allowsMultipleSelection = NO;
      }
      if (folder.length)
        panel.directoryURL = [NSURL fileURLWithPath:folder isDirectory:YES];
      if (filename.length)
        panel.nameFieldStringValue = filename;
      RZMacSheet *sheet = new_sheet(window, token, panel, panel);
      [panel beginSheetModalForWindow:window.window
                    completionHandler:^(NSModalResponse response) {
                      @try {
                        NSSavePanel *finished = sheet.payload;
                        NSURL *url = finished.URL;
                        if (response == NSModalResponseOK && url.isFileURL)
                          finish_sheet(sheet, 1, url.path);
                        else if (response == NSModalResponseOK) {
                          sheet.failure = @"panel returned no file URL";
                          finish_sheet(sheet, RZ_ARGUMENT, nil);
                        } else
                          finish_sheet(sheet, 0, nil);
                      } @catch (NSException *exception) {
                        sheet.failure = exception.reason ?: exception.name;
                        finish_sheet(sheet, caught(exception), nil);
                      }
                    }];
      return 0;
    } @catch (NSException *exception) {
      return sheet_exception(window, exception);
    }
  }
}
int32_t rinzu_macos_alert_begin(void *raw, int64_t token, const uint8_t *title, size_t title_length,
                                const uint8_t *message, size_t message_length, const uint8_t *first,
                                size_t first_length, const uint8_t *second, size_t second_length,
                                const uint8_t *third, size_t third_length) {
  @autoreleasepool {
    RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
    int32_t admission = sheet_admission(window, token);
    if (admission)
      return admission;
    @try {
      if (title_length > 4096 || message_length > 65536 || first_length > 4096 ||
          second_length > 4096 || third_length > 4096)
        return fail(RZ_ARGUMENT, @"alert text exceeds its limit");
      NSString *heading = text(title, title_length), *body = text(message, message_length);
      NSString *one = text(first, first_length), *two = text(second, second_length);
      NSString *three = text(third, third_length);
      if (!heading || !body || !one.length || !two || !three || title_length > 4096 ||
          message_length > 65536 || first_length > 4096 || second_length > 4096 ||
          third_length > 4096 || (!two.length && three.length))
        return fail(RZ_ARGUMENT, @"alert requires one to three bounded button titles");
      NSAlert *alert = [NSAlert new];
      if (!alert)
        _Exit(70);
      alert.messageText = heading;
      alert.informativeText = body;
      [alert addButtonWithTitle:one];
      if (two.length)
        [alert addButtonWithTitle:two];
      if (three.length)
        [alert addButtonWithTitle:three];
      if (alert.buttons.count > 1)
        alert.buttons.lastObject.keyEquivalent = @"\e";
      RZMacSheet *sheet = new_sheet(window, token, alert.window, alert);
      [alert beginSheetModalForWindow:window.window
                    completionHandler:^(NSModalResponse response) {
                      @try {
                        NSAlert *finished = sheet.payload;
                        int32_t button = (int32_t)(response - NSAlertFirstButtonReturn + 1);
                        finish_sheet(
                            sheet,
                            button >= 1 && button <= (int32_t)finished.buttons.count ? button : 0,
                            nil);
                      } @catch (NSException *exception) {
                        sheet.failure = exception.reason ?: exception.name;
                        finish_sheet(sheet, caught(exception), nil);
                      }
                    }];
      return 0;
    } @catch (NSException *exception) {
      return sheet_exception(window, exception);
    }
  }
}
_Bool rinzu_macos_sheet_owned(void *raw, int64_t token) {
  @autoreleasepool {
    return owned_sheet(raw, token) != nil;
  }
}
void *rinzu_macos_sheet_completion(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSheet *sheet = owned_sheet(raw, token);
    return sheet ? &sheet->completion.base : NULL;
  }
}
int32_t rinzu_macos_sheet_result(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSheet *sheet = owned_sheet(raw, token);
    if (!sheet || !sheet.finished)
      return fail(RZ_BUSY, @"sheet result requires ownership and physical completion");
    return sheet.result < 0 ? fail(sheet.result, sheet.failure) : sheet.result;
  }
}
const rinzu_native_byte_array_t *rinzu_macos_sheet_bytes(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSheet *sheet = owned_sheet(raw, token);
    return sheet && sheet.finished ? &sheet->snapshot : NULL;
  }
}
int32_t rinzu_macos_sheet_cancel(void *raw, int64_t token) {
  @autoreleasepool {
    @try {
      RZMacSheet *sheet = owned_sheet(raw, token);
      if (!sheet)
        return fail(RZ_BUSY, @"sheet cancellation requires ownership");
      cancel_sheet(sheet);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_sheet_finish(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacSheet *sheet = owned_sheet(raw, token);
    if (!sheet || !rinzu_native_completion_is_done(&sheet->completion.base))
      return fail(RZ_BUSY, @"sheet acknowledgement requires ownership and physical completion");
    sheet.parent.sheet = nil;
    return 0;
  }
}
int32_t rinzu_macos_sheet_dismiss(void *raw) {
  @autoreleasepool {
    RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
    return window ? (window.sheet ? rinzu_macos_sheet_cancel(raw, window.sheet.token) : 0)
                  : missing();
  }
}
int32_t rinzu_macos_sheet_stop(void *raw) {
  @autoreleasepool {
    RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
    if (!window)
      return missing();
    window.sheetsStopping = YES;
    return rinzu_macos_sheet_dismiss(raw);
  }
}
int32_t rinzu_macos_window_state(void *raw) {
  @autoreleasepool {
    RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
    return window ? window_flags(window) : missing();
  }
}
int32_t rinzu_macos_window_visibility(void *raw, int32_t operation) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      if (!window)
        return missing();
      if (!accepting(window))
        return fail(RZ_BUSY, @"Window is closing");
      switch (operation) {
      case 1:
        [window.window orderOut:nil];
        break;
      case 2:
        [window.window miniaturize:nil];
        break;
      case 3:
        [window.window deminiaturize:nil];
        [window.window makeKeyAndOrderFront:nil];
        break;
      default:
        return fail(RZ_ARGUMENT, @"invalid Window visibility operation");
      }
      window_state(window);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_text_composing(void *raw) {
  @autoreleasepool {
    RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
    if (!view)
      return missing();
    if (view.widgetKind != RZ_TEXT_VIEW && view.widgetKind != RZ_TEXT_FIELD)
      return fail(RZ_ARGUMENT, @"composition requires a text widget");
    return text_editor(view).hasMarkedText ? 1 : 0;
  }
}
int32_t rinzu_macos_scroll_to(void *raw, double x, double y) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (view.widgetKind != RZ_SCROLL_VIEW || !isfinite(x) || !isfinite(y))
        return fail(RZ_ARGUMENT, @"scroll requires a ScrollView and finite coordinates");
      NSScrollView *scroll = (NSScrollView *)view.view;
      NSClipView *clip = scroll.contentView;
      NSRect proposed = clip.bounds;
      proposed.origin = NSMakePoint(x, y);
      [clip scrollToPoint:[clip constrainBoundsRect:proposed].origin];
      [scroll reflectScrolledClipView:clip];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_scroll_rect(void *raw, struct MacRect *output) {
  @autoreleasepool {
    RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
    if (!view)
      return missing();
    if (view.widgetKind != RZ_SCROLL_VIEW || !output)
      return fail(RZ_ARGUMENT, @"visible rect requires a ScrollView and output");
    NSRect bounds = ((NSScrollView *)view.view).contentView.bounds;
    *output =
        (struct MacRect){bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height};
    return 0;
  }
}

// Single-level, four-byte unorm textures. Shared storage is coherent on the
// macOS arm64 devices supported by this SDK; CPU access still requires no leases.
static BOOL texture_size(int64_t width, int64_t height) {
  return width > 0 && height > 0 && width <= 8192 && height <= 8192 &&
         (uint64_t)width * (uint64_t)height <= RZ_BUFFER_LIMIT / 4;
}
static RZMacTexture *new_texture(RZMacDevice *device, int64_t width, int64_t height, int64_t format,
                                 int64_t usage) {
  if (!texture_size(width, height) || (format != 1 && format != 2) || usage < 1 || usage > 7 ||
      (format == 2 && (usage & 4))) {
    fail(RZ_ARGUMENT, @"invalid texture dimensions (1..8192), format or usage");
    return nil;
  }
  MTLTextureDescriptor *descriptor = [MTLTextureDescriptor
      texture2DDescriptorWithPixelFormat:format == 1 ? MTLPixelFormatRGBA8Unorm
                                                     : MTLPixelFormatBGRA8Unorm
                                   width:(NSUInteger)width
                                  height:(NSUInteger)height
                               mipmapped:NO];
  descriptor.storageMode = MTLStorageModeShared;
  descriptor.usage = (usage & 1 ? MTLTextureUsageShaderRead : 0) |
                     (usage & 2 ? MTLTextureUsageRenderTarget : 0) |
                     (usage & 4 ? MTLTextureUsageShaderWrite : 0);
  RZMacTexture *texture = [RZMacTexture new];
  texture.texture = [device.device newTextureWithDescriptor:descriptor];
  if (!texture.texture)
    _Exit(70);
  texture.device = device;
  texture.format = format;
  texture.usage = usage;
  return texture;
}
int64_t rinzu_macos_texture_create(void *raw, int64_t width, int64_t height, int64_t format,
                                   int64_t usage) {
  @autoreleasepool {
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(raw, RZ_DEVICE);
      if (!device)
        return missing();
      if (!accepting(device))
        return fail(RZ_BUSY, @"Session is closing");
      RZMacTexture *texture = new_texture(device, width, height, format, usage);
      return texture ? publish(texture, RZ_TEXTURE, device.owner) : RZ_ARGUMENT;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_texture(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_TEXTURE);
  }
}
int64_t rinzu_macos_texture_id(void *raw) {
  @autoreleasepool {
    RZMacTexture *texture = (RZMacTexture *)lookup(raw, RZ_TEXTURE);
    return texture ? texture.identity : missing();
  }
}
int32_t rinzu_macos_rotation2d(double radians, struct MacTransform2D *output) {
  @autoreleasepool {
    if (!isfinite(radians) || !output)
      return fail(RZ_ARGUMENT, @"invalid rotation or output");
    double c = cos(radians), s = sin(radians);
    *output = (struct MacTransform2D){c, s, -s, c, 0, 0};
    return 0;
  }
}
int32_t rinzu_macos_surface_size(void *raw, struct MacRect *output) {
  @autoreleasepool {
    RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
    if (!view)
      return missing();
    if (!view.device || !output)
      return fail(RZ_ARGUMENT, @"drawable size requires a Metal surface and output");
    CGSize size = ((CAMetalLayer *)view.view.layer).drawableSize;
    *output = (struct MacRect){0, 0, size.width, size.height};
    return 0;
  }
}
int32_t rinzu_macos_texture_info(void *raw, struct MacTextureInfo *output) {
  @autoreleasepool {
    RZMacTexture *texture = (RZMacTexture *)lookup(raw, RZ_TEXTURE);
    if (!texture)
      return missing();
    if (!output)
      return fail(RZ_ARGUMENT, @"missing texture info output");
    *output = (struct MacTextureInfo){
        (int64_t)texture.texture.width, (int64_t)texture.texture.height, texture.format,
        texture.usage, (int64_t)(texture.texture.width * texture.texture.height * 4)};
    return 0;
  }
}
static BOOL texture_region(RZMacTexture *texture, const struct MacScissor *region) {
  return region && region->width > 0 && region->height > 0 &&
         range_valid(texture.texture.width, region->x, (uint64_t)region->width) &&
         range_valid(texture.texture.height, region->y, (uint64_t)region->height);
}
static MTLRegion metal_region(const struct MacScissor *region) {
  return MTLRegionMake2D((NSUInteger)region->x, (NSUInteger)region->y, (NSUInteger)region->width,
                         (NSUInteger)region->height);
}
int32_t rinzu_macos_texture_write(void *raw, const struct MacScissor *region, int64_t row_bytes,
                                  const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacTexture *texture = (RZMacTexture *)lookup(raw, RZ_TEXTURE);
      if (!texture)
        return missing();
      if (texture.leases)
        return fail(RZ_BUSY, @"texture is leased by a command");
      if (!texture_region(texture, region) || row_bytes < region->width * 4 ||
          row_bytes > RZ_BUFFER_LIMIT || row_bytes % 4 || !bytes ||
          (uint64_t)row_bytes * (uint64_t)(region->height - 1) + (uint64_t)region->width * 4 >
              length)
        return fail(RZ_ARGUMENT, @"invalid texture region, row stride or upload bytes");
      [texture.texture replaceRegion:metal_region(region)
                         mipmapLevel:0
                           withBytes:bytes
                         bytesPerRow:(NSUInteger)row_bytes];
      texture.readback = nil;
      texture->snapshot = (rinzu_native_byte_array_t){0};
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_texture_read(void *raw, const struct MacScissor *region) {
  @autoreleasepool {
    @try {
      RZMacTexture *texture = (RZMacTexture *)lookup(raw, RZ_TEXTURE);
      if (!texture)
        return missing();
      if (texture.leases)
        return fail(RZ_BUSY, @"texture is leased by a command");
      if (!texture_region(texture, region))
        return fail(RZ_ARGUMENT, @"invalid texture read region");
      NSUInteger stride = (NSUInteger)region->width * 4;
      NSMutableData *bytes = [NSMutableData dataWithLength:stride * (NSUInteger)region->height];
      if (!bytes)
        _Exit(70);
      [texture.texture getBytes:bytes.mutableBytes
                    bytesPerRow:stride
                     fromRegion:metal_region(region)
                    mipmapLevel:0];
      texture.readback = bytes;
      texture->snapshot = (rinzu_native_byte_array_t){(uint8_t *)bytes.bytes, bytes.length};
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
const rinzu_native_byte_array_t *rinzu_macos_texture_bytes(void *raw) {
  @autoreleasepool {
    RZMacTexture *texture = (RZMacTexture *)lookup(raw, RZ_TEXTURE);
    return texture && !texture.leases ? &texture->snapshot : NULL;
  }
}
int32_t rinzu_macos_texture_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacTexture *texture = (RZMacTexture *)lookup(raw, RZ_TEXTURE);
      if (!texture)
        return missing();
      if (texture.leases)
        return fail(RZ_BUSY, @"texture is leased by a command");
      texture.texture = nil;
      texture.device = nil;
      texture.readback = nil;
      texture->snapshot = (rinzu_native_byte_array_t){0};
      retire(texture);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
// Decode frame zero only, normalize EXIF orientation and color to premultiplied
// sRGB RGBA8. Inspect dimensions before requesting decompression.
static NSData *decode_image(const uint8_t *bytes, size_t length, size_t *width, size_t *height) {
  if (!bytes || !length || length > 67108864)
    return nil;
  CGImageSourceRef source = NULL;
  CGImageRef image = NULL;
  CGColorSpaceRef color = NULL;
  CGContextRef context = NULL;
  @try {
    NSData *encoded = [NSData dataWithBytesNoCopy:(void *)bytes length:length freeWhenDone:NO];
    source = CGImageSourceCreateWithData((__bridge CFDataRef)encoded,
                                         (__bridge CFDictionaryRef)
                                             @{(id)kCGImageSourceShouldCache : @NO});
    if (!source || !CGImageSourceGetCount(source) ||
        CGImageSourceGetStatusAtIndex(source, 0) != kCGImageStatusComplete)
      return nil;
    NSDictionary *properties =
        CFBridgingRelease(CGImageSourceCopyPropertiesAtIndex(source, 0, NULL));
    int64_t w = [properties[(id)kCGImagePropertyPixelWidth] longLongValue];
    int64_t h = [properties[(id)kCGImagePropertyPixelHeight] longLongValue];
    if (!texture_size(w, h))
      return nil;
    NSDictionary *options = @{
      (id)kCGImageSourceCreateThumbnailFromImageAlways : @YES,
      (id)kCGImageSourceCreateThumbnailWithTransform : @YES,
      (id)kCGImageSourceThumbnailMaxPixelSize : @(MAX(w, h)),
      (id)kCGImageSourceShouldCacheImmediately : @YES
    };
    image = CGImageSourceCreateThumbnailAtIndex(source, 0, (__bridge CFDictionaryRef)options);
    if (!image)
      return nil;
    *width = CGImageGetWidth(image);
    *height = CGImageGetHeight(image);
    if (!texture_size((int64_t)*width, (int64_t)*height))
      return nil;
    NSMutableData *pixels = [NSMutableData dataWithLength:*width * *height * 4];
    if (!pixels)
      _Exit(70);
    color = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    context = CGBitmapContextCreate(pixels.mutableBytes, *width, *height, 8, *width * 4, color,
                                    kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
    if (!context)
      return nil;
    CGContextSetBlendMode(context, kCGBlendModeCopy);
    CGContextDrawImage(context, CGRectMake(0, 0, *width, *height), image);
    return pixels;
  } @finally {
    if (context)
      CGContextRelease(context);
    if (color)
      CGColorSpaceRelease(color);
    if (image)
      CGImageRelease(image);
    if (source)
      CFRelease(source);
  }
}
int64_t rinzu_macos_texture_image(void *raw, const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(raw, RZ_DEVICE);
      if (!device)
        return missing();
      if (!accepting(device))
        return fail(RZ_BUSY, @"Session is closing");
      size_t width = 0, height = 0;
      NSData *pixels = decode_image(bytes, length, &width, &height);
      if (!pixels)
        return fail(RZ_ARGUMENT,
                    @"invalid, incomplete or oversized image (encoded <=64 MiB, sides <=8192)");
      RZMacTexture *texture = new_texture(device, (int64_t)width, (int64_t)height, 1, 1);
      if (!texture)
        return RZ_ARGUMENT;
      [texture.texture replaceRegion:MTLRegionMake2D(0, 0, width, height)
                         mipmapLevel:0
                           withBytes:pixels.bytes
                         bytesPerRow:width * 4];
      return publish(texture, RZ_TEXTURE, device.owner);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int64_t rinzu_macos_sampler_create(void *raw, int64_t filter, int64_t address) {
  @autoreleasepool {
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(raw, RZ_DEVICE);
      if (!device)
        return missing();
      if (!accepting(device))
        return fail(RZ_BUSY, @"Session is closing");
      if (filter < 0 || filter > 1 || address < 0 || address > 2)
        return fail(RZ_ARGUMENT, @"invalid sampler filter or address mode");
      MTLSamplerDescriptor *descriptor = [MTLSamplerDescriptor new];
      descriptor.minFilter = descriptor.magFilter =
          filter ? MTLSamplerMinMagFilterLinear : MTLSamplerMinMagFilterNearest;
      descriptor.sAddressMode = descriptor.tAddressMode =
          address == 0   ? MTLSamplerAddressModeClampToEdge
          : address == 1 ? MTLSamplerAddressModeRepeat
                         : MTLSamplerAddressModeMirrorRepeat;
      RZMacSampler *sampler = [RZMacSampler new];
      sampler.device = device;
      sampler.sampler = [device.device newSamplerStateWithDescriptor:descriptor];
      if (!sampler.sampler)
        _Exit(70);
      return publish(sampler, RZ_SAMPLER, device.owner);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_sampler(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_SAMPLER);
  }
}
int32_t rinzu_macos_sampler_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacSampler *sampler = (RZMacSampler *)lookup(raw, RZ_SAMPLER);
      if (!sampler)
        return missing();
      if (sampler.leases)
        return fail(RZ_BUSY, @"sampler is leased by a command");
      sampler.sampler = nil;
      sampler.device = nil;
      retire(sampler);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_render_texture(void *raw, void *texture_raw,
                                           const struct MacColor *clear) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      RZMacTexture *texture = (RZMacTexture *)lookup(texture_raw, RZ_TEXTURE);
      if (!command || !texture)
        return missing();
      if (command.device != texture.device || !(texture.usage & 2) || !clear ||
          !isfinite(clear->red) || !isfinite(clear->green) || !isfinite(clear->blue) ||
          !isfinite(clear->alpha))
        return fail(RZ_ARGUMENT, @"invalid render target, Device or clear color");
      if (texture.leases)
        return fail(RZ_BUSY, @"render target is leased by a command");
      int32_t status = begin_command(command);
      if (status)
        return status;
      MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
      pass.colorAttachments[0].texture = texture.texture;
      pass.colorAttachments[0].loadAction = MTLLoadActionClear;
      pass.colorAttachments[0].storeAction = MTLStoreActionStore;
      pass.colorAttachments[0].clearColor =
          MTLClearColorMake(clear->red, clear->green, clear->blue, clear->alpha);
      command.render = [command.command renderCommandEncoderWithDescriptor:pass];
      if (!command.render)
        _Exit(70);
      command.target = texture;
      command.targetWidth = texture.texture.width;
      command.targetHeight = texture.texture.height;
      command.targetFormat = texture.format;
      pin(command, texture);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_viewport(void *raw, const struct MacRect *rect) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      if (!command)
        return missing();
      if (command.state != 1 || !command.render)
        return fail(RZ_BUSY, @"command is not encoding a render pass");
      if (!rect || !isfinite(rect->x) || !isfinite(rect->y) || !isfinite(rect->width) ||
          !isfinite(rect->height) || rect->x < 0 || rect->y < 0 || rect->width <= 0 ||
          rect->height <= 0 || rect->x + rect->width > command.targetWidth ||
          rect->y + rect->height > command.targetHeight)
        return fail(RZ_ARGUMENT, @"viewport must be finite, positive and inside the target");
      [command.render setViewport:(MTLViewport){rect->x, rect->y, rect->width, rect->height, 0, 1}];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_scissor(void *raw, const struct MacScissor *rect) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      if (!command)
        return missing();
      if (command.state != 1 || !command.render)
        return fail(RZ_BUSY, @"command is not encoding a render pass");
      if (!rect || rect->width <= 0 || rect->height <= 0 ||
          !range_valid(command.targetWidth, rect->x, (uint64_t)rect->width) ||
          !range_valid(command.targetHeight, rect->y, (uint64_t)rect->height))
        return fail(RZ_ARGUMENT, @"scissor must be positive and inside the target");
      [command.render
          setScissorRect:(MTLScissorRect){(NSUInteger)rect->x, (NSUInteger)rect->y,
                                          (NSUInteger)rect->width, (NSUInteger)rect->height}];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_texture(void *raw, void *texture_raw, int64_t index, int32_t stage) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      RZMacTexture *texture = (RZMacTexture *)lookup(texture_raw, RZ_TEXTURE);
      if (!command || !texture)
        return missing();
      if (command.state != 1)
        return fail(RZ_BUSY, @"command is not recording");
      if (texture.device != command.device || index < 0 || index > 15 || stage < 0 || stage > 2 ||
          (stage == 0 ? !(texture.usage & 5) : !(texture.usage & 1)) ||
          (stage == 0 ? !command.compute : !command.render) || command.target == texture)
        return fail(RZ_ARGUMENT,
                    @"invalid texture Device, usage, stage, binding or render feedback");
      if (texture.leases && ![command.pins containsObject:texture])
        return fail(RZ_BUSY, @"texture is leased by another command");
      if (stage == 0)
        [command.compute setTexture:texture.texture atIndex:(NSUInteger)index];
      else if (stage == 1)
        [command.render setVertexTexture:texture.texture atIndex:(NSUInteger)index];
      else
        [command.render setFragmentTexture:texture.texture atIndex:(NSUInteger)index];
      pin(command, texture);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_command_sampler(void *raw, void *sampler_raw, int64_t index, int32_t stage) {
  @autoreleasepool {
    @try {
      RZMacCommand *command = (RZMacCommand *)lookup(raw, RZ_COMMAND);
      RZMacSampler *sampler = (RZMacSampler *)lookup(sampler_raw, RZ_SAMPLER);
      if (!command || !sampler)
        return missing();
      if (command.state != 1)
        return fail(RZ_BUSY, @"command is not recording");
      if (sampler.device != command.device || index < 0 || index > 15 || stage < 0 || stage > 2 ||
          (stage == 0 ? !command.compute : !command.render))
        return fail(RZ_ARGUMENT, @"invalid sampler Device, stage or binding");
      if (stage == 0)
        [command.compute setSamplerState:sampler.sampler atIndex:(NSUInteger)index];
      else if (stage == 1)
        [command.render setVertexSamplerState:sampler.sampler atIndex:(NSUInteger)index];
      else
        [command.render setFragmentSamplerState:sampler.sampler atIndex:(NSUInteger)index];
      pin(command, sampler);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_buffer_writable(void *raw) {
  @autoreleasepool {
    RZMacBuffer *buffer = (RZMacBuffer *)lookup(raw, RZ_BUFFER);
    if (!buffer)
      return missing();
    return buffer.leases ? fail(RZ_BUSY, @"buffer is leased by a command") : 0;
  }
}
static BOOL vertex_pack(const struct MacVertex2D *vertex, float *packed) {
  const double values[8] = {vertex->x,   vertex->y,     vertex->u,    vertex->v,
                            vertex->red, vertex->green, vertex->blue, vertex->alpha};
  for (unsigned i = 0; i < 8; ++i) {
    packed[i] = (float)values[i];
    if (!isfinite(values[i]) || !isfinite(packed[i]))
      return NO;
  }
  return YES;
}
int32_t rinzu_macos_buffer_vertex2d(void *raw, int64_t index, const struct MacVertex2D *vertex) {
  @autoreleasepool {
    @try {
      RZMacBuffer *buffer = (RZMacBuffer *)lookup(raw, RZ_BUFFER);
      if (!buffer)
        return missing();
      if (buffer.leases)
        return fail(RZ_BUSY, @"buffer is leased by a command");
      float packed[8];
      if (!vertex || index < 0 || (uint64_t)index >= buffer.length / sizeof(packed) ||
          !vertex_pack(vertex, packed))
        return fail(RZ_ARGUMENT, @"invalid 2D vertex or vertex buffer index");
      memcpy((uint8_t *)buffer.buffer.contents + (size_t)index * sizeof(packed), packed,
             sizeof(packed));
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_buffer_quad2d(void *raw, int64_t index, const struct MacQuad2D *quad) {
  @autoreleasepool {
    @try {
      RZMacBuffer *buffer = (RZMacBuffer *)lookup(raw, RZ_BUFFER);
      if (!buffer)
        return missing();
      if (buffer.leases)
        return fail(RZ_BUSY, @"buffer is leased by a command");
      float packed[6][8];
      if (!quad || index < 0 || (uint64_t)index >= buffer.length / sizeof(packed) ||
          !isfinite(quad->width) || !isfinite(quad->height) || quad->width <= 0 ||
          quad->height <= 0)
        return fail(RZ_ARGUMENT, @"invalid quad or quad buffer index");
      const unsigned corners[6][2] = {{0, 0}, {0, 1}, {1, 0}, {1, 0}, {0, 1}, {1, 1}};
      for (unsigned i = 0; i < 6; ++i) {
        double x = quad->x + corners[i][0] * quad->width;
        double y = quad->y + corners[i][1] * quad->height;
        struct MacVertex2D vertex = {quad->a * x + quad->c * y + quad->tx,
                                     quad->b * x + quad->d * y + quad->ty,
                                     quad->u + corners[i][0] * quad->uvWidth,
                                     quad->v + corners[i][1] * quad->uvHeight,
                                     quad->red,
                                     quad->green,
                                     quad->blue,
                                     quad->alpha};
        if (!vertex_pack(&vertex, packed[i]))
          return fail(RZ_ARGUMENT, @"quad coordinates, tint or transform are not finite float32");
      }
      memcpy((uint8_t *)buffer.buffer.contents + (size_t)index * sizeof(packed), packed,
             sizeof(packed));
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}

// Immutable CoreText layouts belong to the Session. Raster textures are
// independent snapshots, so GPU leases never retain a mutable text engine.
int64_t rinzu_macos_font_create(void *raw, const uint8_t *bytes, size_t length, double size,
                                _Bool bold) {
  @autoreleasepool {
    @try {
      RZMacSession *session = (RZMacSession *)lookup(raw, RZ_SESSION);
      if (!session)
        return missing();
      if (!accepting(session))
        return fail(RZ_BUSY, @"Session is closing");
      NSString *name = length <= 256 ? text(bytes, length) : nil;
      if (!name || !isfinite(size) || size < 1 || size > 512)
        return fail(RZ_ARGUMENT, @"invalid font name or point size (1..512)");
      NSFont *native =
          length
              ? [NSFont fontWithName:name size:size]
              : [NSFont systemFontOfSize:size weight:bold ? NSFontWeightBold : NSFontWeightRegular];
      if (!native)
        return fail(RZ_ARGUMENT, @"font was not found");
      if (length && bold)
        native = [[NSFontManager sharedFontManager] convertFont:native toHaveTrait:NSBoldFontMask];
      RZMacFont *font = [RZMacFont new];
      font->font = CTFontCreateWithName((__bridge CFStringRef)native.fontName, size, NULL);
      if (!font->font)
        _Exit(70);
      return publish(font, RZ_FONT, session);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_font(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_FONT);
  }
}
int32_t rinzu_macos_font_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacFont *font = (RZMacFont *)lookup(raw, RZ_FONT);
      if (!font)
        return missing();
      for (RZMacHandle *handle in handles.allValues)
        if (handle.kind == RZ_TEXT && ((RZMacText *)handle).font == font)
          return fail(RZ_BUSY, @"Font still owns text layouts");
      CFRelease(font->font);
      font->font = NULL;
      retire(font);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int64_t rinzu_macos_text_layout(void *raw, const uint8_t *bytes, size_t length, double width,
                                int64_t alignment) {
  @autoreleasepool {
    CTParagraphStyleRef paragraph = NULL;
    CTFramesetterRef setter = NULL;
    CGPathRef path = NULL;
    @try {
      RZMacFont *font = (RZMacFont *)lookup(raw, RZ_FONT);
      if (!font)
        return missing();
      if (!accepting(font))
        return fail(RZ_BUSY, @"Session is closing");
      NSString *plain = length <= 65536 ? text(bytes, length) : nil;
      if (!plain || !isfinite(width) || width < 0 || width > 8192 || alignment < 0 || alignment > 2)
        return fail(RZ_ARGUMENT, @"invalid text (<=64 KiB), wrap width (0..8192) or alignment");
      // A zero-width final character keeps an empty/trailing line measurable.
      NSString *content = !plain.length || [plain hasSuffix:@"\n"]
                              ? [plain stringByAppendingString:@"\u200b"]
                              : plain;
      CTTextAlignment align = alignment == 0   ? kCTTextAlignmentLeft
                              : alignment == 1 ? kCTTextAlignmentCenter
                                               : kCTTextAlignmentRight;
      CTLineBreakMode wrap = kCTLineBreakByWordWrapping;
      CTParagraphStyleSetting settings[] = {
          {kCTParagraphStyleSpecifierAlignment, sizeof(align), &align},
          {kCTParagraphStyleSpecifierLineBreakMode, sizeof(wrap), &wrap}};
      paragraph = CTParagraphStyleCreate(settings, 2);
      RZMacText *layout = [RZMacText new];
      layout.font = font;
      layout.bytes = (int64_t)length;
      layout.attributed = [[NSAttributedString alloc]
          initWithString:content
              attributes:@{
                (id)kCTFontAttributeName : (__bridge id)font->font,
                (id)kCTParagraphStyleAttributeName : (__bridge id)paragraph,
                (id)kCTForegroundColorAttributeName : (__bridge id)NSColor.whiteColor.CGColor
              }];
      setter = CTFramesetterCreateWithAttributedString(
          (__bridge CFAttributedStringRef)layout.attributed);
      if (!paragraph || !setter)
        _Exit(70);
      CFRange fitted = {0};
      CGSize suggested = CTFramesetterSuggestFrameSizeWithConstraints(
          setter, CFRangeMake(0, 0), NULL, CGSizeMake(width > 0 ? width : CGFLOAT_MAX, 8192),
          &fitted);
      if ((NSUInteger)fitted.length != layout.attributed.length || !isfinite(suggested.width) ||
          !isfinite(suggested.height) || suggested.height > 8192 || suggested.width > 8192)
        return fail(RZ_ARGUMENT, @"text exceeds the bounded layout area");
      double column = width > 0 ? width : MAX(1.0, ceil(suggested.width));
      layout.frameHeight = 8192;
      path = CGPathCreateWithRect(CGRectMake(0, 0, column, layout.frameHeight), NULL);
      layout->frame = CTFramesetterCreateFrame(setter, CFRangeMake(0, 0), path, NULL);
      if (!path || !layout->frame)
        _Exit(70);
      CFArrayRef lines = CTFrameGetLines(layout->frame);
      CFIndex count = CFArrayGetCount(lines);
      CFRange visible = CTFrameGetVisibleStringRange(layout->frame);
      if (count < 1 || count > 1024 || (NSUInteger)visible.length != layout.attributed.length)
        return fail(RZ_ARGUMENT, @"text exceeds 1024 lines or the layout bounds");
      CGPoint origins[1024];
      CTFrameGetLineOrigins(layout->frame, CFRangeMake(0, count), origins);
      CGFloat ascent = 0, descent = 0, leading = 0, lastDescent = 0;
      double measured = 0;
      CGRect ink = CGRectNull;
      for (CFIndex i = 0; i < count; ++i) {
        CGFloat a = 0, d = 0, l = 0;
        double advance = CTLineGetTypographicBounds(CFArrayGetValueAtIndex(lines, i), &a, &d, &l);
        measured = MAX(measured, advance);
        CGRect bounds = CTLineGetImageBounds(CFArrayGetValueAtIndex(lines, i), NULL);
        if (!CGRectIsNull(bounds) && !CGRectIsEmpty(bounds))
          ink = CGRectUnion(ink, CGRectOffset(bounds, origins[i].x, origins[i].y));
        if (!i) {
          ascent = a;
          descent = d;
          leading = l;
        }
        if (i == count - 1)
          lastDescent = d;
      }
      double height = ceil(layout.frameHeight - origins[count - 1].y + lastDescent);
      if (!isfinite(height) || height <= 0 || height > 8192)
        return fail(RZ_ARGUMENT, @"text height exceeds layout bounds");
      double logicalWidth = width > 0 ? column : ceil(measured);
      double left = -2, top = -2, right = logicalWidth + 2, bottom = height + 2;
      if (!CGRectIsNull(ink)) {
        left = MIN(left, floor(CGRectGetMinX(ink)) - 2);
        top = MIN(top, floor(layout.frameHeight - CGRectGetMaxY(ink)) - 2);
        right = MAX(right, ceil(CGRectGetMaxX(ink)) + 2);
        bottom = MAX(bottom, ceil(layout.frameHeight - CGRectGetMinY(ink)) + 2);
      }
      layout->metrics =
          (struct MacTextMetrics){logicalWidth, height,        layout.frameHeight - origins[0].y,
                                  ascent,       descent,       leading,
                                  left,         top,           right - left,
                                  bottom - top, (int64_t)count};
      return publish(layout, RZ_TEXT, font.owner);
    } @catch (NSException *exception) {
      return caught(exception);
    } @finally {
      if (path)
        CGPathRelease(path);
      if (setter)
        CFRelease(setter);
      if (paragraph)
        CFRelease(paragraph);
    }
  }
}
void *rinzu_macos_text(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_TEXT);
  }
}
int32_t rinzu_macos_text_metrics(void *raw, struct MacTextMetrics *output) {
  @autoreleasepool {
    RZMacText *layout = (RZMacText *)lookup(raw, RZ_TEXT);
    if (!layout)
      return missing();
    if (!output)
      return fail(RZ_ARGUMENT, @"missing text metrics output");
    *output = layout->metrics;
    return 0;
  }
}
int64_t rinzu_macos_text_texture(void *device_raw, void *text_raw, double scale) {
  @autoreleasepool {
    CGColorSpaceRef color = NULL;
    CGContextRef context = NULL;
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(device_raw, RZ_DEVICE);
      RZMacText *layout = (RZMacText *)lookup(text_raw, RZ_TEXT);
      if (!device || !layout)
        return missing();
      if (!accepting(device))
        return fail(RZ_BUSY, @"Session is closing");
      if (device.owner != layout.owner || !isfinite(scale) || scale < 0.25 || scale > 4)
        return fail(RZ_ARGUMENT, @"invalid text Device or raster scale (0.25..4)");
      double w = ceil(layout->metrics.rasterWidth * scale),
             h = ceil(layout->metrics.rasterHeight * scale);
      if (!isfinite(w) || !isfinite(h) || w < 1 || h < 1 || w > 8192 || h > 8192 ||
          !texture_size((int64_t)w, (int64_t)h))
        return fail(RZ_ARGUMENT, @"text raster exceeds the Texture size limit");
      NSUInteger width = (NSUInteger)w, height = (NSUInteger)h;
      NSMutableData *pixels = [NSMutableData dataWithLength:width * height * 4];
      if (!pixels)
        _Exit(70);
      color = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
      context = CGBitmapContextCreate(pixels.mutableBytes, width, height, 8, width * 4, color,
                                      kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
      if (!context)
        _Exit(70);
      CGContextSetShouldAntialias(context, true);
      CGContextSetShouldSmoothFonts(context, false);
      CGContextSetTextMatrix(context, CGAffineTransformIdentity);
      CGContextTranslateCTM(context, -layout->metrics.left * scale,
                            height + layout->metrics.top * scale);
      CGContextScaleCTM(context, scale, scale);
      CGContextTranslateCTM(context, 0, -layout.frameHeight);
      CTFrameDraw(layout->frame, context);
      RZMacTexture *texture = new_texture(device, (int64_t)width, (int64_t)height, 1, 1);
      if (!texture)
        return RZ_ARGUMENT;
      [texture.texture replaceRegion:MTLRegionMake2D(0, 0, width, height)
                         mipmapLevel:0
                           withBytes:pixels.bytes
                         bytesPerRow:width * 4];
      return publish(texture, RZ_TEXTURE, device.owner);
    } @catch (NSException *exception) {
      return caught(exception);
    } @finally {
      if (context)
        CGContextRelease(context);
      if (color)
        CGColorSpaceRelease(color);
    }
  }
}
int32_t rinzu_macos_text_destroy(void *raw) {
  @autoreleasepool {
    @try {
      RZMacText *layout = (RZMacText *)lookup(raw, RZ_TEXT);
      if (!layout)
        return missing();
      CFRelease(layout->frame);
      layout->frame = NULL;
      layout.attributed = nil;
      layout.font = nil;
      retire(layout);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}

// A deadline scheduler, not a display-link callback. It holds native state only.
// One-shot timers exist only for an admitted reader with visible dirty/animated work.
static BOOL frame_visible(RZMacFrameClock *clock) {
  RZMacView *view = clock.view;
  NSWindow *window = view.view.window;
  CGSize pixels = ((CAMetalLayer *)view.view.layer).drawableSize;
  return clock && !clock.stopped && view && !view.closed && clock.owner.running && window &&
         window.visible && !window.miniaturized && !view.view.hiddenOrHasHiddenAncestor &&
         (window.occlusionState & NSWindowOcclusionStateVisible) &&
         view.view.bounds.size.width > 0 && view.view.bounds.size.height > 0 && pixels.width > 0 &&
         pixels.height > 0;
}
static void arm_frame(RZMacFrameClock *clock) {
  if (!clock || clock.stopped || !clock.token || clock.timer ||
      rinzu_native_completion_is_done(&clock->completion.base) ||
      (!clock.dirty && !clock.animating) || !frame_visible(clock))
    return;
  double now = CACurrentMediaTime();
  double deadline = MAX(now, clock.lastTick + MAX(clock.interval, clock.retryDelay));
  __weak RZMacFrameClock *weak = clock;
  NSTimer *timer = [NSTimer
      timerWithTimeInterval:MAX(0.0001, deadline - now)
                    repeats:NO
                      block:^(NSTimer *tick) {
                        (void)tick;
                        @autoreleasepool {
                          __attribute__((objc_precise_lifetime)) RZMacFrameClock *current = weak;
                          if (!current || current.closed)
                            return;
                          @try {
                            current.timer = nil;
                            if (!current.token ||
                                rinzu_native_completion_is_done(&current->completion.base) ||
                                !frame_visible(current) || (!current.dirty && !current.animating))
                              return;
                            CGSize size = ((CAMetalLayer *)current.view.view.layer).drawableSize;
                            if (size.width <= 0 || size.height <= 0)
                              return;
                            double stamp = CACurrentMediaTime();
                            if (current.ticks == INT64_MAX)
                              _Exit(70);
                            current->info = (struct MacFrameInfo){
                                ++current.ticks,
                                (int64_t)size.width,
                                (int64_t)size.height,
                                current.view.view.window.backingScaleFactor,
                                stamp - current.started,
                                current.lastTick > 0 ? stamp - current.lastTick : 0};
                            current.lastTick = stamp;
                            current.dirty = NO;
                            current.available = YES;
                            // LAST carrier access. Source may acknowledge/reuse immediately.
                            rinzu_native_completion_v2_publish(&current->completion);
                          } @catch (NSException *exception) {
                            current.failure = caught(exception);
                            stop_frame(current);
                          }
                        }
                      }];
  if (!timer)
    _Exit(70);
  timer.tolerance = MIN(clock.interval / 8, 0.002);
  clock.timer = timer;
  [[NSRunLoop mainRunLoop] addTimer:timer forMode:NSRunLoopCommonModes];
}
static void refresh_frame(RZMacFrameClock *clock, BOOL dirty) {
  if (!clock || clock.stopped)
    return;
  if (dirty) {
    if (clock.requests == INT64_MAX || clock.coalesced == INT64_MAX)
      _Exit(70);
    ++clock.requests;
    if (clock.dirty)
      ++clock.coalesced;
    clock.dirty = YES;
  }
  BOOL visible = frame_visible(clock);
  if (visible && !clock.wasVisible) {
    clock.dirty = YES;
    clock.retryDelay = 0;
  }
  clock.wasVisible = visible;
  if (!visible) {
    [clock.timer invalidate];
    clock.timer = nil;
  } else
    arm_frame(clock);
}
static void stop_frame(RZMacFrameClock *clock) {
  if (!clock)
    return;
  [clock.timer invalidate];
  clock.timer = nil;
  clock.stopped = YES;
  clock.available = NO;
  if (clock.token && !rinzu_native_completion_is_done(&clock->completion.base))
    rinzu_native_completion_v2_publish(&clock->completion);
}
static void stop_frames(RZMacSession *session, RZMacWindow *window) {
  for (RZMacHandle *handle in handles.allValues)
    if (handle.kind == RZ_FRAME_CLOCK && handle.owner == session) {
      RZMacFrameClock *clock = (RZMacFrameClock *)handle;
      if (!window || clock.view.rootWindow == window || clock.view.scopeWindow == window)
        stop_frame(clock);
    }
}
int64_t rinzu_macos_surface_scoped(void *device_raw, void *window_raw, const struct MacRect *rect) {
  @autoreleasepool {
    @try {
      RZMacDevice *device = (RZMacDevice *)lookup(device_raw, RZ_DEVICE);
      RZMacWindow *window = (RZMacWindow *)lookup(window_raw, RZ_WINDOW);
      if (!device || !window)
        return missing();
      if (device.owner != window.owner)
        return fail(RZ_ARGUMENT, @"surface belongs to another Session");
      if (!accepting(window))
        return fail(RZ_BUSY, @"Window is closing");
      int64_t id = create_view(device.owner, rect, device);
      if (id > 0)
        ((RZMacView *)lookup((void *)(uintptr_t)id, RZ_VIEW)).scopeWindow = window;
      return id;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int64_t rinzu_macos_frame_create(void *raw, int64_t fps) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      if (!view.device || fps < 1 || fps > 240)
        return fail(RZ_ARGUMENT, @"FrameClock requires a Metal surface and fps 1..240");
      if (!accepting(view) || view.clock)
        return fail(RZ_BUSY, @"surface is closing or already owns a FrameClock");
      NSUInteger count = 0;
      for (RZMacHandle *handle in handles.allValues)
        count += handle.kind == RZ_FRAME_CLOCK && handle.owner == view.owner;
      if (count >= 256)
        return fail(RZ_BUSY, @"Session FrameClock limit is 256");
      RZMacFrameClock *clock = [RZMacFrameClock new];
      clock.view = view;
      clock.interval = 1.0 / (double)fps;
      clock.started = CACurrentMediaTime();
      clock.dirty = YES;
      clock.requests = 1;
      rinzu_native_completion_v2_init(&clock->completion);
      int64_t id = publish(clock, RZ_FRAME_CLOCK, view.owner);
      view.clock = clock;
      return id;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
void *rinzu_macos_frame(int64_t identity) {
  @autoreleasepool {
    return by_id(identity, RZ_FRAME_CLOCK);
  }
}
int32_t rinzu_macos_frame_state(void *raw) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    if (!clock)
      return missing();
    return clock.stopped || (!clock.owner.running && clock.owner.hasRun) ? 2
           : clock.owner.running                                         ? 1
                                                                         : 0;
  }
}
int32_t rinzu_macos_frame_request(void *raw) {
  @autoreleasepool {
    @try {
      RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
      if (!clock)
        return missing();
      if (clock.stopped)
        return fail(RZ_BUSY, @"FrameClock is stopped");
      refresh_frame(clock, YES);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_frame_animating(void *raw, _Bool enabled) {
  @autoreleasepool {
    @try {
      RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
      if (!clock)
        return missing();
      if (clock.stopped)
        return fail(RZ_BUSY, @"FrameClock is stopped");
      clock.animating = enabled;
      if (!enabled && !clock.dirty) {
        [clock.timer invalidate];
        clock.timer = nil;
      }
      arm_frame(clock);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_frame_retry(void *raw) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    if (!clock)
      return missing();
    if (clock.stopped)
      return fail(RZ_BUSY, @"FrameClock is stopped");
    if (clock.retries == INT64_MAX)
      _Exit(70);
    ++clock.retries;
    clock.retryDelay = MIN(0.25, MAX(clock.interval, clock.retryDelay * 2));
    refresh_frame(clock, YES);
    return 0;
  }
}
int32_t rinzu_macos_frame_presented(void *raw) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    if (!clock)
      return missing();
    clock.retryDelay = 0;
    return 0;
  }
}
int32_t rinzu_macos_frame_statistics(void *raw, struct MacFrameStatistics *output) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    if (!clock)
      return missing();
    if (!output)
      return fail(RZ_ARGUMENT, @"missing FrameClock statistics output");
    *output = (struct MacFrameStatistics){
        clock.requests,
        clock.coalesced,
        clock.ticks,
        clock.retries,
        clock.token && !rinzu_native_completion_is_done(&clock->completion.base),
        frame_visible(clock),
        clock.animating,
        clock.stopped};
    return 0;
  }
}
int32_t rinzu_macos_frame_wait_begin(void *raw, int64_t token) {
  @autoreleasepool {
    @try {
      RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
      if (!clock)
        return missing();
      if (clock.token || token <= 0 || token > next_waiter ||
          (!clock.owner.running && !clock.stopped))
        return fail(RZ_BUSY, @"FrameClock requires a running Session and a unique reader");
      clock.token = token;
      clock.available = NO;
      rinzu_native_completion_v2_init(&clock->completion);
      if (clock.stopped)
        rinzu_native_completion_v2_publish(&clock->completion);
      else
        arm_frame(clock);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
_Bool rinzu_macos_frame_wait_owned(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    return clock && token > 0 && clock.token == token;
  }
}
void *rinzu_macos_frame_completion(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    return clock && token > 0 && clock.token == token ? &clock->completion : NULL;
  }
}
int32_t rinzu_macos_frame_take(void *raw, int64_t token, struct MacFrameInfo *output) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    if (!clock)
      return missing();
    if (token <= 0 || clock.token != token ||
        !rinzu_native_completion_is_done(&clock->completion.base))
      return fail(RZ_BUSY, @"FrameClock result is not physically published");
    if (!output)
      return fail(RZ_ARGUMENT, @"missing frame output");
    *output = clock->info;
    return clock.failure ? clock.failure : clock.available ? 1 : 0;
  }
}
int32_t rinzu_macos_frame_cancel(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    if (!clock)
      return missing();
    if (token <= 0 || clock.token != token)
      return fail(RZ_BUSY, @"FrameClock reader token mismatch");
    [clock.timer invalidate];
    clock.timer = nil;
    if (!rinzu_native_completion_is_done(&clock->completion.base)) {
      clock.available = NO;
      rinzu_native_completion_v2_publish(&clock->completion);
    }
    return 0;
  }
}
int32_t rinzu_macos_frame_finish(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    if (!clock)
      return missing();
    if (token <= 0 || clock.token != token ||
        !rinzu_native_completion_is_done(&clock->completion.base))
      return fail(RZ_BUSY, @"FrameClock finish requires physical publication");
    clock.token = 0;
    return 0;
  }
}
int32_t rinzu_macos_frame_stop(void *raw) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    if (!clock)
      return missing();
    stop_frame(clock);
    return 0;
  }
}
int32_t rinzu_macos_frame_stop_window(void *raw) {
  @autoreleasepool {
    RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
    if (!window)
      return missing();
    stop_frames(window.owner, window);
    return 0;
  }
}
int32_t rinzu_macos_frame_destroy(void *raw) {
  @autoreleasepool {
    RZMacFrameClock *clock = (RZMacFrameClock *)lookup(raw, RZ_FRAME_CLOCK);
    if (!clock)
      return missing();
    if (clock.token)
      return fail(RZ_BUSY, @"join FrameClock reader before destroy");
    stop_frame(clock);
    clock.view.clock = nil;
    clock.view = nil;
    retire(clock);
    return 0;
  }
}

int32_t rinzu_macos_window_minimum_size(void *raw, double width, double height) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      if (!window)
        return missing();
      if (!isfinite(width) || !isfinite(height) || width <= 0 || height <= 0 || width > 32768 ||
          height > 32768)
        return fail(RZ_ARGUMENT, @"invalid minimum content size");
      window.window.contentMinSize = NSMakeSize(width, height);
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}

// Native models contain copies only. AppKit never calls a Source datasource.
@implementation RZMacDataNode
@end
@implementation RZMacDataModel
- (instancetype)init {
  if ((self = [super init])) {
    _rows = [NSMutableArray new];
    _roots = [NSMutableArray new];
    _byId = [NSMutableDictionary new];
  }
  return self;
}
@end
@interface RZMacDataTable : NSTableView
@property(nonatomic, weak) RZMacDataController *control;
@end
@interface RZMacDataTree : NSOutlineView
@property(nonatomic, weak) RZMacDataController *control;
@end
static BOOL data_key(RZMacDataController *data, NSEvent *event) {
  if (!data.view.enabled || !accepting(data.view))
    return YES;
  if ((event.keyCode == 36 || event.keyCode == 76) &&
      !(event.modifierFlags & (NSEventModifierFlagCommand | NSEventModifierFlagControl))) {
    [data emit:2];
    return YES;
  }
  return NO;
}
@implementation RZMacDataTable
- (void)keyDown:(NSEvent *)event {
  if (!data_key(self.control, event))
    [super keyDown:event];
}
@end
@implementation RZMacDataTree
- (void)keyDown:(NSEvent *)event {
  if (!data_key(self.control, event))
    [super keyDown:event];
}
@end
@implementation RZMacDataController
- (RZMacDataNode *)nodeAtRow:(NSInteger)row {
  if (row < 0 ||
      (self.tree ? row >= self.table.numberOfRows : (uint64_t)row >= self.model.roots.count))
    return nil;
  return self.tree ? [(NSOutlineView *)self.table itemAtRow:row]
                   : self.model.roots[(NSUInteger)row];
}
- (NSInteger)rowForNode:(RZMacDataNode *)node {
  if (!node)
    return -1;
  if (self.tree)
    return [(NSOutlineView *)self.table rowForItem:node];
  NSUInteger row = [self.model.roots indexOfObjectIdenticalTo:node];
  return row == NSNotFound ? -1 : (NSInteger)row;
}
- (int64_t)selectedId {
  return [self nodeAtRow:self.table.selectedRow].identity;
}
- (NSInteger)numberOfRowsInTableView:(NSTableView *)table {
  (void)table;
  return (NSInteger)self.model.roots.count;
}
- (NSInteger)outlineView:(NSOutlineView *)outline numberOfChildrenOfItem:(id)item {
  (void)outline;
  return (NSInteger)(item ? ((RZMacDataNode *)item).children.count : self.model.roots.count);
}
- (id)outlineView:(NSOutlineView *)outline child:(NSInteger)index ofItem:(id)item {
  (void)outline;
  return item ? ((RZMacDataNode *)item).children[(NSUInteger)index]
              : self.model.roots[(NSUInteger)index];
}
- (BOOL)outlineView:(NSOutlineView *)outline isItemExpandable:(id)item {
  (void)outline;
  return ((RZMacDataNode *)item).children.count > 0;
}
- (NSView *)cell:(NSTableColumn *)column node:(RZMacDataNode *)node {
  if (!node || !column || column.identifier.integerValue < 0 ||
      (uint64_t)column.identifier.integerValue >= node.cells.count)
    return nil;
  NSTableCellView *cell = [self.table makeViewWithIdentifier:column.identifier owner:nil];
  if (!cell) {
    cell = [[NSTableCellView alloc] initWithFrame:NSMakeRect(0, 0, column.width, 24)];
    cell.identifier = column.identifier;
    NSTextField *label = [NSTextField labelWithString:@""];
    label.frame = NSMakeRect(4, 3, MAX(0, column.width - 8), 18);
    label.autoresizingMask = NSViewWidthSizable;
    label.lineBreakMode = NSLineBreakByTruncatingTail;
    cell.textField = label;
    [cell addSubview:label];
    [self.cells addObject:cell];
  }
  NSString *value = node.cells[(NSUInteger)column.identifier.integerValue];
  cell.textField.stringValue = value;
  cell.textField.toolTip = value;
  return cell;
}
- (NSView *)tableView:(NSTableView *)table
    viewForTableColumn:(NSTableColumn *)column
                   row:(NSInteger)row {
  (void)table;
  return [self cell:column node:[self nodeAtRow:row]];
}
- (NSView *)outlineView:(NSOutlineView *)outline
     viewForTableColumn:(NSTableColumn *)column
                   item:(id)item {
  (void)outline;
  return [self cell:column node:item];
}
- (BOOL)tableView:(NSTableView *)table shouldSelectRow:(NSInteger)row {
  (void)table;
  (void)row;
  return self.view.enabled && accepting(self.view);
}
- (BOOL)outlineView:(NSOutlineView *)outline shouldSelectItem:(id)item {
  (void)outline;
  (void)item;
  return self.view.enabled && accepting(self.view);
}
- (BOOL)outlineView:(NSOutlineView *)outline shouldExpandItem:(id)item {
  (void)outline;
  (void)item;
  return self.updating || (self.view.enabled && accepting(self.view));
}
- (BOOL)outlineView:(NSOutlineView *)outline shouldCollapseItem:(id)item {
  (void)outline;
  (void)item;
  return self.updating || (self.view.enabled && accepting(self.view));
}
- (void)emit:(int64_t)kind {
  if (self.updating || !self.view || self.view.closed || !self.view.enabled ||
      !accepting(self.view))
    return;
  RZMacDataNode *node = [self nodeAtRow:self.table.selectedRow];
  struct MacEvent event = view_event(self.view, RZ_DATA);
  event.action = self.view.action;
  event.flags = kind;
  event.selectionStart = node.identity;
  event.selectionEnd = self.table.selectedRow;
  event.value = (double)self.revision;
  [self.view.owner enqueue:event
                      text:[node.cells.firstObject dataUsingEncoding:NSUTF8StringEncoding]];
}
- (void)tableViewSelectionDidChange:(NSNotification *)notification {
  (void)notification;
  [self emit:1];
}
- (void)outlineViewSelectionDidChange:(NSNotification *)notification {
  (void)notification;
  [self emit:1];
}
- (void)activated:(id)sender {
  (void)sender;
  if (self.table.selectedRow >= 0)
    [self emit:2];
}
- (void)outlineViewItemDidExpand:(NSNotification *)notification {
  RZMacDataNode *node = notification.userInfo[@"NSObject"];
  [self.expanded addObject:@(node.identity)];
  if (!self.updating && self.view.enabled && accepting(self.view)) {
    struct MacEvent event = view_event(self.view, RZ_DATA);
    event.action = self.view.action;
    event.flags = 3;
    event.selectionStart = node.identity;
    event.selectionEnd = [self rowForNode:node];
    event.value = (double)self.revision;
    [self.view.owner enqueue:event
                        text:[node.cells.firstObject dataUsingEncoding:NSUTF8StringEncoding]];
  }
}
- (void)outlineViewItemDidCollapse:(NSNotification *)notification {
  RZMacDataNode *node = notification.userInfo[@"NSObject"];
  [self.expanded removeObject:@(node.identity)];
  if (!self.updating && self.view.enabled && accepting(self.view)) {
    struct MacEvent event = view_event(self.view, RZ_DATA);
    event.action = self.view.action;
    event.flags = 4;
    event.selectionStart = node.identity;
    event.selectionEnd = [self rowForNode:node];
    event.value = (double)self.revision;
    [self.view.owner enqueue:event
                        text:[node.cells.firstObject dataUsingEncoding:NSUTF8StringEncoding]];
  }
}
- (void)sortModel {
  if (self.sortColumn < 0)
    return;
  NSUInteger column = (NSUInteger)self.sortColumn;
  BOOL ascending = self.ascending;
  NSComparator compare = ^NSComparisonResult(RZMacDataNode *a, RZMacDataNode *b) {
    NSComparisonResult result = [a.cells[column] compare:b.cells[column]
                                                 options:NSCaseInsensitiveSearch | NSNumericSearch];
    if (result == NSOrderedSame)
      result = a.identity < b.identity   ? NSOrderedAscending
               : a.identity > b.identity ? NSOrderedDescending
                                         : NSOrderedSame;
    return ascending ? result : -result;
  };
  [self.model.roots sortUsingComparator:compare];
  if (self.tree)
    for (RZMacDataNode *node in self.model.rows)
      [node.children sortUsingComparator:compare];
}
- (void)reloadKeeping:(int64_t)identity {
  [self.table reloadData];
  if (self.tree) {
    // Parent-before-child insertion order restores only reachable expanded items.
    for (RZMacDataNode *node in self.model.rows)
      if ([self.expanded containsObject:@(node.identity)] &&
          (!node.parent || [(NSOutlineView *)self.table isItemExpanded:node.parent]))
        [(NSOutlineView *)self.table expandItem:node];
    [self.expanded intersectSet:[NSSet setWithArray:self.model.byId.allKeys]];
  }
  NSInteger row = [self rowForNode:self.model.byId[@(identity)]];
  if (row >= 0)
    [self.table selectRowIndexes:[NSIndexSet indexSetWithIndex:(NSUInteger)row]
            byExtendingSelection:NO];
  else
    [self.table deselectAll:nil];
}
- (void)tableView:(NSTableView *)table sortDescriptorsDidChange:(NSArray<NSSortDescriptor *> *)old {
  (void)old;
  if (self.updating || !self.view.enabled || !accepting(self.view) || !table.sortDescriptors.count)
    return;
  NSSortDescriptor *sort = table.sortDescriptors.firstObject;
  NSInteger column = sort.key.integerValue;
  if (column < 0 || column >= (NSInteger)table.tableColumns.count)
    return;
  int64_t selected = self.selectedId;
  self.sortColumn = column;
  self.ascending = sort.ascending;
  self.updating = YES;
  @try {
    [self sortModel];
    [self reloadKeeping:selected];
  } @finally {
    self.updating = NO;
  }
  struct MacEvent event = view_event(self.view, RZ_DATA);
  event.action = self.view.action;
  event.flags = 5;
  event.selectionStart = column;
  event.selectionEnd = -1;
  event.modifiers = sort.ascending;
  event.value = (double)self.revision;
  [self.view.owner enqueue:event];
}
- (void)outlineView:(NSOutlineView *)outline
    sortDescriptorsDidChange:(NSArray<NSSortDescriptor *> *)old {
  [self tableView:outline sortDescriptorsDidChange:old];
}
@end

static RZMacDataController *data_control(void *raw) {
  return ((RZMacView *)lookup(raw, RZ_VIEW)).dataController;
}
int64_t rinzu_macos_data_create(void *raw, int64_t kind, int64_t columns,
                                const struct MacRect *rect, int64_t action) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      if (!window)
        return missing();
      if (!accepting(window))
        return fail(RZ_BUSY, @"Window is closing");
      if (kind < 1 || kind > 3 || columns < 1 || columns > 32 || (kind == 1 && columns != 1) ||
          !rect_valid(rect) || action < 0)
        return fail(RZ_ARGUMENT, @"invalid data view kind, columns, frame or action");
      RZMacView *view = [RZMacView new];
      view.enabled = YES;
      view.widgetKind = 7;
      view.action = action;
      view.scopeWindow = window;
      NSScrollView *scroll = [[NSScrollView alloc] initWithFrame:nsrect(rect)];
      scroll.hasVerticalScroller = YES;
      scroll.hasHorizontalScroller = YES;
      scroll.borderType = NSBezelBorder;
      RZMacDataController *data = [RZMacDataController new];
      data.tree = kind == 3;
      data.model = [RZMacDataModel new];
      data.view = view;
      data.expanded = [NSMutableSet new];
      data.cells = [NSHashTable weakObjectsHashTable];
      data.sortColumn = -1;
      NSTableView *table;
      if (data.tree) {
        RZMacDataTree *tree = [[RZMacDataTree alloc] initWithFrame:scroll.bounds];
        tree.control = data;
        tree.indentationPerLevel = 14;
        tree.autoresizesOutlineColumn = NO;
        table = tree;
      } else {
        RZMacDataTable *flat = [[RZMacDataTable alloc] initWithFrame:scroll.bounds];
        flat.control = data;
        table = flat;
      }
      data.table = table;
      table.rowHeight = 24;
      table.usesAutomaticRowHeights = NO;
      table.allowsMultipleSelection = NO;
      table.allowsEmptySelection = YES;
      table.allowsColumnReordering = NO;
      table.usesAlternatingRowBackgroundColors = YES;
      table.columnAutoresizingStyle = NSTableViewLastColumnOnlyAutoresizingStyle;
      table.autoresizingMask = NSViewWidthSizable;
      for (int64_t i = 0; i < columns; ++i) {
        NSTableColumn *column = [[NSTableColumn alloc]
            initWithIdentifier:[NSString stringWithFormat:@"%lld", (long long)i]];
        column.title = @"";
        column.width = 160;
        column.minWidth = 16;
        column.maxWidth = 4096;
        [table addTableColumn:column];
      }
      if (data.tree)
        ((NSOutlineView *)table).outlineTableColumn = table.tableColumns.firstObject;
      if (kind == 1)
        table.headerView = nil;
      table.delegate = data;
      table.dataSource = data;
      table.target = data;
      table.doubleAction = @selector(activated:);
      scroll.documentView = table;
      view.view = scroll;
      view.dataController = data;
      return publish(view, RZ_VIEW, window.owner);
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_data_column(void *raw, int64_t index, const uint8_t *bytes, size_t length,
                                double width, _Bool sortable) {
  @autoreleasepool {
    @try {
      RZMacDataController *data = data_control(raw);
      if (!data)
        return missing();
      NSString *title = length <= 4096 ? text(bytes, length) : nil;
      if (!title || index < 0 || (uint64_t)index >= data.table.tableColumns.count ||
          !isfinite(width) || width < 16 || width > 4096)
        return fail(RZ_ARGUMENT, @"invalid data column");
      if (data.model.rows.count || data.token)
        return fail(RZ_BUSY, @"configure columns before loading rows");
      NSTableColumn *column = data.table.tableColumns[(NSUInteger)index];
      data.columnBytes +=
          (int64_t)length - (int64_t)[column.title lengthOfBytesUsingEncoding:NSUTF8StringEncoding];
      column.title = title;
      column.width = width;
      column.sortDescriptorPrototype =
          sortable ? [NSSortDescriptor sortDescriptorWithKey:column.identifier ascending:YES] : nil;
      if ((NSUInteger)index + 1 == data.table.tableColumns.count)
        [data.table sizeLastColumnToFit];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_data_begin(void *raw, int64_t token) {
  @autoreleasepool {
    @try {
      RZMacDataController *data = data_control(raw);
      if (!data)
        return missing();
      if (!accepting(data.view) || data.token)
        return fail(RZ_BUSY, @"data view is closing or already updating");
      if (token <= 0 || token > next_waiter)
        return fail(RZ_ARGUMENT, @"invalid data update token");
      data.pending = [RZMacDataModel new];
      data.token = token;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
_Bool rinzu_macos_data_owned(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacDataController *data = data_control(raw);
    return data && token > 0 && data.token == token;
  }
}
int32_t rinzu_macos_data_append(void *raw, int64_t token, int64_t identity, int64_t parent,
                                const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacDataController *data = data_control(raw);
      if (!data)
        return missing();
      if (token <= 0 || data.token != token)
        return fail(RZ_BUSY, @"data update token mismatch");
      RZMacDataModel *model = data.pending;
      RZMacDataNode *ancestor = parent ? model.byId[@(parent)] : nil;
      if (identity <= 0 || model.byId[@(identity)] || parent < 0 ||
          (parent && (!data.tree || !ancestor)) || (ancestor && ancestor.depth >= 63) ||
          model.rows.count >= 100000 || length > 1048576 || (length && !bytes))
        return fail(RZ_ARGUMENT, @"invalid row ID, parent, depth or row limit");
      NSMutableArray<NSString *> *cells = [NSMutableArray new];
      size_t offset = 0;
      int64_t payload = 0;
      for (NSUInteger i = 0; i < data.table.tableColumns.count; ++i) {
        size_t size = 0, digits = 0;
        while (offset < length && bytes[offset] >= '0' && bytes[offset] <= '9') {
          if (++digits > 6)
            return fail(RZ_ARGUMENT, @"invalid cell length");
          size = size * 10 + (size_t)(bytes[offset++] - '0');
        }
        if (!digits || offset >= length || bytes[offset++] != ':' || size > 65536 ||
            size > length - offset)
          return fail(RZ_ARGUMENT, @"invalid cell payload");
        NSString *cell = text(bytes + offset, size);
        if (!cell)
          return fail(RZ_ARGUMENT, @"invalid UTF8 data cell");
        [cells addObject:cell];
        offset += size;
        payload += (int64_t)size;
      }
      if (offset != length || model.bytes + payload > 16777216)
        return fail(RZ_ARGUMENT, @"column count or 16 MiB data limit exceeded");
      RZMacDataNode *node = [RZMacDataNode new];
      node.identity = identity;
      node.cells = cells;
      node.parent = ancestor;
      node.depth = ancestor ? ancestor.depth + 1 : 0;
      node.children = [NSMutableArray new];
      [model.rows addObject:node];
      model.byId[@(identity)] = node;
      if (ancestor)
        [ancestor.children addObject:node];
      else
        [model.roots addObject:node];
      model.bytes += payload;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_data_commit(void *raw, int64_t token) {
  @autoreleasepool {
    @try {
      RZMacDataController *data = data_control(raw);
      if (!data)
        return missing();
      if (token <= 0 || data.token != token)
        return fail(RZ_BUSY, @"data update token mismatch");
      if (!accepting(data.view) || data.revision >= INT64_C(9007199254740991))
        return fail(RZ_BUSY, @"data view is closing or revision exhausted");
      int64_t selected = data.selectedId;
      data.updating = YES;
      @try {
        data.model = data.pending;
        data.pending = nil;
        data.token = 0;
        ++data.revision;
        [data sortModel];
        [data reloadKeeping:selected];
      } @finally {
        data.updating = NO;
      }
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_data_cancel(void *raw, int64_t token) {
  @autoreleasepool {
    RZMacDataController *data = data_control(raw);
    if (!data)
      return missing();
    if (token <= 0 || data.token != token)
      return fail(RZ_BUSY, @"data update token mismatch");
    data.pending = nil;
    data.token = 0;
    return 0;
  }
}
static void data_reveal(RZMacDataController *data, RZMacDataNode *node) {
  if (!data.tree || !node.parent)
    return;
  data_reveal(data, node.parent);
  [(NSOutlineView *)data.table expandItem:node.parent];
}
int32_t rinzu_macos_data_select(void *raw, int64_t identity, _Bool reveal) {
  @autoreleasepool {
    @try {
      RZMacDataController *data = data_control(raw);
      if (!data)
        return missing();
      RZMacDataNode *node = data.model.byId[@(identity)];
      if (identity && !node)
        return fail(RZ_ARGUMENT, @"row ID not found");
      data.updating = YES;
      @try {
        if (reveal)
          data_reveal(data, node);
        NSInteger row = [data rowForNode:node];
        if (identity && row < 0)
          return fail(RZ_ARGUMENT, @"row is collapsed; reveal it first");
        if (row < 0)
          [data.table deselectAll:nil];
        else {
          [data.table selectRowIndexes:[NSIndexSet indexSetWithIndex:(NSUInteger)row]
                  byExtendingSelection:NO];
          if (reveal)
            [data.table scrollRowToVisible:row];
        }
      } @finally {
        data.updating = NO;
      }
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_data_scroll(void *raw, int64_t identity) {
  @autoreleasepool {
    @try {
      RZMacDataController *data = data_control(raw);
      if (!data)
        return missing();
      RZMacDataNode *node = data.model.byId[@(identity)];
      if (!node)
        return fail(RZ_ARGUMENT, @"row ID not found");
      data.updating = YES;
      @try {
        data_reveal(data, node);
        [data.table scrollRowToVisible:[data rowForNode:node]];
      } @finally {
        data.updating = NO;
      }
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_data_expand(void *raw, int64_t identity, _Bool expanded) {
  @autoreleasepool {
    @try {
      RZMacDataController *data = data_control(raw);
      if (!data)
        return missing();
      RZMacDataNode *node = data.model.byId[@(identity)];
      if (!data.tree || !node)
        return fail(RZ_ARGUMENT, @"tree row ID not found");
      data.updating = YES;
      @try {
        if (expanded) {
          data_reveal(data, node);
          [(NSOutlineView *)data.table expandItem:node];
        } else
          [(NSOutlineView *)data.table collapseItem:node];
      } @finally {
        data.updating = NO;
      }
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_data_expanded(void *raw, int64_t identity) {
  @autoreleasepool {
    RZMacDataController *data = data_control(raw);
    if (!data)
      return missing();
    RZMacDataNode *node = data.model.byId[@(identity)];
    if (!data.tree || !node)
      return fail(RZ_ARGUMENT, @"tree row ID not found");
    return [(NSOutlineView *)data.table isItemExpanded:node];
  }
}
int32_t rinzu_macos_data_sort(void *raw, int64_t column, _Bool ascending) {
  @autoreleasepool {
    @try {
      RZMacDataController *data = data_control(raw);
      if (!data)
        return missing();
      if (column < 0 || (uint64_t)column >= data.table.tableColumns.count)
        return fail(RZ_ARGUMENT, @"sort column not found");
      int64_t selected = data.selectedId;
      data.updating = YES;
      @try {
        data.sortColumn = (NSInteger)column;
        data.ascending = ascending;
        data.table.sortDescriptors = @[ [NSSortDescriptor
            sortDescriptorWithKey:[NSString stringWithFormat:@"%lld", (long long)column]
                        ascending:ascending] ];
        [data sortModel];
        [data reloadKeeping:selected];
      } @finally {
        data.updating = NO;
      }
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_data_state(void *raw, struct MacDataState *output) {
  @autoreleasepool {
    @try {
      RZMacDataController *data = data_control(raw);
      if (!data)
        return missing();
      if (!output)
        return fail(RZ_ARGUMENT, @"missing data state output");
      NSRange range = [data.table rowsInRect:data.table.visibleRect];
      *output = (struct MacDataState){(int64_t)data.model.rows.count,
                                      data.table.numberOfRows,
                                      (int64_t)data.cells.allObjects.count,
                                      data.model.bytes,
                                      data.revision,
                                      data.selectedId,
                                      data.table.selectedRow,
                                      range.length ? (int64_t)range.location : -1,
                                      range.length ? (int64_t)NSMaxRange(range) - 1 : -1,
                                      (int64_t)data.pending.rows.count};
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_data_cell(void *raw, int64_t token, int64_t identity, int64_t column) {
  @autoreleasepool {
    @try {
      RZMacDataController *data = data_control(raw);
      if (!data)
        return missing();
      RZMacDataNode *node = data.model.byId[@(identity)];
      if (!node || column < 0 || (uint64_t)column >= node.cells.count || token <= 0 ||
          token > next_waiter)
        return fail(RZ_ARGUMENT, @"invalid row, column or text token");
      RZMacView *view = data.view;
      if (view.snapshotToken)
        return fail(RZ_BUSY, @"data cell has an active reader");
      NSData *bytes = [node.cells[(NSUInteger)column] dataUsingEncoding:NSUTF8StringEncoding];
      if (!bytes)
        _Exit(70);
      view.snapshotData = bytes;
      view.snapshotToken = token;
      ++view.leases;
      view->textSnapshot =
          (rinzu_native_byte_array_t){.data = (uint8_t *)bytes.bytes, .length = bytes.length};
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_view_accessibility(void *raw, const uint8_t *label_bytes, size_t label_length,
                                       const uint8_t *help_bytes, size_t help_length,
                                       const uint8_t *id_bytes, size_t id_length) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      NSString *label = label_length <= 4096 ? text(label_bytes, label_length) : nil;
      NSString *help = help_length <= 4096 ? text(help_bytes, help_length) : nil;
      NSString *identifier = id_length <= 256 ? text(id_bytes, id_length) : nil;
      if (!label || !help || !identifier)
        return fail(RZ_ARGUMENT, @"invalid accessibility text");
      NSView *target = view.dataController ? view.dataController.table : view.view;
      target.accessibilityLabel = label;
      target.accessibilityHelp = help;
      target.accessibilityIdentifier = identifier;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_view_tooltip(void *raw, const uint8_t *bytes, size_t length) {
  @autoreleasepool {
    @try {
      RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
      if (!view)
        return missing();
      NSString *value = length <= 4096 ? text(bytes, length) : nil;
      if (!value)
        return fail(RZ_ARGUMENT, @"invalid tooltip");
      view.view.toolTip = value;
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_window_theme(void *raw, int64_t theme) {
  @autoreleasepool {
    @try {
      RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
      if (!window)
        return missing();
      if (theme < 0 || theme > 2)
        return fail(RZ_ARGUMENT, @"theme must be system, light or dark");
      window.window.appearance =
          theme == 0 ? nil
                     : [NSAppearance appearanceNamed:theme == 1 ? NSAppearanceNameAqua
                                                                : NSAppearanceNameDarkAqua];
      return 0;
    } @catch (NSException *exception) {
      return caught(exception);
    }
  }
}
int32_t rinzu_macos_window_effective_theme(void *raw) {
  @autoreleasepool {
    RZMacWindow *window = (RZMacWindow *)lookup(raw, RZ_WINDOW);
    if (!window)
      return missing();
    NSString *match = [window.window.effectiveAppearance
        bestMatchFromAppearancesWithNames:@[ NSAppearanceNameAqua, NSAppearanceNameDarkAqua ]];
    return [match isEqualToString:NSAppearanceNameDarkAqua] ? 2 : 1;
  }
}

#include "rinzu/callback_entry.h"
#include "rinzu/foreign_queue.h"
#include "rinzu/native_diagnostics.h"
#include "rinzu/ready_work.h"
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include "rinzu/thread_identity.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum {
  RINZU_CALLBACK_PAGE = 16384,
  RINZU_CALLBACK_STRIDE = 32,
  RINZU_CALLBACK_SLOTS = RINZU_CALLBACK_PAGE / RINZU_CALLBACK_STRIDE
};
extern const unsigned char rinzu_callback_template[], rinzu_callback_template_end[];
extern void rinzu_callback_pointer_u64_bridge(void);
extern void rinzu_callback_frame_bridge(void);
typedef struct {
  void *control;
  void (*bridge)(void);
  uintptr_t reserved[2];
} Slot;
typedef struct Block {
  mach_vm_address_t base;
  size_t used;
  struct Block *next;
} Block;
enum { PREPARED, OPEN, CLOSING, CLOSED };
typedef struct {
  rinzu_activity_board_t *board;
  uint64_t file, owner, depth;
  uint32_t start_byte, end_byte;
  char symbol[];
} CallbackDiagnostic;
struct rinzu_callback_control {
  pthread_mutex_t gate;
  pthread_t origin;
  int state;
  size_t in_flight;
  rinzu_callback_pointer_u64_handler_t handler;
  rinzu_callback_frame_handler_t frame_handler;
  void *context;
  rinzu_foreign_queue_t *foreign_queue;
  uint64_t foreign_generation;
  unsigned foreign_eligible;
  CallbackDiagnostic *diagnostic;
  Slot *slot;
};
static pthread_mutex_t pool_gate = PTHREAD_MUTEX_INITIALIZER;
static Block *blocks;
static size_t entry_count, block_count;
static size_t closed_count, diagnostic_live, diagnostic_bytes;
static size_t retired_count;
static _Atomic size_t live_connections, in_flight, invocations;
static _Thread_local unsigned foreign_executing;
int rinzu_callback_foreign_executing(void) {
  return foreign_executing != 0;
}
_Static_assert(sizeof(Slot) == RINZU_CALLBACK_STRIDE, "callback control slot ABI");
_Static_assert(sizeof(rinzu_callback_frame_t) == 136 &&
                   offsetof(rinzu_callback_frame_t, stack) == 128,
               "callback saved frame ABI");

#ifdef RINZU_CALLBACK_TESTING
static size_t allocation_budget = SIZE_MAX;
static int mapping_failure;
static int merged_control_region;
void rinzu_callback_test_fail_allocation_after(size_t successes) {
  allocation_budget = successes;
}
void rinzu_callback_test_fail_mapping(int category) {
  mapping_failure = category;
}
void rinzu_callback_test_merge_control_region(void) {
  merged_control_region = 1;
}
#endif
static void lock(pthread_mutex_t *gate) {
  if (pthread_mutex_lock(gate))
    _Exit(86);
}
static void unlock(pthread_mutex_t *gate) {
  if (pthread_mutex_unlock(gate))
    _Exit(86);
}
static void *allocate(size_t size) {
#ifdef RINZU_CALLBACK_TESTING
  if (!allocation_budget)
    _Exit(71);
  if (allocation_budget != SIZE_MAX)
    allocation_budget--;
#endif
  void *memory = calloc(1, size);
  if (!memory)
    _Exit(71);
  return memory;
}
static void vm_check(kern_return_t result) {
  if (result != KERN_SUCCESS)
    _Exit(result == KERN_RESOURCE_SHORTAGE || result == KERN_NO_SPACE ? 71 : 78);
}
static Block *grow(void) {
  if (getpagesize() != RINZU_CALLBACK_PAGE ||
      (uintptr_t)rinzu_callback_template % RINZU_CALLBACK_PAGE ||
      (uintptr_t)rinzu_callback_template_end - (uintptr_t)rinzu_callback_template !=
          RINZU_CALLBACK_PAGE)
    _Exit(78);
  Block *block = allocate(sizeof(*block));
#ifdef RINZU_CALLBACK_TESTING
  if (mapping_failure)
    vm_check(mapping_failure == 1 ? KERN_RESOURCE_SHORTAGE : KERN_PROTECTION_FAILURE);
#endif
  size_t prefix = 0;
#ifdef RINZU_CALLBACK_TESTING
  if (merged_control_region) {
    prefix = RINZU_CALLBACK_PAGE;
    merged_control_region = 0;
  }
#endif
  vm_check(mach_vm_allocate(mach_task_self(), &block->base, RINZU_CALLBACK_PAGE * 2 + prefix,
                            VM_FLAGS_ANYWHERE));
  block->base += prefix;
  mach_vm_address_t target = block->base + RINZU_CALLBACK_PAGE;
  vm_prot_t current, maximum;
  vm_check(mach_vm_remap(mach_task_self(), &target, RINZU_CALLBACK_PAGE, 0,
                         VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE, mach_task_self(),
                         (mach_vm_address_t)rinzu_callback_template, FALSE, &current, &maximum,
                         VM_INHERIT_NONE));
  if (target != block->base + RINZU_CALLBACK_PAGE ||
      (current & (VM_PROT_READ | VM_PROT_EXECUTE)) != (VM_PROT_READ | VM_PROT_EXECUTE) ||
      (current & VM_PROT_WRITE))
    _Exit(78);
  vm_check(mach_vm_protect(mach_task_self(), block->base, RINZU_CALLBACK_PAGE, FALSE,
                           VM_PROT_READ | VM_PROT_WRITE));
  mach_vm_address_t region = block->base;
  mach_vm_size_t size = 0;
  vm_region_basic_info_data_64_t info;
  mach_msg_type_number_t count = VM_REGION_BASIC_INFO_COUNT_64;
  mach_port_t object = MACH_PORT_NULL;
  vm_check(mach_vm_region(mach_task_self(), &region, &size, VM_REGION_BASIC_INFO_64,
                          (vm_region_info_t)&info, &count, &object));
  /* Mach may coalesce adjacent RW allocations. Validate that the entire
     control page lies in the reported RW region without adding addresses. */
  if (region > block->base || size < RINZU_CALLBACK_PAGE ||
      block->base - region > size - RINZU_CALLBACK_PAGE ||
      info.protection != (VM_PROT_READ | VM_PROT_WRITE))
    _Exit(78);
  if (object != MACH_PORT_NULL)
    vm_check(mach_port_deallocate(mach_task_self(), object));
  block->next = blocks;
  blocks = block;
  block_count++;
  return block;
}

static const void *prepare_entry(rinzu_callback_control_t *control, void (*bridge)(void)) {
  lock(&pool_gate);
  if (entry_count == SIZE_MAX || block_count >= SIZE_MAX / (RINZU_CALLBACK_PAGE * 2))
    _Exit(71);
  Block *block = blocks;
  if (!block || block->used == RINZU_CALLBACK_SLOTS)
    block = grow();
  size_t index = block->used++;
  Slot *slot = (Slot *)(block->base + index * RINZU_CALLBACK_STRIDE);
  slot->control = control;
  control->slot = slot;
  slot->bridge = bridge;
  const void *entry =
      (const void *)(block->base + RINZU_CALLBACK_PAGE + index * RINZU_CALLBACK_STRIDE);
  entry_count++;
  atomic_fetch_add(&live_connections, 1);
  unlock(&pool_gate);
  return entry;
}
static rinzu_callback_control_t *prepare_control(void *context, int state) {
  if (!context)
    _Exit(78);
  rinzu_callback_control_t *control = allocate(sizeof(*control));
  if (pthread_mutex_init(&control->gate, NULL))
    _Exit(71);
  control->origin = pthread_self();
  control->context = context;
  control->state = state;
  return control;
}
static rinzu_callback_pointer_u64_entry_t
prepare_pointer(rinzu_callback_pointer_u64_handler_t handler, void *context, int state) {
  if (!handler)
    _Exit(78);
  rinzu_callback_control_t *control = prepare_control(context, state);
  control->handler = handler;
  return (rinzu_callback_pointer_u64_entry_t){
      control,
      (rinzu_callback_pointer_u64_t)prepare_entry(control, rinzu_callback_pointer_u64_bridge)};
}
static rinzu_callback_frame_entry_t prepare_frame(rinzu_callback_frame_handler_t handler,
                                                  void *context, int state) {
  if (!handler)
    _Exit(78);
  rinzu_callback_control_t *control = prepare_control(context, state);
  control->frame_handler = handler;
  return (rinzu_callback_frame_entry_t){control,
                                        prepare_entry(control, rinzu_callback_frame_bridge)};
}
rinzu_callback_pointer_u64_entry_t
rinzu_callback_pointer_u64_prepare(rinzu_callback_pointer_u64_handler_t handler, void *context) {
  return prepare_pointer(handler, context, OPEN);
}
rinzu_callback_frame_entry_t rinzu_callback_frame_prepare(rinzu_callback_frame_handler_t handler,
                                                          void *context) {
  return prepare_frame(handler, context, OPEN);
}
rinzu_callback_pointer_u64_entry_t
rinzu_callback_pointer_u64_prepare_deferred(rinzu_callback_pointer_u64_handler_t handler,
                                            void *context) {
  return prepare_pointer(handler, context, PREPARED);
}
rinzu_callback_frame_entry_t
rinzu_callback_frame_prepare_deferred(rinzu_callback_frame_handler_t handler, void *context) {
  return prepare_frame(handler, context, PREPARED);
}
void rinzu_callback_entry_open(rinzu_callback_control_t *control) {
  if (!control || !rinzu_thread_equal(control->origin, pthread_self()))
    _Exit(86);
  lock(&control->gate);
  if (control->state != PREPARED || control->in_flight || !control->context)
    _Exit(86);
  control->state = OPEN;
  unlock(&control->gate);
}
void rinzu_callback_entry_bind_foreign(rinzu_callback_control_t *control,
                                       rinzu_foreign_queue_t *queue, uint64_t generation,
                                       int eligible) {
  if (!control || !queue || !generation || (eligible != 0 && eligible != 1) ||
      !rinzu_thread_equal(control->origin, pthread_self()))
    _Exit(86);
  rinzu_foreign_owner(queue);
  lock(&control->gate);
  if (control->state != PREPARED || control->foreign_queue || control->in_flight)
    _Exit(86);
  control->foreign_queue = queue;
  control->foreign_generation = generation;
  control->foreign_eligible = (unsigned)eligible;
  unlock(&control->gate);
}
void rinzu_callback_entry_bind_diagnostics(rinzu_callback_control_t *control,
                                           rinzu_activity_board_t *board, const char *symbol,
                                           uint64_t file, uint32_t start_byte, uint32_t end_byte,
                                           uint64_t owner, uint64_t depth) {
  if (!control || !board || !symbol || !rinzu_thread_equal(control->origin, pthread_self()))
    _Exit(86);
  lock(&control->gate);
  if (control->state != PREPARED || control->diagnostic || control->in_flight)
    _Exit(86);
  size_t length = strlen(symbol);
  if (length > SIZE_MAX - sizeof(CallbackDiagnostic) - 1)
    _Exit(71);
  CallbackDiagnostic *diagnostic = allocate(sizeof(*diagnostic) + length + 1);
  diagnostic->board = board;
  diagnostic->file = file;
  diagnostic->start_byte = start_byte;
  diagnostic->end_byte = end_byte;
  diagnostic->owner = owner;
  diagnostic->depth = depth;
  memcpy(diagnostic->symbol, symbol, length + 1);
  size_t bytes = sizeof(*diagnostic) + length + 1;
  lock(&pool_gate);
  if (diagnostic_live == SIZE_MAX || bytes > SIZE_MAX - diagnostic_bytes)
    _Exit(71);
  diagnostic_live++;
  diagnostic_bytes += bytes;
  unlock(&pool_gate);
  control->diagnostic = diagnostic;
  unlock(&control->gate);
}
static void settle(rinzu_callback_control_t *control) {
  if (control->state == CLOSING && !control->in_flight && control->context) {
    control->context = NULL;
    control->handler = NULL;
    control->frame_handler = NULL;
    control->state = CLOSED;
    lock(&pool_gate);
    if (control->diagnostic) {
      size_t bytes = sizeof(*control->diagnostic) + strlen(control->diagnostic->symbol) + 1;
      if (!diagnostic_live || diagnostic_bytes < bytes)
        _Exit(86);
      diagnostic_live--;
      diagnostic_bytes -= bytes;
      free(control->diagnostic);
      control->diagnostic = NULL;
    }
    control->foreign_queue = NULL;
    control->foreign_generation = 0;
    control->foreign_eligible = 0;
    if (closed_count == SIZE_MAX || !atomic_fetch_sub(&live_connections, 1))
      _Exit(86);
    closed_count++;
    unlock(&pool_gate);
  }
}
static void foreign_receipt(void *raw) {
  rinzu_callback_control_t *control = raw;
  lock(&control->gate);
  if (!control->in_flight)
    _Exit(86);
  control->in_flight--;
  atomic_fetch_sub(&in_flight, 1);
  settle(control);
  unlock(&control->gate);
}
static void foreign_wait_end(void *raw) {
  rinzu_activity_end(raw);
}
static void foreign_ready_received(void *raw) {
  rinzu_ready_receive(raw);
}
static void foreign_ready_dispose(void *raw) {
  rinzu_ready_dispose(raw);
}
static uint64_t rinzu_callback_foreign_invoke(rinzu_callback_control_t *control,
                                              rinzu_foreign_queue_t *queue, uint64_t generation,
                                              const uint64_t *words, size_t count) {
  /* Only ABI bits/pointers and stable admission metadata are touched here. */
  rinzu_activity_t *activity = NULL;
  const CallbackDiagnostic *diagnostic = control->diagnostic;
  if (diagnostic) {
    activity = rinzu_activity_prepare(diagnostic->board, RINZU_ACTIVITY_FOREIGN_WAIT,
                                      diagnostic->symbol, diagnostic->file, diagnostic->start_byte,
                                      diagnostic->end_byte, diagnostic->owner, diagnostic->depth);
  }
  rinzu_foreign_record_t *record = rinzu_foreign_record_new(
      queue, (uint64_t)(uintptr_t)control, generation, words, count, foreign_receipt, control);
  if (diagnostic) {
    rinzu_ready_work_t *ready = rinzu_ready_prepare_on(
        diagnostic->board, diagnostic->symbol, diagnostic->file, diagnostic->start_byte,
        diagnostic->end_byte, diagnostic->owner, diagnostic->depth, control->origin);
    record->ready_context = ready;
    record->ready_received = foreign_ready_received;
    record->ready_dispose = foreign_ready_dispose;
    rinzu_ready_publish(ready);
  }
  if (activity)
    rinzu_activity_start(activity);
  rinzu_foreign_publish(record);
  uint64_t result =
      rinzu_foreign_receive_before_receipt(record, activity ? foreign_wait_end : NULL, activity);
  rinzu_foreign_release(record);
  return result;
}
int rinzu_callback_foreign_drain_one(rinzu_foreign_queue_t *queue) {
  if (foreign_executing)
    _Exit(86);
  rinzu_foreign_record_t *record = rinzu_foreign_pop(queue);
  if (!record)
    return 0;
  rinzu_callback_control_t *control = (void *)(uintptr_t)record->callback;
  lock(&control->gate);
  if (!rinzu_thread_equal(control->origin, pthread_self()) || !control->in_flight ||
      (control->state != OPEN && control->state != CLOSING) || !control->context ||
      control->foreign_queue != queue || control->foreign_generation != record->generation)
    _Exit(86);
  rinzu_callback_pointer_u64_handler_t handler = control->handler;
  rinzu_callback_frame_handler_t frame_handler = control->frame_handler;
  rinzu_activity_board_t *board = control->diagnostic ? control->diagnostic->board : NULL;
  void *context = control->context;
  unlock(&control->gate);
  if (board)
    rinzu_activity_owner_turn(board);
  /* Admission, not current loop/generation progress, authorizes this execution.
     Accepted records remain valid after loop return and control close. */
  uint64_t result;
  foreign_executing = 1;
  if (handler) {
    if (record->count != 1)
      _Exit(86);
    result = handler(context, (void *)(uintptr_t)record->words[0]);
  } else {
    if (!frame_handler || record->count != 17)
      _Exit(86);
    rinzu_callback_frame_t frame;
    memcpy(&frame, record->words, sizeof(frame));
    result = frame_handler(context, &frame);
  }
  foreign_executing = 0;
  rinzu_foreign_complete(record, result);
  rinzu_foreign_release(record);
  return 1;
}
int rinzu_callback_entry_close(rinzu_callback_control_t *control) {
  if (!control || !rinzu_thread_equal(control->origin, pthread_self()))
    _Exit(86);
  lock(&control->gate);
  if (control->state != CLOSED)
    control->state = CLOSING;
  settle(control);
  int done = control->in_flight == 0;
  unlock(&control->gate);
  return done;
}
int rinzu_callback_entry_settled(rinzu_callback_control_t *control) {
  if (!control)
    _Exit(86);
  lock(&control->gate);
  int done = control->state == CLOSED && !control->in_flight;
  unlock(&control->gate);
  return done;
}
int rinzu_callback_entry_retire(rinzu_callback_control_t **owner) {
  if (!owner)
    _Exit(86);
  rinzu_callback_control_t *control = *owner;
  if (!control)
    return 1;
  if (!rinzu_thread_equal(control->origin, pthread_self()))
    _Exit(86);
  lock(&control->gate);
  if (control->state != CLOSED || control->in_flight || control->context || control->diagnostic ||
      control->foreign_queue) {
    unlock(&control->gate);
    return 0;
  }
  /* Caller closes the template's pre-admission gap by whole capability proof
     and physical return. An atomic NULL store alone would not rescue a reader
     which already loaded the old control. */
  lock(&pool_gate);
  if (!control->slot || control->slot->control != control || retired_count >= closed_count)
    _Exit(86);
  control->slot->control = NULL;
  retired_count++;
  unlock(&control->gate);
  if (pthread_mutex_destroy(&control->gate))
    _Exit(86);
  free(control);
  *owner = NULL;
  unlock(&pool_gate);
  return 1;
}
/* Called while the connection gate pins its copied diagnostic metadata. Queue
   and board inspection stays entirely native, even on the rejected thread. */
static void rinzu_callback_require_foreign(rinzu_callback_control_t *control) {
  const char *reason = NULL;
  if (!control->foreign_queue || !control->foreign_generation)
    reason = "no-compatible-origin";
  else if (!control->foreign_eligible)
    reason = "unsupported-signature";
  else
    switch (rinzu_foreign_admission_status(control->foreign_queue, control->foreign_generation)) {
    case RINZU_FOREIGN_ALLOWED:
      return;
    case RINZU_FOREIGN_NO_ORIGIN:
      reason = "no-compatible-origin";
      break;
    case RINZU_FOREIGN_INACTIVE_ORIGIN:
      reason = "inactive-origin";
      break;
    case RINZU_FOREIGN_BOUNDED_WAIT:
      reason = "nested-native-wait";
      break;
    case RINZU_FOREIGN_UNVERIFIED_LOOP:
      reason = "unverified-runloop-progress";
      break;
    default:
      _Exit(86);
    }
  CallbackDiagnostic *diagnostic = control->diagnostic;
  if (diagnostic)
    rinzu_activity_foreign_unavailable_fatal(
        diagnostic->board, reason, diagnostic->symbol, diagnostic->file, diagnostic->start_byte,
        diagnostic->end_byte, diagnostic->owner, diagnostic->depth);
  _Exit(86);
}
uint64_t rinzu_callback_pointer_u64_dispatch(const unsigned char *data,
                                             rinzu_callback_control_t *control) {
  if (!control)
    _Exit(86);
  lock(&control->gate);
  if (control->state != OPEN || !control->handler || !control->context ||
      control->in_flight == SIZE_MAX)
    _Exit(86);
  int foreign = !rinzu_thread_equal(control->origin, pthread_self());
  if (foreign)
    rinzu_callback_require_foreign(control);
  rinzu_foreign_queue_t *queue = control->foreign_queue;
  uint64_t generation = control->foreign_generation;
  rinzu_callback_pointer_u64_handler_t handler = control->handler;
  void *context = control->context;
  control->in_flight++;
  atomic_fetch_add(&in_flight, 1);
  atomic_fetch_add(&invocations, 1);
  unlock(&control->gate);
  if (foreign) {
    uint64_t word = (uint64_t)(uintptr_t)data;
    return rinzu_callback_foreign_invoke(control, queue, generation, &word, 1);
  }
  uint64_t result = handler(context, data);
  lock(&control->gate);
  if (!control->in_flight)
    _Exit(86);
  control->in_flight--;
  atomic_fetch_sub(&in_flight, 1);
  settle(control);
  unlock(&control->gate);
  return result;
}
uint64_t rinzu_callback_frame_dispatch(rinzu_callback_control_t *control,
                                       const rinzu_callback_frame_t *frame) {
  if (!control)
    _Exit(86);
  lock(&control->gate);
  if (control->state != OPEN || !control->frame_handler || !control->context ||
      control->in_flight == SIZE_MAX || !frame)
    _Exit(86);
  int foreign = !rinzu_thread_equal(control->origin, pthread_self());
  if (foreign)
    rinzu_callback_require_foreign(control);
  rinzu_foreign_queue_t *queue = control->foreign_queue;
  uint64_t generation = control->foreign_generation;
  rinzu_callback_frame_handler_t handler = control->frame_handler;
  void *context = control->context;
  control->in_flight++;
  atomic_fetch_add(&in_flight, 1);
  atomic_fetch_add(&invocations, 1);
  unlock(&control->gate);
  if (foreign) {
    uint64_t words[17];
    memcpy(words, frame, sizeof(words));
    return rinzu_callback_foreign_invoke(control, queue, generation, words, 17);
  }
  uint64_t result = handler(context, frame);
  lock(&control->gate);
  if (!control->in_flight)
    _Exit(86);
  control->in_flight--;
  atomic_fetch_sub(&in_flight, 1);
  settle(control);
  unlock(&control->gate);
  return result;
}
uint64_t rinzu_callback_frame_word(const rinzu_callback_frame_t *frame,
                                   rinzu_callback_abi_cursor_t *cursor, unsigned kind) {
  if (!frame || !cursor || kind > RINZU_CALLBACK_POINTER || cursor->integers > 8 ||
      cursor->floats > 8)
    _Exit(86);
  static const unsigned widths[] = {1, 2, 4, 8, 1, 2, 4, 8, 4, 8, 1, 8};
  unsigned width = widths[kind];
  int floating = kind == 8 || kind == 9;
  uint64_t bits = 0;
  if (floating && cursor->floats < 8)
    bits = frame->floats[cursor->floats++];
  else if (!floating && cursor->integers < 8)
    bits = frame->integers[cursor->integers++];
  else {
    if (!frame->stack)
      _Exit(86);
    if (cursor->stack > SIZE_MAX - (width - 1))
      _Exit(71);
    size_t offset = (cursor->stack + width - 1) & ~((size_t)width - 1);
    if (offset > SIZE_MAX - width)
      _Exit(71);
    memcpy(&bits, frame->stack + offset, width);
    cursor->stack = offset + width;
  }
  switch (kind) {
  case 0:
    return (uint64_t)(int64_t)(int8_t)bits;
  case 1:
    return (uint64_t)(int64_t)(int16_t)bits;
  case 2:
    return (uint64_t)(int64_t)(int32_t)bits;
  case 4:
  case 10:
    return (uint8_t)bits;
  case 5:
    return (uint16_t)bits;
  case 6:
  case 8:
    return (uint32_t)bits;
  default:
    return bits;
  }
}
rinzu_callback_stats_t rinzu_callback_entry_stats(void) {
  lock(&pool_gate);
  if (entry_count >
      (SIZE_MAX - block_count * RINZU_CALLBACK_PAGE) / sizeof(rinzu_callback_control_t))
    _Exit(71);
  rinzu_callback_stats_t stats = {entry_count,
                                  block_count,
                                  block_count * RINZU_CALLBACK_PAGE * 2,
                                  block_count * RINZU_CALLBACK_PAGE +
                                      (entry_count - retired_count) *
                                          sizeof(rinzu_callback_control_t),
                                  atomic_load(&live_connections),
                                  atomic_load(&in_flight),
                                  atomic_load(&invocations),
                                  closed_count,
                                  (closed_count - retired_count) * sizeof(rinzu_callback_control_t),
                                  block_count * sizeof(Block),
                                  diagnostic_live,
                                  diagnostic_bytes,
                                  retired_count,
                                  retired_count * sizeof(Slot),
                                  retired_count * sizeof(rinzu_callback_control_t)};
  unlock(&pool_gate);
  return stats;
}

/* Private call-site ledger. The public SDK still receives its exact C ABI. */
#include "rinzu/native_opaque.h"
#include "rinzu/native_record.h"
#include "rinzu/callback_return.h"
#include "rinzu/callback_consumption.h"
#include "rinzu/foreign_queue.h"
enum {
  RINZU_NATIVE_SCALAR,
  RINZU_NATIVE_STRING,
  RINZU_NATIVE_BYTES,
  RINZU_NATIVE_CALLBACK,
  RINZU_NATIVE_OPAQUE,
  RINZU_NATIVE_RECORD,
  RINZU_NATIVE_USERDATA = 7,
  RINZU_NATIVE_BYTE_ARRAY
};
enum {
  RINZU_NATIVE_RESULT_OPAQUE = 12,
  RINZU_NATIVE_RESULT_STRING,
  RINZU_NATIVE_RESULT_BYTES,
  RINZU_NATIVE_RESULT_RECORD,
  RINZU_NATIVE_RESULT_BYTE_ARRAY,
  RINZU_NATIVE_RESULT_USERDATA
};
typedef struct RinzuNativeCallbackPlan RinzuNativeCallbackPlan;
typedef struct {
  unsigned kind, scalar;
  uint64_t length;
  const uint64_t *targets;
  size_t target_count;
  const RinzuNativeCallbackPlan *callback;
} RinzuNativeParameter;
struct RinzuNativeCallbackPlan {
  const RinzuNativeParameter *parameters;
  size_t count;
  unsigned result, userdata;
  uint64_t result_length;
  unsigned foreign_eligible, captured;
  size_t capture_type;
};
typedef struct {
  const RinzuNativeParameter *parameters;
  size_t count;
  unsigned result;
  size_t result_type;
  uint64_t result_length;
  uint64_t (*supplant)(uint64_t, const uint64_t *);
  const char *symbol;
  unsigned consume_returns;
} RinzuNativePlan;
typedef struct {
  const unsigned char *bytes;
  size_t prefix;
  uint64_t length;
  unsigned string;
} RinzuCallbackBufferConstant;
typedef struct {
  unsigned kind;
  uint64_t length;
  uint64_t (*body)(void *, const uint64_t *);
  unsigned has_fallback;
  uint64_t fallback;
  RinzuCallbackBufferConstant buffer;
  const char *symbol;
} RinzuNativeTarget;
/* SOURCE_NATIVE_PLANS */
#ifdef RINZU_TOOLING_PLANS
#include RINZU_TOOLING_PLANS
#endif
typedef struct {
  RinzuContext *owner;
  const RinzuNativeCallbackPlan *plan;
  RinzuNativeTarget target;
  uint64_t userdata_scope;
  void *parent_scope;
  rinzu_source_position_t diagnostic_source;
  uint64_t capture;
  uint64_t return_scope;
} RinzuManagedCallback;
typedef struct {
  rinzu_callback_control_t *control;
  rinzu_native_length_callback_t length;
  RinzuManagedCallback *managed;
  unsigned foreign_eligible;
} RinzuNativeCallback;
typedef struct RinzuNativeCall {
  size_t plan;
  uint64_t depth, result, userdata_scope;
  uint64_t return_scope;
  unsigned phase;
  uint64_t *words;
  void **buffers;
  size_t buffer_count, callback_count, record_count;
  rinzu_native_record_loan_t *record_loan;
  rinzu_native_record_argument_t *records;
  size_t record_argument_count;
  RinzuNativeCallback *callbacks;
  rinzu_managed_roots_t retained;
  rinzu_managed_root_set_t root_set;
  rinzu_native_opaque_reservation_t opaque_result;
  struct RinzuNativeCall *next;
  rinzu_owner_native_frame_t native_frame;
  unsigned bounded;
  rinzu_activity_t *activity;
} RinzuNativeCall;
typedef struct {
  _Atomic size_t prepared, returned, closed, settled, read, released;
  _Atomic size_t callback_entries, copied_bytes, live_calls, live_buffers, live_callbacks;
  _Atomic size_t abandoned;
  _Atomic size_t return_normal, return_fallback, return_bytes, return_metadata;
} RinzuNativeEvidence;
static RinzuNativeEvidence rinzu_native_evidence;
static const unsigned char *rinzu_native_return_copy(uint64_t scope, const unsigned char *input,
                                                     size_t prefix, size_t length, unsigned string,
                                                     unsigned fallback) {
  const unsigned char *result =
      scope ? rinzu_return_scope_copy(scope, input, prefix, length, string, fallback)
            : rinzu_callback_return_buffer_copy(input, prefix, length, string, fallback);
  size_t bytes = length + string;
  atomic_fetch_add(
      fallback ? &rinzu_native_evidence.return_fallback : &rinzu_native_evidence.return_normal, 1);
  atomic_fetch_add(&rinzu_native_evidence.return_bytes, bytes ? bytes : 1);
  atomic_fetch_add(&rinzu_native_evidence.return_metadata, sizeof(rinzu_callback_return_node_t));
  return result;
}
/* SOURCE_NATIVE_STARTUP */
#ifdef RINZU_TOOLING_STARTUP
#include RINZU_TOOLING_STARTUP
#endif
void rinzu_baseline_leave_dispatch(void *raw);
static uint64_t rinzu_native_callback_fallback(const RinzuManagedCallback *callback) {
  if (!callback->target.has_fallback)
    rinzu_interop_fatal();
  return callback->target.fallback;
}
static int rinzu_native_callback_parameter(RinzuManagedCallback *callback,
                                           const RinzuNativeParameter *parameter, uint64_t word,
                                           rinzu_userdata_invocation_t admission, uint64_t *output,
                                           int *managed) {
  RinzuContext *context = callback->owner;
  rinzu_managed_data_t *data = rinzu_context_data(context);
  *managed = parameter->kind != RINZU_NATIVE_SCALAR;
  if (parameter->kind == RINZU_NATIVE_SCALAR) {
    if (!rinzu_transfer_valid(parameter->scalar, word))
      return 5;
    *output = word;
  } else if (parameter->kind == RINZU_NATIVE_USERDATA) {
    if (!callback->userdata_scope)
      rinzu_interop_fatal();
    if (rinzu_userdata_resolve(rinzu_context_userdata(context), data, admission,
                               (size_t)parameter->length, (const void *)(uintptr_t)word, output))
      return 8;
  } else if (parameter->kind == RINZU_NATIVE_STRING) {
    int status = rinzu_managed_receive_string(data, (const unsigned char *)(uintptr_t)word, output);
    if (status)
      return status == RINZU_NATIVE_NULL_STRING ? 8 : 9;
    atomic_fetch_add(&rinzu_native_evidence.copied_bytes,
                     rinzu_managed_find(data, *output)->length);
    rinzu_resource_copy(rinzu_managed_find(data, *output)->length);
  } else if (parameter->kind == RINZU_NATIVE_BYTES) {
    rinzu_managed_receive_bytes(data, parameter->length, (const unsigned char *)(uintptr_t)word,
                                output);
    atomic_fetch_add(&rinzu_native_evidence.copied_bytes, parameter->length);
    rinzu_resource_copy(parameter->length);
  } else if (parameter->kind == RINZU_NATIVE_RECORD) {
    int status = rinzu_managed_receive_record(data, rinzu_native_record_layout(parameter->length),
                                              (const unsigned char *)(uintptr_t)word, output);
    if (status)
      return status == RINZU_NATIVE_NULL_RECORD ? 8 : 5;
  } else if (parameter->kind == RINZU_NATIVE_OPAQUE) {
    if (!word)
      return 8;
    rinzu_native_opaque_reservation_t reservation =
        rinzu_managed_reserve_opaque(data, (size_t)parameter->length);
    if (rinzu_managed_receive_opaque(data, &reservation, (const void *)(uintptr_t)word, output))
      rinzu_interop_fatal();
  } else
    rinzu_interop_fatal();
  return 0;
}
static uint64_t rinzu_native_managed_callback(void *raw, const rinzu_callback_frame_t *native) {
  RinzuManagedCallback *callback = raw;
  RinzuContext *context = callback->owner;
  rinzu_managed_data_t *data = rinzu_context_data(context);
  const RinzuNativeCallbackPlan *plan = callback->plan;
  rinzu_owner_callback_scope_t transition =
      rinzu_callback_foreign_executing()
          ? rinzu_owner_admitted_callback_enter(rinzu_context_transitions(context))
          : rinzu_owner_callback_enter(rinzu_context_transitions(context));
  rinzu_activity_t *activity = rinzu_owner_activity_prepare(
      rinzu_context_transitions(context), RINZU_ACTIVITY_MANAGED, callback->target.symbol,
      callback->diagnostic_source.file, callback->diagnostic_source.start_byte,
      callback->diagnostic_source.end_byte, context->roots.depth);
  rinzu_activity_start(activity);
  void *previous_scope = rinzu_aot_callback_enter_scope(context, callback->parent_scope);
  const unsigned captured = plan->captured;
  if (captured > 1)
    rinzu_interop_fatal();
  size_t managed_count = plan->count + captured;
  if (managed_count < plan->count || managed_count > SIZE_MAX / sizeof(uint64_t))
    rinzu_resource_fatal();
  uint64_t *words = managed_count ? rinzu_failure_allocate(managed_count * sizeof(*words)) : NULL;
  if (captured)
    words[0] = callback->capture;
  rinzu_managed_roots_t inputs = {0};
  rinzu_managed_root_set_t attached = {0};
  rinzu_managed_enter(&inputs);
  rinzu_managed_attach_roots(data, &attached, &inputs);
  inputs.current->roots =
      managed_count ? rinzu_failure_allocate(managed_count * sizeof(uint64_t)) : NULL;
  inputs.current->capacity = managed_count;
  if (captured) {
    inputs.current->roots[inputs.current->count] = callback->capture;
    rinzu_managed_set_count(&inputs, inputs.current->count + 1);
  }
  rinzu_userdata_invocation_t admission = {0};
  if (callback->userdata_scope)
    admission =
        rinzu_userdata_admit(rinzu_context_userdata(context), data, callback->userdata_scope);
  rinzu_callback_abi_cursor_t cursor = {0};
  int input_error = 0;
  for (size_t index = 0; index < plan->count; ++index) {
    const RinzuNativeParameter *parameter = &plan->parameters[index];
    uint64_t word = rinzu_callback_frame_word(
        native, &cursor,
        parameter->kind == RINZU_NATIVE_SCALAR ? parameter->scalar : RINZU_CALLBACK_POINTER);
    int managed = 0;
    input_error = rinzu_native_callback_parameter(callback, parameter, word, admission,
                                                  &words[index + captured], &managed);
    if (input_error)
      break;
    if (managed) {
      inputs.current->roots[inputs.current->count] = words[index + captured];
      rinzu_managed_set_count(&inputs, inputs.current->count + 1);
      inputs.publications++;
    }
    rinzu_managed_collect(data, &context->roots);
  }
  uint64_t result = 0;
  if (!input_error) {
    rinzu_failure_state_t outer = context->pending;
    rinzu_source_position_t source = context->source;
    uint64_t depth = context->roots.depth;
    context->pending.current = (rinzu_failure_t){0};
    context->pending.suspended = NULL;
    context->pending.depth = 0;
    if (context->callback_depth == UINT64_MAX || !callback->target.body)
      rinzu_interop_fatal();
    context->callback_depth++;
    context->callback_dispatches++;
    atomic_fetch_add(&rinzu_native_evidence.callback_entries, 1);
    result = callback->target.body(context, words);
    if (context->roots.depth != depth + 1 || context->pending.depth || context->pending.suspended)
      rinzu_interop_fatal();
    /* Copy while the compiled return root still holds the managed result. */
    if (!context->pending.current.primary) {
      if (plan->result == RINZU_NATIVE_RESULT_STRING || plan->result == RINZU_NATIVE_RESULT_BYTES) {
        rinzu_managed_value_t *value = rinzu_managed_find(data, result);
        unsigned string = plan->result == RINZU_NATIVE_RESULT_STRING;
        if (value->kind != (string ? RINZU_MANAGED_STRING : RINZU_MANAGED_BYTES) ||
            (!string && value->length != plan->result_length))
          rinzu_interop_fatal();
        result = (uint64_t)(uintptr_t)rinzu_native_return_copy(
            callback->return_scope, value->bytes, value->length, value->length, string, 0);
        size_t bytes = value->length + string;
        if (!callback->return_scope)
          rinzu_resource_return_site(0, bytes ? bytes : 1, callback->diagnostic_source.file,
                                     callback->diagnostic_source.start_byte,
                                     callback->diagnostic_source.end_byte, callback->target.symbol);
      } else if (plan->result ? !rinzu_transfer_valid(plan->result - 1, result) : result != 0)
        rinzu_interop_fatal();
    }
    rinzu_baseline_leave_dispatch(context);
    rinzu_failure_t failure = context->pending.current;
    context->pending.current = outer.current;
    context->pending.suspended = outer.suspended;
    context->pending.depth = outer.depth;
    context->source = source;
    context->callback_depth--;
    if (failure.primary) {
      if (failure.primary != 1 && failure.primary != 16 && !rinzu_failure_is_error(failure.primary))
        rinzu_interop_fatal();
      for (rinzu_diagnostic_node_t *node = failure.head; node; node = node->next)
        if (node->cause != 1 && node->cause != 16 && !rinzu_failure_is_error(node->cause))
          rinzu_interop_fatal();
      rinzu_failure_release(failure);
      result = rinzu_native_callback_fallback(callback);
    }
  }
  rinzu_managed_leave(&inputs);
  rinzu_managed_detach_roots(data, &attached);
  rinzu_managed_dispose(&inputs);
  if (callback->userdata_scope)
    rinzu_userdata_settle(rinzu_context_userdata(context), data, admission);
  free(words);
  rinzu_aot_callback_leave_scope(context, previous_scope);
  rinzu_activity_end(activity);
  rinzu_owner_callback_leave(transition);
  return input_error ? rinzu_native_callback_fallback(callback) : result;
}
static void rinzu_native_callback_release(RinzuNativeCallback *callback) {
  if (callback->control && !rinzu_callback_entry_close(callback->control))
    rinzu_interop_fatal();
  if (callback->managed)
    free(callback->managed);
  else {
    rinzu_native_length_context_t *body = callback->length.context;
    if (!body || body->prepared_inputs != body->body_entries)
      rinzu_interop_fatal();
    atomic_fetch_add(&rinzu_native_evidence.callback_entries, body->body_entries);
    atomic_fetch_add(&rinzu_native_evidence.copied_bytes, body->copied_bytes);
    /* The once owner cleared control at retirement. Keep the length wrapper's
       handle in agreement before its normal close/context release. */
    callback->length.callback.control = callback->control;
    rinzu_native_length_release(&callback->length);
  }
  *callback = (RinzuNativeCallback){0};
}
static RinzuNativeCall *rinzu_native_call(RinzuContext *context, int64_t plan) {
  if (plan < 0 || (uint64_t)plan >= rinzu_native_plan_count)
    rinzu_interop_fatal();
  for (RinzuNativeCall *call = context->native_calls; call; call = call->next)
    if (call->plan == (size_t)plan && call->depth == context->roots.depth)
      return call;
  rinzu_interop_fatal();
  return NULL;
}
/* The private call owns its argument bookkeeping until release/abandon. Keep
   only the arrays required by the signature, in the same physical allocation.
   Snapshot payloads and managed root frames retain their independent owners. */
static size_t rinzu_native_call_array(size_t *size, size_t count, size_t width, size_t alignment) {
  if (!count)
    return 0;
  size_t padding = (alignment - *size % alignment) % alignment;
  if (padding > SIZE_MAX - *size || count > (SIZE_MAX - *size - padding) / width)
    rinzu_resource_fatal();
  size_t offset = *size + padding;
  *size = offset + count * width;
  return offset;
}
static int64_t rinzu_aot_record_busy(RinzuContext *context);
/* Reservations keep identities only. Copy after all argument evaluation, at
   the actual entry preparation, without retaining any movable payload slots. */
static int rinzu_native_prepare_records(RinzuContext *context, RinzuNativeCall *call) {
  if (call->phase || call->record_loan)
    rinzu_interop_fatal();
  if (!call->record_argument_count)
    return RINZU_NATIVE_RECORD_OK;
  rinzu_managed_data_t *data = rinzu_context_data(context);
  int status = rinzu_native_record_prepare(data, call->records, call->record_argument_count,
                                           &call->record_loan);
  if (status)
    return status;
  call->record_count = call->record_loan->link.count;
  const RinzuNativePlan *plan = &rinzu_native_plans[call->plan];
  size_t record = 0;
  for (size_t index = 0; index < plan->count; ++index)
    if (plan->parameters[index].kind == RINZU_NATIVE_RECORD)
      call->words[index] = (uint64_t)(uintptr_t)rinzu_native_record_argument_pointer(
          data, call->record_loan, record++);
  call->record_argument_count = 0;
  return RINZU_NATIVE_RECORD_OK;
}
/* No entry pointer has escaped from an unsuccessful preparation. Retire its
   callback capabilities before releasing their managed environments. */
static void rinzu_native_discard_preparation(RinzuContext *context, RinzuNativeCall *call) {
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (call->return_scope && !rinzu_return_scope_finish(call->return_scope, 0))
    rinzu_interop_fatal();
  if (call->userdata_scope)
    rinzu_userdata_discard_prepared(rinzu_context_userdata(context), data, call->userdata_scope);
  for (size_t index = 0; index < call->callback_count; ++index) {
    RinzuNativeCallback *callback = &call->callbacks[index];
    if (!rinzu_callback_entry_close(callback->control) ||
        !rinzu_callback_entry_retire(&callback->control))
      rinzu_interop_fatal();
    rinzu_native_callback_release(callback);
  }
  if (call->record_loan)
    rinzu_native_record_release(data, call->record_loan);
  for (size_t index = 0; index < call->buffer_count; ++index)
    free(call->buffers[index]);
  rinzu_managed_leave(&call->retained);
  rinzu_managed_detach_roots(data, &call->root_set);
  rinzu_managed_dispose(&call->retained);
  if (call->opaque_result.identity)
    rinzu_managed_discard_opaque(data, &call->opaque_result);
  free(call);
}
void rinzu_baseline_source_native_prepare(void *raw, int64_t plan_index,
                                          const uint64_t *arguments) {
  RinzuContext *context = raw;
  if (plan_index < 0 || (uint64_t)plan_index >= rinzu_native_plan_count || !context->roots.depth ||
      !rinzu_native_addresses[plan_index])
    rinzu_interop_fatal();
  for (RinzuNativeCall *other = context->native_calls; other; other = other->next)
    if (other->plan == (size_t)plan_index && other->depth == context->roots.depth)
      rinzu_interop_fatal();
  const RinzuNativePlan *plan = &rinzu_native_plans[plan_index];
  if ((plan->count && !arguments) || plan->count >= SIZE_MAX / sizeof(uint64_t) ||
      plan->count > SIZE_MAX / sizeof(RinzuNativeCallback))
    rinzu_interop_fatal();
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (plan->result == RINZU_NATIVE_RESULT_RECORD) {
    const rinzu_native_record_layout_t *layout = rinzu_native_record_layout(plan->result_length);
    if (layout->type != plan->result_type)
      rinzu_interop_fatal();
    rinzu_native_record_validate(data, layout);
  }
  size_t buffer_capacity = 0, callback_capacity = 0, record_capacity = 0;
  for (size_t index = 0; index < plan->count; ++index) {
    unsigned kind = plan->parameters[index].kind;
    buffer_capacity += kind == RINZU_NATIVE_STRING || kind == RINZU_NATIVE_BYTES ||
                       kind == RINZU_NATIVE_BYTE_ARRAY;
    callback_capacity += kind == RINZU_NATIVE_CALLBACK;
    record_capacity += kind == RINZU_NATIVE_RECORD;
  }
  size_t call_size = sizeof(RinzuNativeCall);
  size_t words_offset =
      rinzu_native_call_array(&call_size, plan->count, sizeof(uint64_t), _Alignof(uint64_t));
  size_t buffers_offset =
      rinzu_native_call_array(&call_size, buffer_capacity, sizeof(void *), _Alignof(void *));
  size_t callbacks_offset = rinzu_native_call_array(
      &call_size, callback_capacity, sizeof(RinzuNativeCallback), _Alignof(RinzuNativeCallback));
  size_t records_offset =
      rinzu_native_call_array(&call_size, record_capacity, sizeof(rinzu_native_record_argument_t),
                              _Alignof(rinzu_native_record_argument_t));
  RinzuNativeCall *call = rinzu_failure_allocate(call_size);
  call->plan = (size_t)plan_index;
  call->depth = context->roots.depth;
  if (plan->consume_returns)
    call->return_scope = rinzu_return_scope_prepare();
  call->words = words_offset ? (uint64_t *)((unsigned char *)call + words_offset) : NULL;
  call->buffers = buffers_offset ? (void **)((unsigned char *)call + buffers_offset) : NULL;
  call->callbacks =
      callbacks_offset ? (RinzuNativeCallback *)((unsigned char *)call + callbacks_offset) : NULL;
  call->records = records_offset
                      ? (rinzu_native_record_argument_t *)((unsigned char *)call + records_offset)
                      : NULL;
  size_t root_count = 0;
  rinzu_managed_attach_roots(data, &call->root_set, &call->retained);
  rinzu_managed_enter(&call->retained);
  /* Stage into this inactive publication buffer. It may have storage from a
     previous call in this Domain; count stays zero until every typed argument
     and any opaque result reservation have been prepared. Callback GC cannot
     trace unused capacity. Keep one spare slot for the received managed result. */
  uint64_t *roots = rinzu_managed_reserve_roots(&call->retained, plan->count + 1);
  if (plan->result == RINZU_NATIVE_RESULT_USERDATA) {
    call->userdata_scope = rinzu_userdata_enter(rinzu_context_userdata(context), data);
    rinzu_userdata_expect_result(rinzu_context_userdata(context), data, call->userdata_scope,
                                 plan->result_type);
  }
  for (size_t index = 0; index < plan->count; ++index)
    if (!call->userdata_scope &&
        (plan->parameters[index].kind == RINZU_NATIVE_USERDATA ||
         (plan->parameters[index].callback && plan->parameters[index].callback->userdata))) {
      call->userdata_scope = rinzu_userdata_enter(rinzu_context_userdata(context), data);
      break;
    }
  for (size_t index = 0; index < plan->count; ++index) {
    const RinzuNativeParameter *parameter = &plan->parameters[index];
    uint64_t bits = arguments[index];
    if (parameter->kind == RINZU_NATIVE_SCALAR) {
      if (!rinzu_transfer_valid(parameter->scalar, bits))
        rinzu_interop_fatal();
      call->words[index] = bits;
    } else if (parameter->kind == RINZU_NATIVE_STRING || parameter->kind == RINZU_NATIVE_BYTES ||
               parameter->kind == RINZU_NATIVE_BYTE_ARRAY) {
      rinzu_managed_value_t *value = rinzu_managed_find(data, bits);
      unsigned expected =
          parameter->kind == RINZU_NATIVE_STRING ? RINZU_MANAGED_STRING : RINZU_MANAGED_BYTES;
      if (value->kind != expected ||
          (parameter->kind == RINZU_NATIVE_BYTES && value->length != parameter->length) ||
          value->length >= SIZE_MAX)
        rinzu_interop_fatal();
      size_t length = (size_t)value->length;
      size_t size = parameter->kind == RINZU_NATIVE_STRING ? length + 1 : (length ? length : 1);
      unsigned char *buffer = rinzu_failure_allocate(size);
      if (length)
        memcpy(buffer, value->bytes, length);
      call->words[index] = (uint64_t)(uintptr_t)buffer;
      call->buffers[call->buffer_count++] = buffer;
      roots[root_count++] = bits;
    } else if (parameter->kind == RINZU_NATIVE_OPAQUE) {
      call->words[index] =
          (uint64_t)(uintptr_t)rinzu_managed_opaque_pointer(data, bits, (size_t)parameter->length);
      roots[root_count++] = bits;
    } else if (parameter->kind == RINZU_NATIVE_RECORD) {
      call->records[call->record_argument_count++] =
          (rinzu_native_record_argument_t){bits, rinzu_native_record_layout(parameter->length)};
      roots[root_count++] = bits;
    } else if (parameter->kind == RINZU_NATIVE_USERDATA) {
      call->words[index] = (uint64_t)(uintptr_t)rinzu_userdata_prepare(
          rinzu_context_userdata(context), data, call->userdata_scope, (size_t)parameter->length,
          bits);
    } else if (parameter->kind == RINZU_NATIVE_CALLBACK) {
      if (!parameter->callback)
        rinzu_interop_fatal();
      uint64_t capture = 0;
      if (parameter->callback->captured) {
        capture = bits;
        size_t type = parameter->callback->capture_type;
        rinzu_managed_slot_t environment = {0};
        uint32_t variant = 0;
        if (type >= data->schema->count || data->schema->types[type].kind != RINZU_VALUE_STATE ||
            data->schema->types[type].field_count != 1 ||
            rinzu_managed_project(data, capture, type, -1, 0, &environment) ||
            !environment.reference ||
            rinzu_managed_enum_variant(data, environment.bits, data->schema->types[type].fields[0],
                                       &variant) ||
            variant >= parameter->target_count)
          rinzu_interop_fatal();
        bits = parameter->targets[variant];
        roots[root_count++] = capture;
      }
      int allowed = 0;
      for (size_t target = 0; target < parameter->target_count; ++target)
        if (parameter->targets[target] == bits)
          allowed = 1;
      if (!allowed || !bits || bits > rinzu_native_target_count)
        rinzu_interop_fatal();
      RinzuNativeTarget target = rinzu_native_targets[bits - 1];
      if (!parameter->callback)
        rinzu_interop_fatal();
      if (target.kind == 4) {
        if (!rinzu_native_target_addresses[bits - 1])
          rinzu_interop_fatal();
        call->words[index] = (uint64_t)(uintptr_t)rinzu_native_target_addresses[bits - 1];
      } else if (target.kind == 3) {
        RinzuManagedCallback *managed = rinzu_failure_allocate(sizeof(*managed));
        *managed = (RinzuManagedCallback){context,
                                          parameter->callback,
                                          target,
                                          call->userdata_scope,
                                          rinzu_aot_callback_parent_scope(context),
                                          context->source,
                                          capture,
                                          call->return_scope};
        if (target.buffer.bytes) {
          const RinzuCallbackBufferConstant *buffer = &target.buffer;
          if (!target.has_fallback ||
              parameter->callback->result !=
                  (buffer->string ? RINZU_NATIVE_RESULT_STRING : RINZU_NATIVE_RESULT_BYTES) ||
              (!buffer->string && parameter->callback->result_length != buffer->length))
            rinzu_interop_fatal();
          if (buffer->length > SIZE_MAX)
            rinzu_resource_fatal();
          managed->target.fallback = (uint64_t)(uintptr_t)rinzu_native_return_copy(
              call->return_scope, buffer->bytes, buffer->prefix, (size_t)buffer->length,
              buffer->string, 1);
          size_t bytes = (size_t)buffer->length + buffer->string;
          if (!call->return_scope)
            rinzu_resource_return_site(1, bytes ? bytes : 1, context->source.file,
                                       context->source.start_byte, context->source.end_byte,
                                       target.symbol);
        }
        rinzu_callback_frame_entry_t entry =
            rinzu_callback_frame_prepare_deferred(rinzu_native_managed_callback, managed);
        call->words[index] = (uint64_t)(uintptr_t)entry.entry;
        call->callbacks[call->callback_count++] =
            (RinzuNativeCallback){.control = entry.control,
                                  .managed = managed,
                                  .foreign_eligible = parameter->callback->foreign_eligible};
      } else {
        const RinzuNativeCallbackPlan *callback_plan = parameter->callback;
        if ((target.kind != 1 && target.kind != 2) || callback_plan->count != 1 ||
            callback_plan->result != 8 ||
            callback_plan->parameters[0].kind !=
                (target.kind == 1 ? RINZU_NATIVE_BYTES : RINZU_NATIVE_STRING) ||
            callback_plan->parameters[0].length != target.length)
          rinzu_interop_fatal();
        rinzu_native_length_callback_t length = rinzu_native_length_prepare_deferred(
            data, &context->roots, target.kind == 2, target.length);
        length.context->transitions = rinzu_context_transitions(context);
        length.context->diagnostic_source = context->source;
        call->words[index] = (uint64_t)(uintptr_t)length.callback.entry;
        call->callbacks[call->callback_count++] =
            (RinzuNativeCallback){.control = length.callback.control,
                                  .length = length,
                                  .foreign_eligible = parameter->callback->foreign_eligible};
      }
    } else
      rinzu_interop_fatal();
  }
  if (plan->result == RINZU_NATIVE_RESULT_OPAQUE) {
    call->opaque_result = rinzu_managed_reserve_opaque(data, plan->result_type);
    roots[root_count++] = call->opaque_result.identity;
  }
  if (!plan->supplant) {
    int status = rinzu_native_prepare_records(context, call);
    if (status) {
      if (status != RINZU_VALUE_BUSY)
        rinzu_interop_fatal();
      rinzu_native_discard_preparation(context, call);
      (void)rinzu_aot_record_busy(context);
      return;
    }
  }
  rinzu_managed_frame_t *frame = call->retained.current;
  if (!frame || frame->writing || frame->count || frame->roots != roots ||
      frame->capacity < plan->count + 1)
    rinzu_interop_fatal();
  rinzu_managed_set_count(&call->retained, root_count);
  call->retained.publications += 1;
  call->activity = rinzu_owner_activity_prepare(
      rinzu_context_transitions(context), RINZU_ACTIVITY_NATIVE, plan->symbol, context->source.file,
      context->source.start_byte, context->source.end_byte, context->roots.depth);
  for (size_t index = 0; index < call->callback_count; index++) {
    RinzuNativeCallback *callback = &call->callbacks[index];
    const char *symbol = callback->managed
                             ? callback->managed->target.symbol
                             : (callback->length.context->utf8 ? "rinzu.core::utf8ByteLength"
                                                               : "rinzu.core::byteLength");
    rinzu_callback_entry_bind_diagnostics(callback->control, call->activity->board, symbol,
                                          context->source.file, context->source.start_byte,
                                          context->source.end_byte, call->activity->owner,
                                          call->activity->depth);
  }
  call->next = context->native_calls;
  context->native_calls = call;
  atomic_fetch_add(&rinzu_native_evidence.prepared, 1);
  atomic_fetch_add(&rinzu_native_evidence.live_calls, 1);
  rinzu_resource_acquire(RINZU_RESOURCE_NATIVE_CALLS, 1);
  atomic_fetch_add(&rinzu_native_evidence.live_buffers, call->buffer_count + call->record_count);
  atomic_fetch_add(&rinzu_native_evidence.live_callbacks, call->callback_count);
}
uint64_t rinzu_baseline_source_native_address(void *raw, int64_t plan) {
  RinzuNativeCall *call = rinzu_native_call(raw, plan);
  if (call->phase)
    rinzu_interop_fatal();
  return (uint64_t)(uintptr_t)rinzu_native_addresses[call->plan];
}
const uint64_t *rinzu_baseline_source_native_words(void *raw, int64_t plan) {
  RinzuContext *context = raw;
  RinzuNativeCall *call = rinzu_native_call(raw, plan);
  if (call->phase || call->record_argument_count)
    rinzu_interop_fatal();
  if (call->record_loan)
    rinzu_native_record_enter(rinzu_context_data(context), call->record_loan);
  if (call->return_scope && !rinzu_return_scope_enter(call->return_scope))
    rinzu_interop_fatal();
  if (call->userdata_scope)
    rinzu_userdata_open(rinzu_context_userdata(context), rinzu_context_data(context),
                        call->userdata_scope);
  rinzu_owner_native_enter(rinzu_context_transitions(context), &call->native_frame,
                           context->roots.depth, rinzu_context_native_roots(context));
  if (context->foreign_queue && !rinzu_native_plans[plan].supplant) {
    rinzu_foreign_bounded_enter(context->foreign_queue);
    call->bounded = 1;
  }
  for (size_t index = 0; index < call->callback_count; ++index)
    rinzu_callback_entry_open(call->callbacks[index].control);
  rinzu_activity_start(call->activity);
  return call->words;
}
void rinzu_baseline_source_native_returned(void *raw, int64_t plan, uint64_t result) {
  RinzuContext *context = raw;
  RinzuNativeCall *call = rinzu_native_call(raw, plan);
  if (call->phase)
    rinzu_interop_fatal();
  rinzu_activity_end(call->activity);
  call->activity = NULL;
  call->result = result;
  if (call->record_loan)
    rinzu_native_record_returned(rinzu_context_data(context), call->record_loan);
  call->phase = 1;
  if (call->bounded) {
    rinzu_foreign_bounded_leave(context->foreign_queue);
    call->bounded = 0;
  }
  rinzu_owner_native_leave(rinzu_context_transitions(context), &call->native_frame);
  if (call->userdata_scope) {
    rinzu_userdata_returned(rinzu_context_userdata(context), rinzu_context_data(context),
                            call->userdata_scope);
    if (rinzu_native_plans[plan].result != RINZU_NATIVE_RESULT_USERDATA)
      call->userdata_scope = 0;
  }
  atomic_fetch_add(&rinzu_native_evidence.returned, 1);
}
void rinzu_baseline_source_native_close(void *raw, int64_t plan) {
  RinzuNativeCall *call = rinzu_native_call(raw, plan);
  if (call->phase != 1)
    rinzu_interop_fatal();
  for (size_t index = 0; index < call->callback_count; ++index)
    (void)rinzu_callback_entry_close(call->callbacks[index].control);
  call->phase = 2;
  atomic_fetch_add(&rinzu_native_evidence.closed, 1);
}
void rinzu_baseline_source_native_settle(void *raw, int64_t plan) {
  RinzuContext *context = raw;
  RinzuNativeCall *call = rinzu_native_call(context, plan);
  if (call->phase != 2)
    rinzu_interop_fatal();
  for (size_t index = 0; index < call->callback_count; ++index)
    if (!rinzu_callback_entry_settled(call->callbacks[index].control))
      rinzu_interop_fatal();
  if (call->record_loan)
    rinzu_native_record_commit(rinzu_context_data(context), call->record_loan);
  /* Exact body/image proof, physical return and result receipts close every
     native payload reader. The managed return value has its separate GC roots. */
  if (call->return_scope) {
    if (!rinzu_return_scope_finish(call->return_scope, 1))
      rinzu_interop_fatal();
    call->return_scope = 0;
    for (size_t index = 0; index < call->callback_count; ++index)
      if (!rinzu_callback_entry_retire(&call->callbacks[index].control))
        rinzu_interop_fatal();
  }
  call->phase = 3;
  atomic_fetch_add(&rinzu_native_evidence.settled, 1);
}
uint64_t rinzu_baseline_source_native_read(void *raw, int64_t plan) {
  RinzuContext *context = raw;
  RinzuNativeCall *call = rinzu_native_call(context, plan);
  if (call->phase != 3)
    rinzu_interop_fatal();
  unsigned result = rinzu_native_plans[plan].result;
  call->phase = 4;
  atomic_fetch_add(&rinzu_native_evidence.read, 1);
  if (result == RINZU_NATIVE_RESULT_OPAQUE) {
    uint64_t received = 0, reservation = call->opaque_result.identity;
    int status = rinzu_managed_receive_opaque(rinzu_context_data(context), &call->opaque_result,
                                              (const void *)(uintptr_t)call->result, &received);
    if (status == RINZU_NATIVE_NULL_OPAQUE) {
      /* A discarded identity must leave the retained root list before callback
         connection teardown can collect this heap. This edit does not allocate. */
      rinzu_managed_frame_t *frame = call->retained.current;
      if (!frame || !frame->count || frame->roots[frame->count - 1] != reservation)
        rinzu_interop_fatal();
      rinzu_managed_set_count(&call->retained, frame->count - 1);
      rinzu_failure_fail(&context->pending, 8, context->source);
      return 0;
    }
    return received;
  }
  if (result == RINZU_NATIVE_RESULT_STRING || result == RINZU_NATIVE_RESULT_BYTES ||
      result == RINZU_NATIVE_RESULT_RECORD || result == RINZU_NATIVE_RESULT_BYTE_ARRAY ||
      result == RINZU_NATIVE_RESULT_USERDATA) {
    const RinzuNativePlan *description = &rinzu_native_plans[plan];
    rinzu_managed_data_t *data = rinzu_context_data(context);
    const unsigned char *input = (const unsigned char *)(uintptr_t)call->result;
    uint64_t received = 0;
    if (result == RINZU_NATIVE_RESULT_USERDATA) {
      int status =
          rinzu_userdata_receive_result(rinzu_context_userdata(context), data, call->userdata_scope,
                                        description->result_type, input, &received);
      if (status == RINZU_USERDATA_NULL) {
        rinzu_failure_fail(&context->pending, 8, context->source);
        return 0;
      }
      if (status != RINZU_USERDATA_OK)
        rinzu_interop_fatal();
    } else if (result == RINZU_NATIVE_RESULT_STRING) {
      int status = rinzu_managed_receive_string(data, input, &received);
      if (status != RINZU_NATIVE_INPUT_OK) {
        rinzu_failure_fail(&context->pending, status == RINZU_NATIVE_NULL_STRING ? 8 : 9,
                           context->source);
        return 0;
      }
    } else if (result == RINZU_NATIVE_RESULT_BYTE_ARRAY) {
      int status = rinzu_managed_receive_byte_array(data, (const rinzu_native_byte_array_t *)input,
                                                    &received);
      if (status != RINZU_NATIVE_INPUT_OK) {
        rinzu_failure_fail(&context->pending, 8, context->source);
        return 0;
      }
    } else if (result == RINZU_NATIVE_RESULT_RECORD) {
      int status = rinzu_native_receive_record_prepared(
          data, &call->root_set, description->result_length, input, &received);
      if (status != RINZU_NATIVE_RECORD_OK) {
        rinzu_failure_fail(&context->pending, status == RINZU_NATIVE_NULL_RECORD ? 8 : 5,
                           context->source);
        return 0;
      }
    } else
      rinzu_managed_receive_bytes(data, description->result_length, input, &received);
    if (rinzu_managed_slot_valid(data, description->result_type,
                                 (rinzu_managed_slot_t){received, 1}))
      rinzu_interop_fatal();
    rinzu_managed_frame_t *frame = call->retained.current;
    if (!frame || !frame->roots || frame->count > description->count)
      rinzu_interop_fatal();
    frame->roots[frame->count] = received;
    rinzu_managed_set_count(&call->retained, frame->count + 1);
    return received;
  }
  if (result) {
    if (!rinzu_transfer_valid(result - 1, call->result))
      rinzu_interop_fatal();
  } else if (call->result)
    rinzu_interop_fatal();
  return call->result;
}
void rinzu_baseline_source_native_release(void *raw, int64_t plan) {
  RinzuContext *context = raw;
  RinzuNativeCall *call = rinzu_native_call(context, plan);
  if (call->phase != 4)
    rinzu_interop_fatal();
  if (call->userdata_scope) {
    if (rinzu_native_plans[plan].result != RINZU_NATIVE_RESULT_USERDATA)
      rinzu_interop_fatal();
    rinzu_userdata_finish_result(rinzu_context_userdata(context), rinzu_context_data(context),
                                 call->userdata_scope);
    call->userdata_scope = 0;
  }
  for (size_t index = 0; index < call->callback_count; ++index) {
    rinzu_native_callback_release(&call->callbacks[index]);
  }
  for (size_t index = 0; index < call->buffer_count; ++index)
    free(call->buffers[index]);
  if (call->record_loan)
    rinzu_native_record_release(rinzu_context_data(context), call->record_loan);
  rinzu_managed_leave(&call->retained);
  rinzu_managed_detach_roots(rinzu_context_data(context), &call->root_set);
  rinzu_managed_dispose(&call->retained);
  RinzuNativeCall **cursor = &context->native_calls;
  while (*cursor != call) {
    if (!*cursor)
      rinzu_interop_fatal();
    cursor = &(*cursor)->next;
  }
  *cursor = call->next;
  atomic_fetch_add(&rinzu_native_evidence.released, 1);
  atomic_fetch_sub(&rinzu_native_evidence.live_calls, 1);
  rinzu_resource_release(RINZU_RESOURCE_NATIVE_CALLS, 1);
  atomic_fetch_sub(&rinzu_native_evidence.live_buffers, call->buffer_count + call->record_count);
  atomic_fetch_sub(&rinzu_native_evidence.live_callbacks, call->callback_count);
  free(call);
}
#ifdef RINZU_AOT_HAS_SUPPLANT
/* A queued cancellation discards prepared resources without manufacturing a
   physical return or touching the result ABI. */
static void rinzu_native_abandon_prepared(RinzuContext *context, int64_t plan) {
  RinzuNativeCall *call = rinzu_native_call(context, plan);
  if (call->phase)
    rinzu_interop_fatal();
  rinzu_activity_end(call->activity);
  call->activity = NULL;
  RinzuNativeCall **cursor = &context->native_calls;
  while (*cursor && *cursor != call)
    cursor = &(*cursor)->next;
  if (!*cursor)
    rinzu_interop_fatal();
  *cursor = call->next;
  atomic_fetch_add(&rinzu_native_evidence.abandoned, 1);
  atomic_fetch_sub(&rinzu_native_evidence.live_calls, 1);
  rinzu_resource_release(RINZU_RESOURCE_NATIVE_CALLS, 1);
  atomic_fetch_sub(&rinzu_native_evidence.live_buffers, call->buffer_count + call->record_count);
  atomic_fetch_sub(&rinzu_native_evidence.live_callbacks, call->callback_count);
  rinzu_native_discard_preparation(context, call);
}
#endif
static int rinzu_aot_source_native_baseline(void) {
  rinzu_callback_stats_t entries = rinzu_callback_entry_stats();
  return !atomic_load(&rinzu_native_evidence.live_calls) &&
         !atomic_load(&rinzu_native_evidence.live_buffers) &&
         !atomic_load(&rinzu_native_evidence.live_callbacks) && !entries.live_connections &&
         !entries.in_flight && !entries.live_diagnostics && !entries.diagnostic_bytes;
}
static void rinzu_aot_source_native_evidence(void) {
  rinzu_callback_stats_t entries = rinzu_callback_entry_stats();
  rinzu_callback_return_stats_t returns = rinzu_callback_return_buffer_stats();
  rinzu_return_scope_stats_t scopes = rinzu_return_scope_stats();
  printf("RINZU_CALLBACK_ALLOCATIONS=%zu,%zu,%zu,%zu\n",
         atomic_load(&rinzu_native_evidence.return_normal),
         atomic_load(&rinzu_native_evidence.return_fallback),
         atomic_load(&rinzu_native_evidence.return_bytes),
         atomic_load(&rinzu_native_evidence.return_metadata));
  printf("RINZU_CALLBACK_SCOPES=%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu\n", scopes.allocated,
         scopes.released, scopes.promoted, scopes.allocated_bytes, scopes.released_bytes,
         scopes.promoted_bytes, scopes.live_bytes, scopes.peak_bytes, scopes.copied_bytes,
         scopes.active_scopes, scopes.metadata_bytes);
  printf("RINZU_CALLBACK_RETURNS=%zu,%zu,%zu,%zu\n", returns.normal, returns.fallback,
         returns.bytes, returns.metadata_bytes);
  printf("RINZU_NATIVE_ENTRIES=%zu,%zu,%zu,%zu,%zu,%zu\n", entries.entries, entries.blocks,
         entries.virtual_bytes, entries.control_bytes, entries.live_connections, entries.in_flight);
  printf("RINZU_CALLBACK_PROCESS=%zu,%zu,%zu\n", entries.closed_entries, entries.tombstone_bytes,
         entries.block_metadata_bytes);
  printf("RINZU_CALLBACK_TEMPORARY=%zu,%zu\n", entries.live_diagnostics, entries.diagnostic_bytes);
  printf("RINZU_CALLBACK_RETIREMENT=%zu,%zu,%zu\n", entries.retired_entries,
         entries.retired_slot_bytes, entries.released_control_bytes);
  printf("RINZU_NATIVE_CALLS=%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu\n",
         atomic_load(&rinzu_native_evidence.prepared), atomic_load(&rinzu_native_evidence.returned),
         atomic_load(&rinzu_native_evidence.closed), atomic_load(&rinzu_native_evidence.settled),
         atomic_load(&rinzu_native_evidence.read), atomic_load(&rinzu_native_evidence.released),
         atomic_load(&rinzu_native_evidence.callback_entries),
         atomic_load(&rinzu_native_evidence.copied_bytes),
         atomic_load(&rinzu_native_evidence.live_calls),
         atomic_load(&rinzu_native_evidence.live_buffers),
         atomic_load(&rinzu_native_evidence.live_callbacks));
  printf("RINZU_NATIVE_ABANDONED=%zu\n", atomic_load(&rinzu_native_evidence.abandoned));
}

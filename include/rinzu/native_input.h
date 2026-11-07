#ifndef RINZU_NATIVE_INPUT_H
#define RINZU_NATIVE_INPUT_H
#include "rinzu/callback_entry.h"
#include "rinzu/managed_composite.h"
#include "rinzu/owner_transition.h"
#include "rinzu/native_bytes.h"

enum {
  RINZU_NATIVE_INPUT_OK,
  RINZU_NATIVE_NULL_STRING,
  RINZU_NATIVE_INVALID_UTF8,
  RINZU_NATIVE_NULL_BYTE_ARRAY
};
static inline int rinzu_managed_receive_byte_array(rinzu_managed_data_t *data,
                                                   const rinzu_native_byte_array_t *descriptor,
                                                   uint64_t *output) {
  rinzu_managed_native_owner(data);
  if (!descriptor)
    return RINZU_NATIVE_NULL_BYTE_ARRAY;
  /* Snapshot metadata once, before allocation. Native storage is stable by the
     same physical return/reception contract as fixed Bytes and native records. */
  rinzu_native_byte_array_t view;
  memcpy(&view, descriptor, sizeof(view));
  if (view.length > INT64_MAX || view.length > PTRDIFF_MAX)
    rinzu_resource_fatal();
  if (!output || (view.length && !view.data))
    rinzu_interop_fatal();
  rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, view.length);
  if (view.length)
    memcpy(value->bytes, view.data, view.length);
  *output = value->identity;
  return RINZU_NATIVE_INPUT_OK;
}
static inline void rinzu_managed_receive_bytes(rinzu_managed_data_t *data, uint64_t length,
                                               const unsigned char *input, uint64_t *output) {
  rinzu_managed_native_owner(data);
  if (length && !input)
    rinzu_interop_fatal();
  if (length >= SIZE_MAX)
    rinzu_resource_fatal();
  if (rinzu_managed_bytes(data, length, input, (size_t)length, output))
    rinzu_interop_fatal();
}
static inline int rinzu_managed_receive_string(rinzu_managed_data_t *data,
                                               const unsigned char *input, uint64_t *output) {
  rinzu_managed_native_owner(data);
  if (!input)
    return RINZU_NATIVE_NULL_STRING;
  size_t length = strlen((const char *)input);
  if (length == SIZE_MAX)
    rinzu_resource_fatal();
  unsigned char *copy = rinzu_failure_allocate(length + 1);
  rinzu_resource_acquire(RINZU_RESOURCE_NATIVE_INPUT, length + 1);
  if (length)
    memcpy(copy, input, length);
  size_t scalars = rinzu_utf8_scalar_length(copy, length);
  if (scalars == SIZE_MAX) {
    free(copy);
    rinzu_resource_release(RINZU_RESOURCE_NATIVE_INPUT, length + 1);
    return RINZU_NATIVE_INVALID_UTF8;
  }
  *output = rinzu_managed_string_take_owned(data, copy, length, scalars);
  rinzu_resource_release(RINZU_RESOURCE_NATIVE_INPUT, length + 1);
  return RINZU_NATIVE_INPUT_OK;
}
typedef struct {
  rinzu_managed_data_t *data;
  rinzu_managed_roots_t *roots;
  uint64_t length;
  int utf8;
  size_t prepared_inputs, body_entries, copied_bytes;
  rinzu_owner_transition_t *transitions;
  rinzu_source_position_t diagnostic_source;
} rinzu_native_length_context_t;
typedef struct {
  rinzu_callback_pointer_u64_entry_t callback;
  rinzu_native_length_context_t *context;
} rinzu_native_length_callback_t;
static inline uint64_t rinzu_native_length_handler(void *raw, const unsigned char *input) {
  rinzu_native_length_context_t *context = raw;
  rinzu_owner_callback_scope_t scope = {0};
  if (context->transitions)
    scope = rinzu_callback_foreign_executing()
                ? rinzu_owner_admitted_callback_enter(context->transitions)
                : rinzu_owner_callback_enter(context->transitions);
  rinzu_activity_t *activity = NULL;
  if (context->transitions) {
    activity = rinzu_owner_activity_prepare(
        context->transitions, RINZU_ACTIVITY_MANAGED,
        context->utf8 ? "rinzu.core::utf8ByteLength" : "rinzu.core::byteLength",
        context->diagnostic_source.file, context->diagnostic_source.start_byte,
        context->diagnostic_source.end_byte, context->roots->depth);
    rinzu_activity_start(activity);
  }
  rinzu_managed_enter(context->roots);
  uint64_t handle;
  if (context->utf8) {
    /* Core length declares no fallback; an unhandled input Error is fatal. */
    if (rinzu_managed_receive_string(context->data, input, &handle))
      rinzu_interop_fatal();
  } else
    rinzu_managed_receive_bytes(context->data, context->length, input, &handle);
  rinzu_managed_publish(context->roots, &handle, 1);
  context->prepared_inputs++;
  rinzu_managed_collect(context->data, context->roots);
  rinzu_managed_value_t *value = rinzu_managed_find(context->data, handle);
  if (value->kind != (context->utf8 ? RINZU_MANAGED_STRING : RINZU_MANAGED_BYTES) ||
      (!context->utf8 && value->length != context->length))
    rinzu_interop_fatal();
  context->body_entries++;
  context->copied_bytes += value->length;
  rinzu_resource_copy(value->length);
  uint64_t result = value->length;
  rinzu_managed_leave(context->roots);
  rinzu_managed_collect(context->data, context->roots);
  if (activity)
    rinzu_activity_end(activity);
  if (context->transitions)
    rinzu_owner_callback_leave(scope);
  return result;
}
static inline rinzu_native_length_callback_t
rinzu_native_length_prepare_state(rinzu_managed_data_t *data, rinzu_managed_roots_t *roots,
                                  int utf8, uint64_t length, int deferred) {
  rinzu_managed_native_owner(data);
  rinzu_native_length_context_t *context = rinzu_failure_allocate(sizeof(*context));
  *context =
      (rinzu_native_length_context_t){.data = data, .roots = roots, .length = length, .utf8 = utf8};
  rinzu_callback_pointer_u64_entry_t entry =
      deferred ? rinzu_callback_pointer_u64_prepare_deferred(rinzu_native_length_handler, context)
               : rinzu_callback_pointer_u64_prepare(rinzu_native_length_handler, context);
  return (rinzu_native_length_callback_t){entry, context};
}
static inline rinzu_native_length_callback_t
rinzu_native_length_prepare(rinzu_managed_data_t *data, rinzu_managed_roots_t *roots, int utf8,
                            uint64_t length) {
  return rinzu_native_length_prepare_state(data, roots, utf8, length, 0);
}
static inline rinzu_native_length_callback_t
rinzu_native_length_prepare_deferred(rinzu_managed_data_t *data, rinzu_managed_roots_t *roots,
                                     int utf8, uint64_t length) {
  return rinzu_native_length_prepare_state(data, roots, utf8, length, 1);
}
static inline void rinzu_native_length_release(rinzu_native_length_callback_t *callback) {
  if (!callback || !callback->context ||
      (callback->callback.control && !rinzu_callback_entry_close(callback->callback.control)))
    rinzu_interop_fatal();
  free(callback->context);
  *callback = (rinzu_native_length_callback_t){0};
}
#endif

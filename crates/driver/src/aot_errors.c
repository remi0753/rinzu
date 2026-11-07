#include "rinzu/managed_errors.h"
static void rinzu_aot_throw_error(RinzuContext *context, uint64_t value) {
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (!context->pending.current.primary) {
    int first = !rinzu_managed_error_source(data, value).present;
    context->pending.current.primary_error = rinzu_managed_lease_error(data, value);
    context->pending.current.primary = 15;
    context->pending.current.primary_source =
        rinzu_managed_first_error_source(data, value, context->source);
    rinzu_error_trace_t *trace = rinzu_managed_error_trace(data, value);
    if (!trace && first) {
      rinzu_error_trace_t *captured = rinzu_error_trace_stack_capture(
          &context->error_trace_stack, context->pending.current.primary_source);
      trace = rinzu_managed_first_error_trace(data, value, captured);
      rinzu_error_trace_release(captured);
    }
    context->pending.current.primary_trace = rinzu_error_trace_copy(trace);
  }
}
static void rinzu_aot_group_observed(RinzuContext *context, const rinzu_failure_t *failure) {
  if (context->pending.observed_count == INT64_MAX)
    rinzu_failure_fatal();
  rinzu_observed_error_t *observed = rinzu_failure_allocate(sizeof(*observed));
  observed->failure = rinzu_failure_copy(failure);
  rinzu_failure_forget_managed(&observed->failure);
  observed->identity = ++context->pending.observed_count;
  observed->next = context->pending.observed;
  context->pending.observed = observed;
  rinzu_aot_async_observe(context, failure);
}
int64_t rinzu_baseline_error_data(void *raw, int64_t operation, int64_t type, int64_t value,
                                  int64_t argument, const uint64_t *types, int64_t count) {
  RinzuContext *context = raw;
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (type < 0 || (uint64_t)type >= data->schema->count || count < 0)
    rinzu_failure_fatal();
  size_t before = data->objects;
  uint64_t output = 0;
  switch (operation) {
  case 8:
    rinzu_error_group_enter(&context->error_groups, (uint64_t)argument);
    break;
  case 9:
    (void)rinzu_error_group_record(&context->error_groups, (uint64_t)argument,
                                   !!context->failure_origin, context->failure_origin,
                                   &context->pending.current);
    context->failure_origin = 0;
    break;
  case 10: {
    rinzu_group_failure_t *entries =
        rinzu_error_group_finish(&context->error_groups, (uint64_t)argument);
    rinzu_cleanup_frame_t *cleanup = context->pending.suspended;
    if (!cleanup || context->pending.current.primary)
      rinzu_failure_fatal();
    rinzu_failure_t incoming = cleanup->incoming;
    cleanup->incoming = (rinzu_failure_t){0};
    rinzu_failure_state_t check = {.current = incoming};
    int control = incoming.primary && !rinzu_failure_recoverable(&check);
    if (value && !control) {
      for (rinzu_group_failure_t *entry = entries; entry; entry = entry->next)
        rinzu_aot_group_observed(context, &entry->failure);
      if (incoming.primary)
        rinzu_aot_group_observed(context, &incoming);
    }
    rinzu_failure_t combined = rinzu_error_group_aggregate(entries);
    if (control) {
      rinzu_error_group_append(&incoming, combined);
      cleanup->incoming = incoming;
    } else {
      rinzu_error_group_append(&combined, incoming);
      if (!value)
        cleanup->incoming = combined;
      else if (combined.primary) {
        output = rinzu_managed_failure_list(data, (size_t)type, &combined);
        rinzu_failure_release(combined);
      }
    }
    if (!output && rinzu_managed_composite_fresh(data, (size_t)type, -1, NULL, 0, &output))
      rinzu_failure_fatal();
    break;
  }
  case 11:
    if (!value)
      rinzu_failure_fail(&context->pending, 14, context->source);
    break;
  case 0:
    output = rinzu_managed_error_cast(data, (size_t)type, (uint64_t)value);
    break;
  case 1:
    output = rinzu_managed_error_metadata(data, (size_t)type, (uint64_t)value, (size_t)argument);
    break;
  case 2:
    output = (uint64_t)rinzu_managed_match_errors(data, (uint64_t)value, types, (size_t)count,
                                                  argument != 0);
    break;
  case 3: {
    rinzu_managed_value_t *list = rinzu_managed_error_list(data, (uint64_t)value);
    if (argument < 0 || (uint64_t)argument >= list->count || !list->slots[argument].reference)
      rinzu_failure_fatal();
    output = rinzu_managed_error_cast(data, (size_t)type, list->slots[argument].bits);
    break;
  }
  case 4: {
    rinzu_managed_value_t *list = rinzu_managed_error_list(data, (uint64_t)value);
    if (argument < 0 || (uint64_t)argument > list->count || list->type != (size_t)type ||
        rinzu_managed_composite_fresh(data, (size_t)type, -1, list->slots + argument,
                                      list->count - (size_t)argument, &output))
      rinzu_failure_fatal();
    break;
  }
  case 5: {
    (void)rinzu_failure_observe(&context->pending);
    rinzu_failure_t *failure = &context->pending.observed->failure;
    rinzu_aot_async_observe(context, failure);
    output = rinzu_managed_failure_list(data, (size_t)type, failure);
    rinzu_failure_forget_managed(failure);
    break;
  }
  case 7: {
    rinzu_managed_value_t *error = rinzu_managed_find(data, (uint64_t)value);
    if (argument < 0 || (uint64_t)argument >= data->schema->count ||
        data->schema->types[argument].kind != RINZU_VALUE_ERROR_RECORD ||
        !rinzu_managed_is_error(data, error))
      rinzu_failure_fatal();
    output = error->kind == RINZU_MANAGED_COMPOSITE && error->type == (size_t)argument;
    break;
  }
  case 6:
    rinzu_aot_throw_error(context, (uint64_t)value);
    break;
  default:
    rinzu_failure_fatal();
  }
  if (data->objects < before || data->objects - before > UINT64_MAX - context->allocations)
    rinzu_failure_fatal();
  context->allocations += data->objects - before;
  return (int64_t)output;
}

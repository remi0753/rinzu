#ifndef RINZU_MANAGED_ERRORS_H
#define RINZU_MANAGED_ERRORS_H
#include "rinzu/managed_composite.h"

static inline int rinzu_managed_is_error(rinzu_managed_data_t *data, rinzu_managed_value_t *value) {
  return value && (value->kind == RINZU_MANAGED_ERROR || value->kind == RINZU_MANAGED_RICH_ERROR ||
      (value->kind == RINZU_MANAGED_COMPOSITE && data->schema && value->type < data->schema->count &&
       data->schema->types[value->type].kind == RINZU_VALUE_ERROR_RECORD));
}
/* Borrowed while the Error is alive. Independent reports retain their own copy. */
static inline rinzu_error_trace_t *rinzu_managed_error_trace(rinzu_managed_data_t *data,
                                                           uint64_t handle) {
  rinzu_managed_value_t *value = rinzu_managed_find(data, handle);
  if (!rinzu_managed_is_error(data, value) ||
      (value->kind == RINZU_MANAGED_COMPOSITE && value->count < 4) ||
      (value->kind == RINZU_MANAGED_RICH_ERROR && value->length <= sizeof value->inline_bytes))
    rinzu_failure_fatal();
  return value->error_trace;
}
static inline rinzu_error_trace_t *rinzu_managed_first_error_trace(rinzu_managed_data_t *data,
    uint64_t handle, rinzu_error_trace_t *trace) {
  rinzu_error_trace_t *original = rinzu_managed_error_trace(data, handle);
  if (!original && trace) {
    original = rinzu_error_trace_copy(trace);
    rinzu_managed_find(data, handle)->error_trace = original;
  }
  return original;
}
static inline rinzu_source_position_t rinzu_managed_error_source(rinzu_managed_data_t *data,
                                                                uint64_t handle) {
  rinzu_managed_value_t *value = rinzu_managed_find(data, handle);
  if (!rinzu_managed_is_error(data, value)) rinzu_failure_fatal();
  if (value->kind == RINZU_MANAGED_RICH_ERROR) {
    rinzu_error_node_t root = rinzu_error_node(value->bytes, value->length,
        (uint32_t)rinzu_error_integer(value->bytes, 4) - 1);
    return root.source;
  }
  return value->error_source;
}
static inline rinzu_source_position_t rinzu_managed_first_error_source(rinzu_managed_data_t *data,
    uint64_t handle, rinzu_source_position_t source) {
  rinzu_source_position_t original = rinzu_managed_error_source(data, handle);
  if (original.present || !source.present) return original;
  rinzu_managed_value_t *value = rinzu_managed_find(data, handle);
  if (value->kind == RINZU_MANAGED_RICH_ERROR) {
    rinzu_error_node_t root = rinzu_error_node(value->bytes, value->length,
        (uint32_t)rinzu_error_integer(value->bytes, 4) - 1);
    size_t offset = root.begin + 24 + root.message_length + root.context_length;
    if (offset > value->length || value->length - offset < 17) rinzu_failure_fatal();
    value->bytes[offset++] = 1;
    for (size_t i = 0; i < 8; ++i) value->bytes[offset++] = (unsigned char)(source.file >> (i * 8));
    for (size_t i = 0; i < 4; ++i) value->bytes[offset++] = (unsigned char)(source.start_byte >> (i * 8));
    for (size_t i = 0; i < 4; ++i) value->bytes[offset++] = (unsigned char)(source.end_byte >> (i * 8));
  } else value->error_source = source;
  return source;
}
static inline uint64_t rinzu_managed_source_option(rinzu_managed_data_t *data, size_t result,
                                                   rinzu_source_position_t source) {
  uint64_t output = 0;
  size_t low = 0, high = data->source_entry_count;
  while (low < high) {
    size_t middle = low + (high - low) / 2;
    const rinzu_source_entry_t *at = &data->source_entries[middle];
    if (at->file < source.file || (at->file == source.file && at->byte < source.start_byte)) low = middle + 1;
    else high = middle;
  }
  const rinzu_source_entry_t *at = low < data->source_entry_count ? &data->source_entries[low] : NULL;
  if (!source.present || !at || at->file != source.file || at->byte != source.start_byte) {
    if (rinzu_managed_composite_fresh(data, result, 0, NULL, 0, &output)) rinzu_failure_fatal();
    return output;
  }
  if (!at->line || !at->column || result >= data->schema->count) rinzu_failure_fatal();
  const rinzu_value_type_t *option = &data->schema->types[result];
  if (option->kind != RINZU_VALUE_ENUM || option->variant_count != 2 || option->variants[1].count != 1)
    rinzu_failure_fatal();
  uint64_t file = rinzu_managed_string_literal(data, (const unsigned char *)at->name, at->name_length);
  rinzu_managed_slot_t fields[] = {{file, 1}, {at->line, 0}, {at->column, 0}};
  uint64_t location = 0;
  if (rinzu_managed_composite_fresh(data, option->variants[1].types[0], -1, fields, 3, &location)) rinzu_failure_fatal();
  rinzu_managed_slot_t slot = {location, 1};
  if (rinzu_managed_composite_fresh(data, result, 1, &slot, 1, &output)) rinzu_failure_fatal();
  return output;
}
static inline rinzu_error_reference_t *rinzu_managed_lease_error(rinzu_managed_data_t *data, uint64_t handle) {
  rinzu_managed_owner(data);
  if (!rinzu_managed_is_error(data, rinzu_managed_find(data, handle))) rinzu_failure_fatal();
  for (rinzu_error_reference_t *entry = data->error_references; entry; entry = entry->next) {
    if (entry->handle == handle) {
      if (entry->references == SIZE_MAX || entry->owner != data) rinzu_failure_fatal();
      entry->references++;
      return entry;
    }
  }
  rinzu_error_reference_t *entry = rinzu_failure_allocate(sizeof(*entry));
  entry->references = 1;
  entry->owner = data;
  entry->handle = handle;
  entry->next = data->error_references;
  data->error_references = entry;
  return entry;
}
static inline uint64_t rinzu_managed_error_cast(rinzu_managed_data_t *data, size_t type, uint64_t handle) {
  if (rinzu_managed_slot_valid(data, type, (rinzu_managed_slot_t){handle, 1}) ||
      !rinzu_managed_is_error(data, rinzu_managed_find(data, handle))) rinzu_failure_fatal();
  return handle;
}
static inline rinzu_managed_value_t *rinzu_managed_error_list(rinzu_managed_data_t *data, uint64_t handle) {
  rinzu_managed_value_t *value = rinzu_managed_find(data, handle);
  if (value->kind != RINZU_MANAGED_COMPOSITE || !data->schema || value->type >= data->schema->count)
    rinzu_failure_fatal();
  const rinzu_value_type_t *type = &data->schema->types[value->type];
  if (type->kind != RINZU_VALUE_LIST || type->element >= data->schema->count ||
      data->schema->types[type->element].kind != RINZU_VALUE_ERROR) rinzu_failure_fatal();
  return value;
}
static inline int rinzu_managed_match_errors(rinzu_managed_data_t *data, uint64_t handle, const uint64_t *expected, size_t count, int rest) {
  rinzu_managed_value_t *list = rinzu_managed_error_list(data, handle);
  if (!list->count || (rest ? list->count < count : list->count != count)) return 0;
  if (count && !expected) rinzu_failure_fatal();
  for (size_t index = 0; index < count; ++index) {
    if (expected[index] >= data->schema->count || !list->slots[index].reference) rinzu_failure_fatal();
    unsigned kind = data->schema->types[expected[index]].kind;
    rinzu_managed_value_t *error = rinzu_managed_find(data, list->slots[index].bits);
    if (!rinzu_managed_is_error(data, error)) rinzu_failure_fatal();
    if (kind == RINZU_VALUE_ERROR) continue;
    if (kind != RINZU_VALUE_ERROR_RECORD) rinzu_failure_fatal();
    if (error->kind != RINZU_MANAGED_COMPOSITE || error->type != expected[index]) return 0;
  }
  return 1;
}
static inline uint64_t rinzu_managed_error_metadata(rinzu_managed_data_t *data, size_t result, uint64_t handle, size_t field) {
  rinzu_managed_value_t *value = rinzu_managed_find(data, handle);
  if (!rinzu_managed_is_error(data, value) || field > 3) rinzu_failure_fatal();
  if (value->kind == RINZU_MANAGED_COMPOSITE) {
    if (field >= value->count || !value->slots[field].reference || rinzu_managed_slot_valid(data, result, value->slots[field])) rinzu_failure_fatal();
    if (field == 2) {
      rinzu_managed_value_t *cached = rinzu_managed_find(data, value->slots[field].bits);
      rinzu_source_position_t source = rinzu_managed_error_source(data, handle);
      if (!cached->variant && source.present)
        value->slots[field].bits = rinzu_managed_source_option(data, result, source);
    }
    return value->slots[field].bits;
  }
  if (field == 0) return rinzu_managed_string(data, (const unsigned char *)"Error", 5);
  if (field == 1) return rinzu_managed_message(data, handle);
  if (field == 2) return rinzu_managed_source_option(data, result, rinzu_managed_error_source(data, handle));
  uint64_t output = 0;
  if (field == 3 && value->kind == RINZU_MANAGED_RICH_ERROR) {
    rinzu_error_node_t root = rinzu_error_node(value->bytes, value->length, (uint32_t)rinzu_error_integer(value->bytes, 4) - 1);
    if (root.cause != UINT32_MAX) {
      rinzu_error_node_t node = rinzu_error_node(value->bytes, value->length, root.cause);
      rinzu_error_buffer_t prefix = {0};
      rinzu_error_write(&prefix, (uint64_t)root.cause + 1, 4);
      rinzu_error_append(&prefix, value->bytes + 4, node.end - 4);
      uint64_t cause = rinzu_managed_rich_error(data, prefix.bytes, prefix.length);
      free(prefix.bytes);
      rinzu_managed_slot_t slot = {cause, 1};
      if (rinzu_managed_composite_fresh(data, result, 1, &slot, 1, &output)) rinzu_failure_fatal();
      return output;
    }
  }
  if (rinzu_managed_composite_fresh(data, result, 0, NULL, 0, &output)) rinzu_failure_fatal();
  return output;
}
static inline uint64_t rinzu_managed_failure_handle(rinzu_managed_data_t *data, int64_t cause, rinzu_source_position_t source, rinzu_error_reference_t *error) {
  if (error) {
    if (error->owner != data || !error->references || !rinzu_managed_is_error(data, rinzu_managed_find(data, error->handle))) rinzu_failure_fatal();
    return error->handle;
  }
  rinzu_failure_t failure = {.primary = cause, .primary_source = source};
  return rinzu_managed_error(data, &failure);
}
static inline uint64_t rinzu_managed_failure_list(rinzu_managed_data_t *data, size_t result, const rinzu_failure_t *failure) {
  if (!rinzu_failure_is_error(failure->primary)) rinzu_failure_fatal();
  size_t count = 1;
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    if (!rinzu_failure_is_error(node->cause) || count == SIZE_MAX / sizeof(rinzu_managed_slot_t)) rinzu_failure_fatal();
    count++;
  }
  rinzu_managed_slot_t *slots = rinzu_failure_allocate(count * sizeof(*slots));
  slots[0] = (rinzu_managed_slot_t){rinzu_managed_failure_handle(data, failure->primary, failure->primary_source, failure->primary_error), 1};
  (void)rinzu_managed_first_error_trace(data, slots[0].bits, failure->primary_trace);
  size_t index = 1;
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    slots[index++] = (rinzu_managed_slot_t){rinzu_managed_failure_handle(data, node->cause, node->source, node->error), 1};
    (void)rinzu_managed_first_error_trace(data, slots[index - 1].bits, node->trace);
  }
  uint64_t handle = 0;
  if (rinzu_managed_composite_fresh(data, result, -1, slots, count, &handle)) rinzu_failure_fatal();
  free(slots);
  return handle;
}
#endif

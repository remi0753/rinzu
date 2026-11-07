typedef struct {
  rinzu_managed_data_t *data;
  rinzu_display_slot_t *slots;
  size_t capacity;
  int busy;
} RinzuDisplayReader;
static int rinzu_aot_display_load(void *raw, size_t type, uint64_t bits,
                                  rinzu_display_node_t *node) {
  RinzuDisplayReader *reader = raw;
  rinzu_managed_value_t *value = rinzu_managed_find(reader->data, bits);
  const rinzu_value_type_t *layout = &rinzu_program_schema.types[type];
  switch (layout->kind) {
  case RINZU_VALUE_STRING:
    if (value->kind != RINZU_MANAGED_STRING)
      return 1;
    break;
  case RINZU_VALUE_BYTES:
  case RINZU_VALUE_DYNAMIC_BYTES:
    if (value->kind != RINZU_MANAGED_BYTES ||
        (layout->kind == RINZU_VALUE_BYTES && value->length != layout->bytes_length))
      return 1;
    break;
  case RINZU_VALUE_RECORD:
  case RINZU_VALUE_STATE:
  case RINZU_VALUE_ENUM:
  case RINZU_VALUE_LIST:
  case RINZU_VALUE_MAP:
    if (rinzu_managed_record_busy(reader->data, bits)) {
      reader->busy = 1;
      return 1;
    }
    if (value->kind != RINZU_MANAGED_COMPOSITE || value->type != type ||
        rinzu_managed_layout(layout, value->has_variant ? (int64_t)value->variant : -1,
                             value->count))
      return 1;
    if (value->count > reader->capacity) {
      reader->capacity = value->count;
      reader->slots = rinzu_display_grow(reader->slots, reader->capacity, sizeof *reader->slots);
    }
    for (size_t i = 0; i < value->count; ++i)
      reader->slots[i] = (rinzu_display_slot_t){value->slots[i].bits, value->slots[i].reference};
    node->fields = reader->slots;
    node->count = value->count;
    node->variant = value->variant;
    return 0;
  case RINZU_VALUE_ERROR_RECORD:
  case RINZU_VALUE_ERROR:
    if (value->kind == RINZU_MANAGED_COMPOSITE &&
        reader->data->schema->types[value->type].kind == RINZU_VALUE_ERROR_RECORD) {
      value = rinzu_managed_string_value(reader->data,
                                         rinzu_managed_message(reader->data, value->identity));
      break;
    }
    if (value->kind == RINZU_MANAGED_ERROR) {
      value = rinzu_managed_string_value(reader->data, value->message);
      break;
    }
    if (value->kind != RINZU_MANAGED_RICH_ERROR)
      return 1;
    {
      rinzu_error_node_t root = rinzu_error_node(
          value->bytes, value->length, (uint32_t)rinzu_error_integer(value->bytes, 4) - 1);
      node->text = (rinzu_display_text_t){root.message, root.message_length};
      return 0;
    }
  case RINZU_VALUE_OTHER:
    if (value->kind != RINZU_MANAGED_ERROR)
      return 1;
    value = rinzu_managed_string_value(reader->data, value->message);
    break;
  default:
    return 1;
  }
  node->text = (rinzu_display_text_t){value->bytes, value->length};
  return 0;
}
typedef struct {
  const unsigned char *bytes;
  unsigned char *owned;
  size_t length, scalars;
  unsigned char scalar[128];
} RinzuFormatPart;

int64_t rinzu_baseline_format_string(void *raw, const uint64_t *bits, const uint64_t *types,
                                     int64_t count) {
  RinzuContext *context = raw;
  if (count < 0 || (uint64_t)count > SIZE_MAX / sizeof(RinzuFormatPart) ||
      (count && (!bits || !types)))
    rinzu_failure_fatal();
  RinzuDisplayReader reader = {.data = rinzu_context_data(context)};
  rinzu_managed_owner(reader.data);
  RinzuFormatPart small[16];
  RinzuFormatPart *parts = (size_t)count <= sizeof(small) / sizeof(*small)
                               ? small
                               : rinzu_display_grow(NULL, (size_t)count, sizeof(*parts));
  size_t length = 0, scalars = 0;
  /* Compiler-supplied types choose the scalar/String path. No managed GC,
     native reentry, or yield occurs while these borrowed slices are held. */
  for (size_t i = 0; i < (size_t)count; ++i) {
    if (types[i] > INT64_MAX)
      rinzu_failure_fatal();
    size_t index = rinzu_data_type((int64_t)types[i]);
    rinzu_managed_slot_t slot = rinzu_data_slot(index, bits[i]);
    RinzuFormatPart *part = &parts[i];
    part->owned = NULL;
    if (rinzu_program_schema.types[index].kind == RINZU_VALUE_STRING) {
      rinzu_managed_value_t *value = NULL;
      if (rinzu_managed_slot_resolve_owned(reader.data, index, slot, &value) || !value)
        rinzu_failure_fatal();
      part->bytes = value->bytes;
      part->length = value->length;
      part->scalars = value->scalar_length;
    } else if (rinzu_program_display[index].kind == RINZU_DISPLAY_SCALAR) {
      rinzu_display_buffer_t rendered = {.bytes = part->scalar, .capacity = sizeof(part->scalar)};
      if (slot.reference ||
          rinzu_display_scalar(&rendered, rinzu_program_display[index].scalar, slot.bits) ||
          rendered.bytes != part->scalar)
        rinzu_failure_fatal();
      part->bytes = part->scalar;
      part->length = part->scalars = rendered.length;
    } else {
      /* Preserve the iterative graph formatter for records, collections,
         enums, errors, bytes and nested/escaped display. */
      if (rinzu_display_format(rinzu_program_display, rinzu_program_schema.count, index,
                               (rinzu_display_slot_t){slot.bits, slot.reference}, 0,
                               rinzu_aot_display_load, &reader, &part->owned, &part->length)) {
        if (reader.busy) {
          for (size_t released = 0; released <= i; ++released)
            free(parts[released].owned);
          if (parts != small)
            free(parts);
          free(reader.slots);
          return rinzu_aot_record_busy(context);
        }
        rinzu_failure_fatal();
      }
      part->bytes = part->owned;
      part->scalars = rinzu_utf8_scalar_length(part->bytes, part->length);
    }
    if (part->scalars == SIZE_MAX || part->length >= SIZE_MAX - length ||
        part->scalars > SIZE_MAX - scalars)
      rinzu_failure_fatal();
    length += part->length;
    scalars += part->scalars;
  }
  free(reader.slots);
  rinzu_managed_value_t *result = rinzu_managed_string_allocate_owned(reader.data, length, scalars);
  size_t offset = 0;
  for (size_t i = 0; i < (size_t)count; ++i) {
    if (parts[i].length)
      memcpy(result->bytes + offset, parts[i].bytes, parts[i].length);
    offset += parts[i].length;
    free(parts[i].owned);
  }
  if (parts != small)
    free(parts);
  context->allocations += 1;
  return (int64_t)result->identity;
}
int64_t rinzu_baseline_stdout(void *raw, int64_t bits, int64_t type, int64_t newline) {
  RinzuContext *context = raw;
  size_t index = rinzu_data_type(type);
  if (newline != 0 && newline != 1)
    rinzu_failure_fatal();
  /* Finish the iterative managed walk before publishing native-safe state. */
  RinzuDisplayReader reader = {.data = rinzu_context_data(context)};
  rinzu_managed_slot_t slot = rinzu_data_slot(index, (uint64_t)bits);
  unsigned char *bytes = NULL;
  size_t length = 0;
  int invalid = rinzu_display_format(rinzu_program_display, rinzu_program_schema.count, index,
                                     (rinzu_display_slot_t){slot.bits, slot.reference}, 0,
                                     rinzu_aot_display_load, &reader, &bytes, &length);
  free(reader.slots);
  if (reader.busy) {
    free(bytes);
    return rinzu_aot_record_busy(context);
  }
  if (invalid)
    rinzu_failure_fatal();
  size_t root_count = rinzu_context_native_roots(context);
  rinzu_owner_native_frame_t native = {0};
  rinzu_owner_native_enter(rinzu_context_transitions(context), &native, context->roots.depth,
                           root_count);
  int status = rinzu_stdio_write(STDOUT_FILENO, bytes, length, (int)newline);
  rinzu_owner_native_leave(rinzu_context_transitions(context), &native);
  free(bytes);
  if (status)
    rinzu_failure_fail(&context->pending, 12, context->source);
  return 0;
}

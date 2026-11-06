#ifdef RINZU_AOT_HAS_JSON
#include "rinzu/json.c"

static void rinzu_aot_throw_error(RinzuContext *, uint64_t);

enum {
  JSON_PARSED_BYTES,
  JSON_PARSED_STRING,
  JSON_NULL,
  JSON_BOOL,
  JSON_TEXT,
  JSON_TOKEN,
  JSON_INT,
  JSON_FLOAT,
  JSON_ARRAY,
  JSON_OBJECT
};

static rinzu_managed_slot_t json_bits(uint64_t bits) {
  return (rinzu_managed_slot_t){bits, 0};
}
static rinzu_managed_slot_t json_ref(uint64_t bits) {
  return (rinzu_managed_slot_t){bits, 1};
}

static RinzuJsonSlice json_slice(const rinzu_managed_value_t *value) {
  if (!value || (value->kind != RINZU_MANAGED_STRING && value->kind != RINZU_MANAGED_BYTES))
    rinzu_failure_fatal();
  return (RinzuJsonSlice){value->bytes, value->length};
}

static RinzuJsonSlice json_literal(const char *text) {
  return (RinzuJsonSlice){(const unsigned char *)text, strlen(text)};
}

static rinzu_managed_slot_t json_composite(RinzuContext *context, rinzu_managed_data_t *data,
                                           size_t type, int64_t variant,
                                           const rinzu_managed_slot_t *slots, size_t count) {
  uint64_t value;
  if (rinzu_managed_composite_owned(data, type, variant, slots, count, &value))
    rinzu_failure_fatal();
  ++context->allocations;
  return json_ref(value);
}

static rinzu_managed_slot_t json_enum(RinzuContext *context, rinzu_managed_data_t *data,
                                      size_t type, unsigned variant) {
  return json_composite(context, data, type, variant, NULL, 0);
}

static rinzu_managed_slot_t json_some(RinzuContext *context, rinzu_managed_data_t *data,
                                      size_t type, rinzu_managed_slot_t value) {
  return json_composite(context, data, type, 1, &value, 1);
}

static size_t json_option_child(const rinzu_managed_data_t *data, size_t type) {
  const rinzu_value_type_t *option = &data->schema->types[type];
  if (option->kind != RINZU_VALUE_ENUM || option->variant_count != 2 || option->variants[0].count ||
      option->variants[1].count != 1)
    rinzu_failure_fatal();
  return option->variants[1].types[0];
}

static rinzu_managed_slot_t json_string(RinzuContext *context, rinzu_managed_data_t *data,
                                        RinzuJsonSlice text, size_t scalars) {
  if (scalars == SIZE_MAX)
    scalars = rinzu_utf8_scalar_length(text.data, text.length);
  if (scalars == SIZE_MAX)
    rinzu_failure_fatal();
  ++context->allocations;
  return json_ref(rinzu_managed_string_copy_owned(data, text.data, text.length, scalars));
}

static rinzu_managed_slot_t json_output_copy(RinzuContext *context, rinzu_managed_data_t *data,
                                             RinzuJsonSlice text, int as_string) {
  rinzu_resource_copy(text.length);
  if (as_string)
    return json_string(context, data, text, SIZE_MAX);
  rinzu_managed_value_t *bytes = rinzu_managed_bytes_allocate_owned(data, text.length);
  if (text.length)
    memcpy(bytes->bytes, text.data, text.length);
  ++context->allocations;
  return json_ref(bytes->identity);
}

/* Transfer the completed allocation to the managed payload ledger. The spare
   byte is for native string/byte loans and the collector's relocation path. */
static rinzu_managed_slot_t json_take_buffer(RinzuContext *context, rinzu_managed_data_t *data,
                                             RinzuJsonBuffer *buffer, int as_string) {
  size_t length = buffer->length;
  size_t scalars = as_string ? rinzu_utf8_scalar_length(buffer->data, length) : 0;
  if (as_string && scalars == SIZE_MAX)
    rinzu_failure_fatal();
  *(unsigned char *)grow(buffer, 1) = 0;
  rinzu_managed_value_t *value =
      rinzu_managed_new_owned(data, as_string ? RINZU_MANAGED_STRING : RINZU_MANAGED_BYTES);
  value->length = length;
  value->scalar_length = scalars;
  value->bytes = buffer->data;
  rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, buffer->capacity);
  rinzu_managed_account_payload(data, value, buffer->capacity);
  *buffer = (RinzuJsonBuffer){0};
  ++context->allocations;
  return json_ref(value->identity);
}

static rinzu_managed_slot_t json_optional_offset(RinzuContext *context, rinzu_managed_data_t *data,
                                                 size_t type, uint32_t offset) {
  return offset == RZ_JSON_NONE ? json_enum(context, data, type, 0)
                                : json_some(context, data, type, json_bits(offset));
}

static uint64_t json_throw(RinzuContext *context, rinzu_managed_data_t *data,
                           const RinzuJsonError *error) {
  const char *names[] = {"DecodeFailed", "InvalidLimits", "LimitExceeded",
                         "TypeMismatch", "NumberError",   "SchemaMismatch"};
  const char *messages[] = {"invalid JSON input",
                            "invalid JSON limit",
                            "JSON limit exceeded",
                            "unexpected JSON value kind",
                            "JSON number conversion failed",
                            "JSON does not match the requested type"};
  if (error->code < RZ_JSON_DECODE || error->code > RZ_JSON_SCHEMA)
    rinzu_failure_fatal();
  size_t index = error->code - 1;
  size_t type = rinzu_program_json_errors[index];
  if (type >= data->schema->count)
    rinzu_failure_fatal();
  const rinzu_value_type_t *types = data->schema->types;
  const size_t *layout = types[type].fields;
  if (types[type].kind != RINZU_VALUE_ERROR_RECORD || types[type].field_count != 5)
    rinzu_failure_fatal();
  size_t payload = layout[4];
  const size_t *fields = types[payload].fields;
  rinzu_managed_slot_t slots[6] = {0};
  switch (error->code) {
  case RZ_JSON_DECODE:
    slots[0] = json_bits(error->offset);
    slots[1] = json_optional_offset(context, data, fields[1], error->first);
    slots[2] = json_enum(context, data, fields[2], error->reason);
    break;
  case RZ_JSON_INVALID_LIMIT:
    slots[0] = json_enum(context, data, fields[0], error->reason);
    slots[1] = json_bits((uint64_t)error->maximum);
    slots[2] = json_bits((uint64_t)error->minimum);
    slots[3] = json_bits((uint64_t)error->supplied);
    break;
  case RZ_JSON_LIMIT:
    slots[0] = json_optional_offset(context, data, fields[0], error->offset);
    slots[1] = json_enum(context, data, fields[1], error->reason);
    slots[2] = json_bits((uint64_t)error->limit);
    break;
  case RZ_JSON_TYPE:
    slots[0] = json_enum(context, data, fields[0], error->actual);
    slots[1] = json_enum(context, data, fields[1], error->expected);
    break;
  case RZ_JSON_NUMBER_ERROR:
    slots[0] = json_enum(context, data, fields[0], error->reason);
    break;
  case RZ_JSON_SCHEMA:
    slots[0] =
        error->reason == RZ_JSON_WRONG_KIND
            ? json_some(context, data, fields[0],
                        json_enum(context, data, json_option_child(data, fields[0]), error->actual))
            : json_enum(context, data, fields[0], 0);
    slots[1] = json_bits(error->offset);
    slots[2] = error->reason == RZ_JSON_WRONG_KIND
                   ? json_some(context, data, fields[2],
                               json_enum(context, data, json_option_child(data, fields[2]),
                                         error->expected))
                   : json_enum(context, data, fields[2], 0);
    slots[3] = error->key.data ? json_some(context, data, fields[3],
                                           json_string(context, data, error->key, SIZE_MAX))
                               : json_enum(context, data, fields[3], 0);
    slots[4] = json_string(context, data, error->path, SIZE_MAX);
    slots[5] = json_enum(context, data, fields[5], error->reason);
    break;
  default:
    rinzu_native_fatal(86);
  }
  if (types[payload].kind != RINZU_VALUE_RECORD || types[payload].field_count > 6)
    rinzu_failure_fatal();
  rinzu_managed_slot_t value =
      json_composite(context, data, payload, -1, slots, types[payload].field_count);
  rinzu_managed_slot_t children[] = {
      json_string(context, data, json_literal(names[index]), strlen(names[index])),
      json_string(context, data, json_literal(messages[index]), strlen(messages[index])),
      json_enum(context, data, layout[2], 0), json_enum(context, data, layout[3], 0), value};
  rinzu_aot_throw_error(context, json_composite(context, data, type, -1, children, 5).bits);
  return 0;
}

static uint64_t json_number_error(RinzuContext *context, rinzu_managed_data_t *data,
                                  unsigned reason) {
  RinzuJsonError error = {.code = RZ_JSON_NUMBER_ERROR, .reason = reason};
  return json_throw(context, data, &error);
}

static RinzuJsonLimits json_limits(const rinzu_managed_value_t *record, unsigned kind) {
  if (!record)
    return (RinzuJsonLimits){RZ_JSON_DEFAULT_BYTES, RZ_JSON_DEFAULT_DEPTH, RZ_JSON_DEFAULT_NODES};
  if (record->count != 3 || record->kind != RINZU_MANAGED_COMPOSITE)
    rinzu_failure_fatal();
  const rinzu_managed_slot_t *slots = record->slots;
  return (RinzuJsonLimits){(int64_t)slots[kind == RZ_JSON_INPUT ? 1 : 2].bits,
                           (int64_t)slots[0].bits,
                           (int64_t)slots[kind == RZ_JSON_INPUT ? 2 : 1].bits};
}

static rinzu_managed_slot_t json_value(RinzuContext *context, rinzu_managed_data_t *data,
                                       size_t type, unsigned tag,
                                       const rinzu_managed_slot_t *fields, size_t count) {
  const rinzu_value_type_t *layout = &data->schema->types[type];
  if (layout->kind != RINZU_VALUE_STATE || layout->field_count != 1)
    rinzu_failure_fatal();
  rinzu_managed_slot_t representation =
      json_composite(context, data, layout->fields[0], tag, fields, count);
  return json_composite(context, data, type, -1, &representation, 1);
}

static rinzu_managed_slot_t json_parsed(RinzuContext *context, rinzu_managed_data_t *data,
                                        size_t type, unsigned tag, uint64_t input, uint64_t index,
                                        uint32_t node) {
  rinzu_managed_slot_t fields[] = {json_ref(input), json_ref(index), json_bits(node)};
  return json_value(context, data, type, tag, fields, 3);
}

typedef struct {
  size_t start;
  uint32_t base, field;
} JsonBuildFrame;

typedef struct {
  RinzuJsonBuffer frames, slots;
  rinzu_managed_slot_t root;
} JsonBuilder;

static uint32_t json_base_type(uint32_t type) {
  while (rinzu_program_json_types[type].kind == RZ_JSON_OPTION_TY)
    type = rinzu_program_json_types[type].child;
  return type;
}

static rinzu_managed_slot_t json_wrap(RinzuContext *context, rinzu_managed_data_t *data,
                                      uint32_t type, uint32_t base, rinzu_managed_slot_t value) {
  if (type == base)
    return value;
  return json_some(context, data, type,
                   json_wrap(context, data, rinzu_program_json_types[type].child, base, value));
}

static JsonBuildFrame *json_frame(JsonBuilder *builder) {
  if (!builder->frames.length)
    rinzu_failure_fatal();
  return (JsonBuildFrame *)(builder->frames.data + builder->frames.length) - 1;
}

static void json_build_value(JsonBuilder *builder, rinzu_managed_slot_t value) {
  if (!builder->frames.length) {
    builder->root = value;
    return;
  }
  JsonBuildFrame *frame = json_frame(builder);
  if (frame->field != RZ_JSON_NONE) {
    ((rinzu_managed_slot_t *)builder->slots.data)[frame->start + frame->field] = value;
    frame->field = RZ_JSON_NONE;
  } else {
    append(&builder->slots, &value, sizeof(value));
  }
}

static void json_build_batch(RinzuContext *context, rinzu_managed_data_t *data,
                             JsonBuilder *builder, RinzuJsonSlice source,
                             const RinzuJsonBatch *batch) {
  for (size_t i = 0; i < batch->count; ++i) {
    const RinzuJsonEvent *event = &batch->events[i];
    uint32_t type = event->type;
    if (event->event == RZ_JSON_KEY) {
      if (event->field == RZ_JSON_NONE) {
        rinzu_managed_slot_t key = json_string(
            context, data, rinzu_json_event_text(event, source, batch->decoded), event->scalars);
        append(&builder->slots, &key, sizeof(key));
      } else {
        json_frame(builder)->field = event->field;
      }
      continue;
    }
    if (event->event == RZ_JSON_SCALAR && event->kind == RZ_JSON_NULL) {
      json_build_value(builder, json_enum(context, data, type, 0));
      continue;
    }
    uint32_t base = json_base_type(type);
    const RinzuJsonType *layout = &rinzu_program_json_types[base];
    switch (event->event) {
    case RZ_JSON_SCALAR: {
      rinzu_managed_slot_t value =
          layout->kind == RZ_JSON_STRING_TY
              ? json_string(context, data, rinzu_json_event_text(event, source, batch->decoded),
                            event->scalars)
              : json_bits(event->bits);
      json_build_value(builder, json_wrap(context, data, type, base, value));
      break;
    }
    case RZ_JSON_OPEN: {
      JsonBuildFrame frame = {builder->slots.length / sizeof(rinzu_managed_slot_t), base,
                              RZ_JSON_NONE};
      append(&builder->frames, &frame, sizeof(frame));
      if (layout->kind == RZ_JSON_RECORD_TY && layout->count) {
        size_t bytes = (size_t)layout->count * sizeof(rinzu_managed_slot_t);
        /* A record field's value can be scalar zero. The scratch-only tag
           distinguishes a missing field from any valid typed slot. */
        rinzu_managed_slot_t *slots = grow(&builder->slots, bytes);
        for (uint32_t field = 0; field < layout->count; ++field)
          slots[field] = (rinzu_managed_slot_t){0, 2};
      }
      break;
    }
    case RZ_JSON_CLOSE: {
      JsonBuildFrame frame = *json_frame(builder);
      if (frame.base != base)
        rinzu_failure_fatal();
      size_t count = builder->slots.length / sizeof(rinzu_managed_slot_t) - frame.start;
      rinzu_managed_slot_t *slots =
          count ? (rinzu_managed_slot_t *)builder->slots.data + frame.start : NULL;
      if (layout->kind == RZ_JSON_RECORD_TY)
        for (uint32_t field = 0; field < layout->count; ++field)
          if (slots[field].reference == 2) {
            uint32_t child = rinzu_program_json_fields[layout->first + field].type;
            slots[field] = json_enum(context, data, child, 0);
          }
      rinzu_managed_slot_t value = json_composite(context, data, base, -1, slots, count);
      builder->frames.length -= sizeof(JsonBuildFrame);
      builder->slots.length = frame.start * sizeof(rinzu_managed_slot_t);
      json_build_value(builder, json_wrap(context, data, type, base, value));
      break;
    }
    default:
      rinzu_native_fatal(86);
    }
  }
}

static void json_build_safepoint(RinzuContext *context, rinzu_managed_data_t *data,
                                 const JsonBuilder *builder) {
  if (!rinzu_gc_due(&data->gc))
    return;
  const rinzu_managed_slot_t *slots = (const rinzu_managed_slot_t *)builder->slots.data;
  size_t count = builder->slots.length / sizeof(*slots), roots = builder->root.reference != 0;
  for (size_t i = 0; i < count; ++i)
    roots += slots[i].reference == 1;
  uint64_t *handles = rinzu_managed_prepare_roots(&context->roots, roots);
  size_t offset = 0;
  if (builder->root.reference)
    handles[offset++] = builder->root.bits;
  for (size_t i = 0; i < count; ++i)
    if (slots[i].reference == 1)
      handles[offset++] = slots[i].bits;
  rinzu_managed_commit_roots(&context->roots, roots);
  rinzu_managed_safepoint(data, &context->roots);
}

static uint64_t json_parse(RinzuContext *context, rinzu_managed_data_t *data, size_t result,
                           uint64_t input, const rinzu_managed_value_t *bounds, int typed) {
  RinzuJsonLimits limits = json_limits(bounds, RZ_JSON_INPUT);
  const rinzu_managed_value_t *source = rinzu_managed_find(data, input);
  unsigned tag = source->kind == RINZU_MANAGED_STRING ? JSON_PARSED_STRING : JSON_PARSED_BYTES;
  RinzuJsonSchema schema = {rinzu_program_json_types, rinzu_program_json_fields,
                            rinzu_program_json_names, (uint32_t)result};
  RinzuJsonParser *parser = rinzu_json_parser_new(source->length, limits, typed ? &schema : NULL);
  JsonBuilder builder = {0};
  RinzuJsonBatch batch;
  uint64_t value = 0;
  rinzu_managed_enter(&context->roots);
  for (;;) {
    RinzuJsonSlice slice = json_slice(rinzu_managed_find(data, input));
    int status = rinzu_json_step(parser, slice.data, typed ? &batch : NULL);
    if (status == RZ_JSON_FAILED) {
      json_throw(context, data, rinzu_json_error(parser));
      break;
    }
    if (typed)
      json_build_batch(context, data, &builder, slice, &batch);
    if (status == RZ_JSON_DONE) {
      if (typed) {
        if (builder.frames.length || builder.slots.length)
          rinzu_failure_fatal();
        value = builder.root.bits;
      } else {
        RinzuJsonBuffer index = rinzu_json_index(parser);
        rinzu_managed_slot_t tape = json_take_buffer(context, data, &index, 0);
        value = json_parsed(context, data, result, tag, input, tape.bits, 0).bits;
      }
      break;
    }
    if (rinzu_aot_poll_context(context))
      break;
    json_build_safepoint(context, data, &builder);
  }
  rinzu_managed_leave(&context->roots);
  rinzu_json_buffer_free(&builder.frames);
  rinzu_json_buffer_free(&builder.slots);
  rinzu_json_parser_free(parser);
  return value;
}

typedef struct {
  const rinzu_managed_value_t *representation, *source, *index;
  uint32_t node;
  unsigned tag;
} JsonView;

static JsonView json_view(rinzu_managed_data_t *data, const rinzu_managed_value_t *value) {
  if (!value || value->kind != RINZU_MANAGED_COMPOSITE || value->count != 1 ||
      !value->slots[0].reference)
    rinzu_failure_fatal();
  const rinzu_managed_value_t *rep = rinzu_managed_find(data, value->slots[0].bits);
  if (rep->kind != RINZU_MANAGED_COMPOSITE || !rep->has_variant || rep->variant > JSON_OBJECT)
    rinzu_failure_fatal();
  JsonView view = {.representation = rep, .tag = rep->variant};
  if (view.tag <= JSON_PARSED_STRING) {
    if (rep->count != 3 || rep->slots[2].bits > UINT32_MAX)
      rinzu_failure_fatal();
    view.source = rinzu_managed_find(data, rep->slots[0].bits);
    view.index = rinzu_managed_find(data, rep->slots[1].bits);
    view.node = (uint32_t)rep->slots[2].bits;
  }
  return view;
}

static RinzuJsonDocument json_document(JsonView view) {
  RinzuJsonDocument doc;
  rinzu_json_document(&doc, json_slice(view.source), json_slice(view.index));
  return doc;
}

static RinzuJsonNode json_info(JsonView view) {
  if (view.source) {
    RinzuJsonDocument doc = json_document(view);
    return rinzu_json_node(&doc, view.node);
  }
  const unsigned kinds[] = {0,
                            0,
                            RZ_JSON_NULL,
                            RZ_JSON_BOOLEAN,
                            RZ_JSON_TEXT,
                            RZ_JSON_NUMBER,
                            RZ_JSON_NUMBER,
                            RZ_JSON_NUMBER,
                            RZ_JSON_ARRAY,
                            RZ_JSON_OBJECT};
  return (RinzuJsonNode){.kind = kinds[view.tag], .nodes = 1};
}

static uint64_t json_wrong_kind(RinzuContext *context, rinzu_managed_data_t *data,
                                unsigned expected, unsigned actual) {
  RinzuJsonError error = {.code = RZ_JSON_TYPE, .expected = expected, .actual = actual};
  return json_throw(context, data, &error);
}

static int json_numeric(rinzu_managed_data_t *data, JsonView view, int as_float, uint64_t *bits) {
  if (view.source) {
    RinzuJsonDocument doc = json_document(view);
    RinzuJsonSlice token = rinzu_json_text(&doc, view.node);
    return as_float ? rinzu_json_float(token, bits) : rinzu_json_int(token, bits);
  }
  uint64_t value = view.representation->slots[0].bits;
  switch (view.tag) {
  case JSON_TOKEN: {
    RinzuJsonSlice token = json_slice(rinzu_managed_find(data, value));
    return as_float ? rinzu_json_float(token, bits) : rinzu_json_int(token, bits);
  }
  case JSON_INT:
    if (as_float) {
      double rounded = (double)(int64_t)value;
      memcpy(bits, &rounded, sizeof(rounded));
    } else {
      *bits = value;
    }
    return -1;
  case JSON_FLOAT:
    if (!as_float)
      return RZ_JSON_INTEGER;
    *bits = value;
    return -1;
  default:
    rinzu_native_fatal(86);
  }
}

static uint64_t json_container_query(RinzuContext *context, rinzu_managed_data_t *data,
                                     int64_t operation, size_t type, size_t result, JsonView view,
                                     const rinzu_managed_value_t *key, uint64_t argument) {
  int lookup = operation == RINZU_DATA_JSON_FIELD || operation == RINZU_DATA_JSON_INT_FIELD ||
               operation == RINZU_DATA_JSON_FLOAT_FIELD;
  int length =
      operation == RINZU_DATA_JSON_ARRAY_LENGTH || operation == RINZU_DATA_JSON_OBJECT_LENGTH;
  int entry = operation == RINZU_DATA_JSON_ENTRY_AT;
  int numeric = operation == RINZU_DATA_JSON_INT_FIELD || operation == RINZU_DATA_JSON_FLOAT_FIELD;
  int as_float = operation == RINZU_DATA_JSON_FLOAT_FIELD;
  int found = 0;
  uint64_t child = 0, key_value = 0;
  uint32_t node = RZ_JSON_NONE;
  if (view.source) {
    RinzuJsonDocument doc = json_document(view);
    if (length)
      return rinzu_json_node(&doc, view.node).length;
    if (lookup)
      node = rinzu_json_find(&doc, view.node, json_slice(key));
    else if (argument <= UINT32_MAX)
      node = rinzu_json_child(&doc, view.node, (uint32_t)argument);
    found = node != RZ_JSON_NONE;
    if (found && entry) {
      key_value = json_string(context, data, rinzu_json_text(&doc, node), SIZE_MAX).bits;
      ++node;
    }
  } else {
    const rinzu_managed_value_t *collection =
        rinzu_managed_find(data, view.representation->slots[0].bits);
    size_t stride = view.tag == JSON_OBJECT ? 2 : 1;
    size_t count = collection->count / stride;
    if (length)
      return count;
    size_t selected = SIZE_MAX;
    if (lookup) {
      size_t bucket;
      uint64_t hash;
      size_t key_type = data->schema->types[collection->type].key;
      rinzu_managed_slot_t wanted = json_ref(key->identity);
      if (rinzu_managed_value_hash(data, key_type, wanted, &hash) ||
          rinzu_managed_map_probe_owned(data, key_type, collection->slots, collection->map_index,
                                        wanted, hash, &bucket, &selected))
        rinzu_failure_fatal();
    } else if (argument < count) {
      selected = (size_t)argument * stride;
    }
    found = selected != SIZE_MAX;
    if (found) {
      child = collection->slots[selected + stride - 1].bits;
      if (entry)
        key_value = collection->slots[selected].bits;
    }
  }
  if (!found)
    return json_enum(context, data, result, 0).bits;
  if (numeric) {
    if (view.source)
      view.node = node;
    else
      view = json_view(data, rinzu_managed_find(data, child));
    unsigned kind = json_info(view).kind;
    if (kind != RZ_JSON_NUMBER)
      return json_wrong_kind(context, data, RZ_JSON_NUMBER, kind);
    uint64_t converted;
    int reason = json_numeric(data, view, as_float, &converted);
    return reason >= 0 ? json_number_error(context, data, (unsigned)reason)
                       : json_some(context, data, result, json_bits(converted)).bits;
  }
  if (view.source)
    child = json_parsed(context, data, type, view.tag, view.source->identity, view.index->identity,
                        node)
                .bits;
  if (entry) {
    rinzu_managed_slot_t fields[] = {json_ref(key_value), json_ref(child)};
    child = json_composite(context, data, json_option_child(data, result), -1, fields, 2).bits;
  }
  return json_some(context, data, result, json_ref(child)).bits;
}

static uint64_t json_query(RinzuContext *context, rinzu_managed_data_t *data, int64_t operation,
                           size_t result, const uint64_t *types, const uint64_t *bits,
                           rinzu_managed_value_t *const *values) {
  if (operation == RINZU_DATA_JSON_NULL)
    return json_value(context, data, result, JSON_NULL, NULL, 0).bits;
  if (operation == RINZU_DATA_JSON_CONSTRUCT || operation == RINZU_DATA_JSON_NUMBER ||
      operation == RINZU_DATA_JSON_FLOAT_VALUE) {
    unsigned tag;
    if (operation == RINZU_DATA_JSON_NUMBER) {
      if (!rinzu_json_number_valid(json_slice(values[0])))
        return json_number_error(context, data, RZ_JSON_SYNTAX);
      tag = JSON_TOKEN;
    } else if (operation == RINZU_DATA_JSON_FLOAT_VALUE) {
      double value;
      memcpy(&value, bits, sizeof(value));
      if (!isfinite(value))
        return json_number_error(context, data, RZ_JSON_NONFINITE);
      tag = JSON_FLOAT;
    } else {
      const rinzu_value_type_t *input = &data->schema->types[types[0]];
      switch (input->kind) {
      case RINZU_VALUE_STRING:
        tag = JSON_TEXT;
        break;
      case RINZU_VALUE_LIST:
        tag = JSON_ARRAY;
        break;
      case RINZU_VALUE_MAP:
        tag = JSON_OBJECT;
        break;
      case RINZU_VALUE_SCALAR:
        tag = input->scalar_kind == RINZU_SCALAR_BOOL ? JSON_BOOL : JSON_INT;
        break;
      default:
        rinzu_native_fatal(86);
      }
    }
    rinzu_managed_slot_t value = {bits[0], rinzu_program_slot_references[types[0]]};
    return json_value(context, data, result, tag, &value, 1).bits;
  }
  JsonView view = json_view(data, values[0]);
  RinzuJsonNode info = json_info(view);
  unsigned expected;
  switch (operation) {
  case RINZU_DATA_JSON_KIND:
    return json_enum(context, data, result, info.kind).bits;
  case RINZU_DATA_JSON_FIELD:
  case RINZU_DATA_JSON_INT_FIELD:
  case RINZU_DATA_JSON_FLOAT_FIELD:
  case RINZU_DATA_JSON_ENTRY_AT:
  case RINZU_DATA_JSON_OBJECT_LENGTH:
    expected = RZ_JSON_OBJECT;
    break;
  case RINZU_DATA_JSON_AT:
  case RINZU_DATA_JSON_ARRAY_LENGTH:
    expected = RZ_JSON_ARRAY;
    break;
  case RINZU_DATA_JSON_BOOL:
    expected = RZ_JSON_BOOLEAN;
    break;
  case RINZU_DATA_JSON_STRING:
    expected = RZ_JSON_TEXT;
    break;
  default:
    expected = RZ_JSON_NUMBER;
    break;
  }
  if (expected != info.kind)
    return json_wrong_kind(context, data, expected, info.kind);
  if (expected == RZ_JSON_ARRAY || expected == RZ_JSON_OBJECT) {
    uint64_t argument =
        operation == RINZU_DATA_JSON_AT || operation == RINZU_DATA_JSON_ENTRY_AT ? bits[1] : 0;
    return json_container_query(context, data, operation, types[0], result, view, values[1],
                                argument);
  }
  if (operation == RINZU_DATA_JSON_BOOL)
    return view.source ? view.source->bytes[info.start] == 't' : view.representation->slots[0].bits;
  if (operation == RINZU_DATA_JSON_INT || operation == RINZU_DATA_JSON_FLOAT) {
    uint64_t value;
    int reason = json_numeric(data, view, operation == RINZU_DATA_JSON_FLOAT, &value);
    return reason >= 0 ? json_number_error(context, data, (unsigned)reason) : value;
  }
  if (view.source) {
    RinzuJsonDocument doc = json_document(view);
    return json_string(context, data, rinzu_json_text(&doc, view.node), SIZE_MAX).bits;
  }
  uint64_t value = view.representation->slots[0].bits;
  if (view.tag == JSON_TEXT || view.tag == JSON_TOKEN)
    return value;
  unsigned char digits[32];
  size_t n = view.tag == JSON_FLOAT ? rinzu_json_format_float(value, digits)
                                    : rinzu_json_format_int(value, digits);
  return json_string(context, data, (RinzuJsonSlice){digits, n}, n).bits;
}

typedef struct {
  RinzuJsonLimits limits;
  uint64_t nodes;
  RinzuJsonBuffer buffer;
  unsigned exceeded, interrupted;
  RinzuContext *context;
} JsonOutput;

typedef struct {
  uint64_t value;
  size_t index;
  uint32_t depth;
  unsigned container;
} JsonWrite;

static int json_count(JsonOutput *out, uint32_t depth, uint32_t nodes) {
  if (depth > (uint64_t)out->limits.depth) {
    out->exceeded = RZ_JSON_DEPTH;
    return 0;
  }
  out->nodes += nodes;
  if (out->nodes > (uint64_t)out->limits.nodes) {
    out->exceeded = RZ_JSON_NODES;
    return 0;
  }
  return 1;
}

static int json_append(JsonOutput *out, RinzuJsonSlice text, int escape) {
  int written = escape ? rinzu_json_escape(&out->buffer, (size_t)out->limits.bytes, text)
                       : rinzu_json_append(&out->buffer, (size_t)out->limits.bytes, text);
  if (!written)
    out->exceeded = RZ_JSON_OUTPUT;
  return written;
}

static int json_write(JsonOutput *out, RinzuJsonSlice text, int quote) {
  if (quote && !json_append(out, json_literal("\""), 0))
    return 0;
  for (size_t start = 0; start < text.length;) {
    if (start && rinzu_aot_poll_context(out->context)) {
      out->interrupted = 1;
      return 0;
    }
    size_t n = text.length - start;
    if (n > SCAN_BYTES)
      n = SCAN_BYTES;
    if (!json_append(out, (RinzuJsonSlice){text.data + start, n}, quote))
      return 0;
    start += n;
  }
  return !quote || json_append(out, json_literal("\""), 0);
}

static RinzuJsonSlice json_range(JsonView view, RinzuJsonNode info) {
  return (RinzuJsonSlice){view.source->bytes + info.start, info.end - info.start};
}

static uint64_t json_encode(RinzuContext *context, rinzu_managed_data_t *data, size_t result,
                            const rinzu_managed_value_t *input,
                            const rinzu_managed_value_t *bounds) {
  JsonOutput out = {.limits = json_limits(bounds, RZ_JSON_OUTPUT), .context = context};
  RinzuJsonError error;
  if (!rinzu_json_limits(out.limits, RZ_JSON_OUTPUT, &error))
    return json_throw(context, data, &error);
  int as_string = data->schema->types[result].kind == RINZU_VALUE_STRING;
  JsonView initial = json_view(data, input);
  if (initial.source) {
    RinzuJsonNode info = json_info(initial);
    if (json_count(&out, info.depth, info.nodes)) {
      RinzuJsonSlice range = initial.node ? json_range(initial, info) : json_slice(initial.source);
      if (range.length <= (size_t)out.limits.bytes) {
        if (!initial.node && as_string == (initial.tag == JSON_PARSED_STRING))
          return initial.source->identity;
        return json_output_copy(context, data, range, as_string).bits;
      }
      out.exceeded = RZ_JSON_OUTPUT;
    }
  } else {
    RinzuJsonBuffer stack = {0};
    JsonWrite root = {.value = input->identity};
    append(&stack, &root, sizeof(root));
    size_t last_poll = 0;
    unsigned work = 0;
    while (stack.length && !out.exceeded && !out.interrupted) {
      if (++work == SCAN_WORK || out.buffer.length - last_poll >= SCAN_BYTES) {
        if (rinzu_aot_poll_context(context)) {
          out.interrupted = 1;
          break;
        }
        work = 0;
        last_poll = out.buffer.length;
      }
      stack.length -= sizeof(JsonWrite);
      JsonWrite action;
      memcpy(&action, stack.data + stack.length, sizeof(action));
      const rinzu_managed_value_t *value = rinzu_managed_find(data, action.value);
      if (action.container) {
        int object = action.container == JSON_OBJECT;
        size_t stride = object ? 2 : 1;
        size_t offset = action.index * stride;
        if (offset == value->count) {
          json_write(&out, json_literal(object ? "}" : "]"), 0);
          continue;
        }
        if (offset && !json_write(&out, json_literal(","), 0))
          break;
        if (object &&
            (!json_count(&out, action.depth, 1) ||
             !json_write(&out, json_slice(rinzu_managed_find(data, value->slots[offset].bits)),
                         1) ||
             !json_write(&out, json_literal(":"), 0)))
          break;
        ++action.index;
        append(&stack, &action, sizeof(action));
        JsonWrite child = {.value = value->slots[offset + stride - 1].bits, .depth = action.depth};
        append(&stack, &child, sizeof(child));
        continue;
      }
      JsonView view = json_view(data, value);
      if (view.source) {
        RinzuJsonNode info = json_info(view);
        if (!json_count(&out, action.depth + info.depth, info.nodes) ||
            !json_write(&out, json_range(view, info), 0))
          break;
        continue;
      }
      unsigned is_container = view.tag == JSON_ARRAY || view.tag == JSON_OBJECT;
      if (!json_count(&out, action.depth + is_container, 1))
        break;
      uint64_t child = view.tag == JSON_NULL ? 0 : view.representation->slots[0].bits;
      switch (view.tag) {
      case JSON_NULL:
        json_write(&out, json_literal("null"), 0);
        break;
      case JSON_BOOL:
        json_write(&out, json_literal(child ? "true" : "false"), 0);
        break;
      case JSON_TEXT:
      case JSON_TOKEN:
        json_write(&out, json_slice(rinzu_managed_find(data, child)), view.tag == JSON_TEXT);
        break;
      case JSON_INT:
      case JSON_FLOAT: {
        unsigned char digits[32];
        size_t length = view.tag == JSON_INT ? rinzu_json_format_int(child, digits)
                                             : rinzu_json_format_float(child, digits);
        json_write(&out, (RinzuJsonSlice){digits, length}, 0);
        break;
      }
      case JSON_ARRAY:
      case JSON_OBJECT: {
        json_write(&out, json_literal(view.tag == JSON_ARRAY ? "[" : "{"), 0);
        JsonWrite collection = {.value = child, .depth = action.depth + 1, .container = view.tag};
        append(&stack, &collection, sizeof(collection));
        break;
      }
      default:
        rinzu_native_fatal(86);
      }
    }
    rinzu_json_buffer_free(&stack);
    if (out.interrupted) {
      rinzu_json_buffer_free(&out.buffer);
      return 0;
    }
    if (!out.exceeded)
      return json_take_buffer(context, data, &out.buffer, as_string).bits;
  }
  rinzu_json_buffer_free(&out.buffer);
  int64_t limit = out.exceeded == RZ_JSON_DEPTH   ? out.limits.depth
                  : out.exceeded == RZ_JSON_NODES ? out.limits.nodes
                                                  : out.limits.bytes;
  error = (RinzuJsonError){
      .code = RZ_JSON_LIMIT, .reason = out.exceeded, .limit = limit, .offset = RZ_JSON_NONE};
  return json_throw(context, data, &error);
}

static int64_t rinzu_aot_json_operation(RinzuContext *context, rinzu_managed_data_t *data,
                                        int64_t operation, size_t result, const uint64_t *types,
                                        const uint64_t *bits,
                                        rinzu_managed_value_t *const *values) {
  switch (operation) {
  case RINZU_DATA_JSON_PARSE:
  case RINZU_DATA_JSON_PARSE_WITH_LIMITS:
  case RINZU_DATA_JSON_DECODE:
  case RINZU_DATA_JSON_DECODE_WITH_LIMITS:
    return (int64_t)json_parse(context, data, result, bits[0], values[1],
                               operation == RINZU_DATA_JSON_DECODE ||
                                   operation == RINZU_DATA_JSON_DECODE_WITH_LIMITS);
  case RINZU_DATA_JSON_ENCODE:
  case RINZU_DATA_JSON_ENCODE_WITH_LIMITS:
    return (int64_t)json_encode(context, data, result, values[0], values[1]);
  default:
    return (int64_t)json_query(context, data, operation, result, types, bits, values);
  }
}
#endif

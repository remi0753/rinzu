static void rinzu_aot_throw_error(RinzuContext *, uint64_t);

static __attribute__((noinline, cold)) int64_t rinzu_aot_collection_overflow(RinzuContext *context,
                                                                             size_t type,
                                                                             size_t current,
                                                                             size_t added) {
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (type >= data->schema->count || current > INT64_MAX || added > INT64_MAX)
    rinzu_failure_fatal();
  const rinzu_value_type_t *error = &data->schema->types[type];
  if (error->kind != RINZU_VALUE_ERROR_RECORD || error->field_count != 5)
    rinzu_failure_fatal();
  uint64_t source = 0, cause = 0, payload = 0, output = 0;
  rinzu_managed_slot_t lengths[] = {{added, 0}, {current, 0}};
  if (rinzu_managed_composite_owned(data, error->fields[2], 0, NULL, 0, &source) ||
      rinzu_managed_composite_owned(data, error->fields[3], 0, NULL, 0, &cause) ||
      rinzu_managed_composite_owned(data, error->fields[4], -1, lengths, 2, &payload))
    rinzu_failure_fatal();
  static const unsigned char name[] = "LengthOverflow";
  static const unsigned char message[] = "collection length exceeds Int";
  rinzu_managed_slot_t slots[] = {
      {rinzu_managed_string(data, name, sizeof(name) - 1), 1},
      {rinzu_managed_string(data, message, sizeof(message) - 1), 1},
      {source, 1},
      {cause, 1},
      {payload, 1},
  };
  if (rinzu_managed_composite_owned(data, type, -1, slots, 5, &output))
    rinzu_failure_fatal();
  context->allocations += 6;
  rinzu_aot_throw_error(context, output);
  return 0;
}

static rinzu_managed_value_t *rinzu_aot_concat_part(rinzu_managed_data_t *data, size_t type,
                                                    rinzu_managed_slot_t slot) {
  rinzu_managed_value_t *part = NULL;
  if (!slot.reference || rinzu_managed_composite_resolve_owned(data, type, slot.bits, &part) ||
      part->collection_building)
    rinzu_failure_fatal();
  return part;
}

static int64_t rinzu_aot_collection_construction(RinzuContext *context, rinzu_managed_data_t *data,
                                                 int64_t operation, size_t result,
                                                 const uint64_t *types, const uint64_t *bits,
                                                 size_t count, rinzu_managed_value_t *source) {
  size_t input = (size_t)types[0];
  const rinzu_value_type_t *layout = &data->schema->types[input];
  int map = layout->kind == RINZU_VALUE_MAP;
  if (!source || source->kind != RINZU_MANAGED_COMPOSITE || source->has_variant ||
      (!map && layout->kind != RINZU_VALUE_LIST))
    rinzu_failure_fatal();
  if (operation == RINZU_DATA_LIST_CONCAT_ALL) {
    if (count != 1 || map || layout->element != result || source->collection_building ||
        data->schema->types[result].kind != RINZU_VALUE_LIST)
      rinzu_failure_fatal();
    size_t length = 0;
    for (size_t index = 0; index < source->count; ++index) {
      rinzu_managed_value_t *part = rinzu_aot_concat_part(data, result, source->slots[index]);
      if (!rinzu_collection_length_fits(length, part->count))
        return rinzu_aot_collection_overflow(context, rinzu_list_length_overflow, length,
                                             part->count);
      length += part->count;
    }
    rinzu_managed_value_t *output = rinzu_managed_collection_allocate_owned(data, result, length);
    for (size_t index = 0; index < source->count; ++index) {
      rinzu_managed_value_t *part = rinzu_aot_concat_part(data, result, source->slots[index]);
      if (part->count)
        memcpy(output->slots + output->count, part->slots, part->count * sizeof(*part->slots));
      output->count += part->count;
    }
    context->allocations++;
    return (int64_t)output->identity;
  }
  if (operation == RINZU_DATA_MAP_ENTRIES) {
    const rinzu_value_type_t *list = &data->schema->types[result];
    if (count != 1 || !map || list->kind != RINZU_VALUE_LIST || source->collection_building)
      rinzu_failure_fatal();
    const rinzu_value_type_t *entry = &data->schema->types[list->element];
    if (entry->kind != RINZU_VALUE_RECORD || entry->field_count != 2 ||
        entry->fields[0] != layout->key || entry->fields[1] != layout->value)
      rinzu_failure_fatal();
    rinzu_managed_value_t *output =
        rinzu_managed_collection_allocate_owned(data, result, source->count / 2);
    for (size_t index = 0; index < source->count; index += 2) {
      uint64_t pair = 0;
      if (rinzu_managed_composite_owned(data, list->element, -1, source->slots + index, 2, &pair))
        rinzu_failure_fatal();
      output->slots[output->count++] = (rinzu_managed_slot_t){pair, 1};
    }
    context->allocations += output->count + 1;
    return (int64_t)output->identity;
  }
  if ((!source->collection_building && source->count) ||
      (operation != RINZU_DATA_MAP_BUFFER_INDEX && input != result))
    rinzu_failure_fatal();
  if (operation == RINZU_DATA_COLLECTION_FINISH) {
    if (count != 1)
      rinzu_failure_fatal();
    source->collection_building = 0;
    return (int64_t)source->identity;
  }
  if (count < 2)
    rinzu_failure_fatal();
  rinzu_managed_slot_t item = {bits[1], rinzu_program_slot_references[types[1]]};
  if (operation == RINZU_DATA_LIST_BUFFER_RESERVE) {
    const rinzu_value_type_t *integer = &data->schema->types[types[1]];
    if (map || count != 2 || source->collection_building || (int64_t)bits[1] < 0 ||
        integer->kind != RINZU_VALUE_SCALAR || integer->scalar_kind != RINZU_SCALAR_I64)
      rinzu_failure_fatal();
    if (!bits[1])
      return (int64_t)source->identity;
    rinzu_managed_value_t *output =
        rinzu_managed_collection_allocate_owned(data, result, (size_t)bits[1]);
    output->collection_capacity = (size_t)bits[1];
    output->collection_building = 1;
    context->allocations++;
    return (int64_t)output->identity;
  }
  if (operation == RINZU_DATA_LIST_BUFFER_APPEND) {
    if (map || count != 2 || types[1] != layout->element)
      rinzu_failure_fatal();
    if (!rinzu_collection_length_fits(source->count, 1))
      return rinzu_aot_collection_overflow(context, rinzu_list_length_overflow, source->count, 1);
    if (!source->collection_building) {
      source = rinzu_managed_collection_allocate_owned(data, input, 0);
      source->collection_building = 1;
    }
    if (source->count == source->collection_capacity)
      rinzu_managed_collection_grow_owned(
          data, source, rinzu_managed_collection_doubled(source->collection_capacity, 1), 0);
    source->slots[source->count++] = item;
  } else {
    if (!map || types[1] != layout->key)
      rinzu_failure_fatal();
    uint64_t hash = 0;
    size_t bucket = 0, selected = SIZE_MAX;
    if (rinzu_managed_value_hash(data, layout->key, item, &hash) ||
        rinzu_managed_map_probe_owned(data, layout->key, source->slots, source->map_index, item,
                                      hash, &bucket, &selected))
      rinzu_failure_fatal();
    if (operation == RINZU_DATA_MAP_BUFFER_INDEX) {
      const rinzu_value_type_t *integer = &data->schema->types[result];
      if (count != 2 || integer->kind != RINZU_VALUE_SCALAR ||
          integer->scalar_kind != RINZU_SCALAR_I64)
        rinzu_failure_fatal();
      return selected == SIZE_MAX ? -1 : (int64_t)(selected / 2);
    }
    if (operation != RINZU_DATA_MAP_BUFFER_INSERT || count != 3 || types[2] != layout->value)
      rinzu_failure_fatal();
    rinzu_managed_slot_t value = {bits[2], rinzu_program_slot_references[types[2]]};
    if (selected == SIZE_MAX) {
      if (!rinzu_collection_length_fits(source->count / 2, 1))
        return rinzu_aot_collection_overflow(context, rinzu_map_length_overflow, source->count / 2,
                                             1);
      if (!source->collection_building) {
        source = rinzu_managed_collection_allocate_owned(data, input, 0);
        source->collection_building = 1;
      }
      if (source->count == source->collection_capacity) {
        rinzu_managed_collection_grow_owned(
            data, source, rinzu_managed_collection_doubled(source->collection_capacity, 2), 1);
        rinzu_managed_map_index_add(source->map_index, hash, source->count);
      } else {
        source->map_index->buckets[bucket] = (rinzu_managed_map_bucket_t){hash, source->count + 1};
      }
      source->slots[source->count++] = item;
      source->slots[source->count++] = value;
    } else {
      source->slots[selected + 1] = value;
    }
  }
  // A successful source update retains one logical charge, even in place.
  context->allocations++;
  return (int64_t)source->identity;
}

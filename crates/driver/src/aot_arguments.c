/* argv is owned by the launcher for the entire Main invocation. The cached
   snapshot is a typed heap root shared by tasks in that Main Domain. */
static int64_t rinzu_aot_program_arguments(RinzuContext *context, rinzu_managed_data_t *data,
                                           size_t result) {
  if (!data->program_argv || data->program_argc < 0)
    rinzu_failure_fatal();
  if (data->program_arguments)
    return (int64_t)data->program_arguments;

  const rinzu_value_type_t *types = data->schema->types;
  const rinzu_value_type_t *layout = &types[result];
  if (layout->kind != RINZU_VALUE_ENUM || layout->variant_count != 2 ||
      layout->variants[0].count != 1 || layout->variants[1].count != 2)
    rinzu_failure_fatal();
  size_t list = layout->variants[0].types[0];
  if (types[list].kind != RINZU_VALUE_LIST || types[types[list].element].kind != RINZU_VALUE_STRING)
    rinzu_failure_fatal();

  size_t count = data->program_argc > 0 ? (size_t)data->program_argc - 1 : 0;
  typedef struct {
    size_t length, scalars;
  } Argument;
  if (count > SIZE_MAX / sizeof(Argument) || count > SIZE_MAX / sizeof(rinzu_managed_slot_t))
    rinzu_resource_fatal();
  Argument *arguments = count ? rinzu_failure_allocate(count * sizeof(*arguments)) : NULL;
  for (size_t index = 0; index < count; ++index) {
    const unsigned char *bytes = (const unsigned char *)data->program_argv[index + 1];
    size_t length = strlen((const char *)bytes);
    size_t invalid;
    size_t scalars = rinzu_utf8_scalar_length_at(bytes, length, &invalid);
    if (scalars == SIZE_MAX) {
      free(arguments);
      if (invalid > INT64_MAX)
        rinzu_resource_fatal();
      rinzu_managed_slot_t failure[] = {{index, 0}, {invalid, 0}};
      uint64_t output;
      if (rinzu_managed_composite_owned(data, result, 1, failure, 2, &output))
        rinzu_failure_fatal();
      ++context->allocations;
      return (int64_t)output;
    }
    arguments[index] = (Argument){length, scalars};
  }

  rinzu_managed_slot_t *slots = count ? rinzu_failure_allocate(count * sizeof(*slots)) : NULL;
  for (size_t index = 0; index < count; ++index) {
    const unsigned char *bytes = (const unsigned char *)data->program_argv[index + 1];
    uint64_t string = rinzu_managed_string_copy_owned(data, bytes, arguments[index].length,
                                                      arguments[index].scalars);
    slots[index] = (rinzu_managed_slot_t){string, 1};
  }
  free(arguments);
  uint64_t values;
  if (rinzu_managed_composite_owned(data, list, -1, slots, count, &values))
    rinzu_failure_fatal();
  free(slots);
  rinzu_managed_slot_t ready = {values, 1};
  if (rinzu_managed_composite_owned(data, result, 0, &ready, 1, &data->program_arguments))
    rinzu_failure_fatal();
  context->allocations += count + 2;
  return (int64_t)data->program_arguments;
}

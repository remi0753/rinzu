/* Report-time formatting only; snapshots retain no source text or frames. */
static void rinzu_error_trace_print_string(const char *text, size_t length) {
  printf("\"");
  for (size_t i = 0; i < length; ++i) {
    unsigned char byte = (unsigned char)text[i];
    if (byte == '"' || byte == '\\') printf("\\%c", byte);
    else if (byte < 32) printf("\\u%04x", (unsigned)byte);
    else printf("%c", byte);
  }
  printf("\"");
}
static void rinzu_error_trace_print(const rinzu_error_trace_t *trace) {
  if (!trace) {printf("null"); return;}
  printf("[");
  for (size_t i = 0; i < trace->count; ++i) {
    const rinzu_error_trace_frame_t *frame = &trace->frames[i];
    if (i) printf(",");
    printf("{\"function\":");
    rinzu_error_trace_print_string(frame->function, frame->function_length);
    printf(",\"source\":");
    if (frame->source.present) {
      printf("{\"file\":%llu,\"startByte\":%u,\"endByte\":%u",
          (unsigned long long)frame->source.file, frame->source.start_byte, frame->source.end_byte);
      const rinzu_source_entry_t *position = NULL;
      size_t count = sizeof rinzu_program_sources / sizeof *rinzu_program_sources - 1;
      /* Sorted compiler metadata, with no filesystem dependency. */
      size_t low = 0, high = count;
      while (low < high) {
        size_t middle = low + (high - low) / 2;
        const rinzu_source_entry_t *entry = &rinzu_program_sources[middle];
        if (entry->file < frame->source.file ||
            (entry->file == frame->source.file && entry->byte < frame->source.start_byte)) low = middle + 1;
        else high = middle;
      }
      if (low < count && rinzu_program_sources[low].file == frame->source.file &&
          rinzu_program_sources[low].byte == frame->source.start_byte) position = &rinzu_program_sources[low];
      if (position) {
        printf(",\"name\":"); rinzu_error_trace_print_string(position->name, position->name_length);
        printf(",\"line\":%u,\"column\":%u", position->line, position->column);
      }
      printf("}");
    } else printf("null");
    printf("}");
  }
  printf("]");
}
static void rinzu_error_trace_print_failure(const rinzu_failure_t *failure) {
  printf("[");
  if (failure->primary) {
    rinzu_error_trace_print(failure->primary_trace);
    for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
      printf(","); rinzu_error_trace_print(node->trace);
    }
  }
  printf("]");
}
static void rinzu_failure_print_traces(const rinzu_failure_state_t *state) {
  if (!state->trace_stack || !state->trace_stack->enabled) return;
  const rinzu_observed_error_t **ordered = rinzu_failure_observed_order(state);
  printf("RINZU_ERROR_TRACES={\"current\":");
  rinzu_error_trace_print_failure(&state->current);
  printf(",\"caught\":[");
  for (size_t index = 0; index < (size_t)state->observed_count; ++index) {
    if (index) printf(",");
    rinzu_error_trace_print_failure(&ordered[index]->failure);
  }
  printf("]}\n");
  free(ordered);
}

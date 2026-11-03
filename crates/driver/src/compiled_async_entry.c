int rinzu_jit_run(const uintptr_t *functions, size_t function_count, const uintptr_t *stubs,
                  size_t stub_count, const char *anchor, const char *directory,
                  uint64_t cancel_after, uint64_t shutdown_after,
                  uint64_t (*context_identity)(void), rinzu_jit_timing_t *timing, char **report, size_t *length) {
  if (rinzu_jit_used || function_count != RINZU_JIT_FUNCTION_COUNT ||
      stub_count != RINZU_JIT_STUB_COUNT || !functions || (stub_count && !stubs) || !anchor ||
      !directory || !context_identity || !report || !length)
    _Exit(86);
  rinzu_jit_used = 1;
  rinzu_jit_timing = timing;
  rinzu_jit_context_identity = context_identity;
  for (size_t index = 0; index < function_count; ++index) {
    if (!functions[index])
      _Exit(86);
    rinzu_jit_functions[index] = functions[index];
  }
  for (size_t index = 0; index < stub_count; ++index) {
    if (!stubs[index])
      _Exit(86);
    rinzu_jit_stubs[index] = stubs[index];
  }
  rinzu_jit_directory = directory;
  rinzu_jit_cancel_after = cancel_after;
  rinzu_jit_shutdown_after = shutdown_after;
  char *arguments[2] = {(char *)anchor, NULL};
  uint64_t started = timing ? rinzu_jit_clock() : 0;
  int result = rinzu_jit_main(1, arguments);
  if (timing) {
    uint64_t ended = rinzu_jit_clock();
    timing->main = started && ended >= started ? ended - started : 0;
  }
  rinzu_jit_timing = NULL;
  memset(rinzu_jit_functions, 0, sizeof rinzu_jit_functions);
  memset(rinzu_jit_stubs, 0, sizeof rinzu_jit_stubs);
  rinzu_jit_directory = NULL;
  rinzu_jit_context_identity = NULL;
  *report = rinzu_jit_report;
  *length = rinzu_jit_report_size;
  rinzu_jit_report = NULL;
  rinzu_jit_report_size = rinzu_jit_report_capacity = 0;
  return result;
}
void rinzu_jit_report_free(char *report) {
  free(report);
}

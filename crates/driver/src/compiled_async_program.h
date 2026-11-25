/* Private declarations shared by the two generated JIT translation units. */
#ifndef RINZU_COMPILED_ASYNC_PROGRAM_H
#define RINZU_COMPILED_ASYNC_PROGRAM_H
RINZU_PROGRAM_API uint64_t rinzu_jit_clock(void);
RINZU_PROGRAM_API void rinzu_jit_publish(void);
RINZU_PROGRAM_API int rinzu_jit_printf(const char *format, ...);
#ifdef RINZU_AOT_HAS_SOURCE_NATIVE
RINZU_PROGRAM_API char *rinzu_source_executable_directory(void);
RINZU_PROGRAM_API char *rinzu_source_path(const char *root, const char *relative);
RINZU_PROGRAM_API void rinzu_source_file(const char *path, const unsigned char expected[32]);
RINZU_PROGRAM_API void rinzu_source_table(const char *path, const uint8_t *wire, size_t wire_length,
                                          const RinzuExpectedImport *expected, size_t count,
                                          const void **staged, const char *facade_identity,
                                          const char *directory, const RinzuNativeImage *images,
                                          size_t image_count, const RinzuNativeSlot *slots,
                                          size_t slot_count, const char *digest);
#endif
RINZU_PROGRAM_API const rinzu_native_record_layout_t *rinzu_native_record_layout(uint64_t index);
RINZU_PROGRAM_API void rinzu_aot_async_event(void *raw, unsigned kind, uint64_t future);
RINZU_PROGRAM_API void rinzu_aot_async_admission(rinzu_async_future_t *future, void *raw);
RINZU_PROGRAM_API void rinzu_aot_async_credit_evidence(RinzuAsyncState *state);
RINZU_PROGRAM_API void rinzu_aot_future_begin(void *raw);
RINZU_PROGRAM_API uint64_t rinzu_aot_future_mark(void *raw, uint64_t handle);
RINZU_PROGRAM_API void rinzu_aot_future_sweep(void *raw);
RINZU_PROGRAM_API int rinzu_aot_future_due(void *raw);
RINZU_PROGRAM_API void rinzu_aot_async_close(RinzuContext *context);
RINZU_PROGRAM_API void rinzu_aot_async_print(RinzuAsyncState *state, int baseline);
RINZU_PROGRAM_API rinzu_async_future_t *
rinzu_aot_typed_worker(RinzuContext *context, const size_t *types, const uint64_t *arguments,
                       size_t count, unsigned result_kind, size_t result_type,
                       void (*run)(rinzu_async_future_t *), const char *label);
RINZU_PROGRAM_API int rinzu_aot_typed_prepare_worker(RinzuContext *context,
                                                     rinzu_async_future_t *future);
RINZU_PROGRAM_API void rinzu_aot_typed_encode_result(RinzuContext *context,
                                                     rinzu_async_future_t *future);
RINZU_PROGRAM_API void rinzu_aot_typed_finish_worker(RinzuContext *context,
                                                     rinzu_async_future_t *future);
RINZU_PROGRAM_API void rinzu_aot_typed_heap_evidence(RinzuAsyncState *state);
RINZU_PROGRAM_API rinzu_owner_transition_t *rinzu_context_transitions(RinzuContext *context);
RINZU_PROGRAM_API void rinzu_aot_userdata_evidence(RinzuContext *context);
RINZU_PROGRAM_API void rinzu_aot_userdata_finish(RinzuContext *context);
RINZU_PROGRAM_API void rinzu_failure_print_traces(const rinzu_failure_state_t *state);
RINZU_PROGRAM_API void rinzu_aot_source_native_startup(void);
RINZU_PROGRAM_API const char *rinzu_error_trace_function_name(int64_t);
RINZU_PROGRAM_API void rinzu_aot_async_prepare(RinzuAsyncTask *);
RINZU_PROGRAM_API int64_t rinzu_aot_async_entry(RinzuAsyncTask *);
RINZU_PROGRAM_API rinzu_async_future_t *rinzu_aot_worker_request(RinzuContext *, int64_t,
                                                                 const uint64_t *, int64_t);
RINZU_PROGRAM_API void rinzu_report_begin(void);
RINZU_PROGRAM_API int rinzu_report_flush(void);
RINZU_PROGRAM_API int rinzu_aot_source_native_baseline(void);
RINZU_PROGRAM_API void rinzu_aot_source_native_evidence(void);
RINZU_PROGRAM_API char *rinzu_jit_directory_copy(void);
#ifdef RINZU_AOT_HAS_SUPPLANT
RINZU_PROGRAM_API void rinzu_aot_supplant_init(RinzuAsyncState *);
RINZU_PROGRAM_API void rinzu_aot_supplant_finish(RinzuAsyncState *);
#endif
#ifdef RINZU_AOT_HAS_FILE_READ
RINZU_PROGRAM_API int64_t rinzu_baseline_async_file(void *, int64_t);
#endif
RINZU_PROGRAM_API int rinzu_jit_main(int, char **);
int64_t rinzu_baseline_resolve_dispatch(void *, int64_t);
void rinzu_baseline_leave_dispatch(void *);
RINZU_PROGRAM_API int rinzu_resource_report_start(const char *, uint64_t);
RINZU_PROGRAM_API int rinzu_resource_report_finish(void);
extern rinzu_jit_timing_t *rinzu_jit_timing;
extern uint64_t rinzu_jit_cancel_after, rinzu_jit_shutdown_after, rinzu_jit_cancel_worker_after;
extern uintptr_t rinzu_jit_functions[], rinzu_jit_stubs[];
extern const size_t rinzu_jit_function_count, rinzu_jit_stub_count;
extern const size_t rinzu_program_type_count, rinzu_program_source_count;
extern const size_t rinzu_list_length_overflow, rinzu_map_length_overflow;
#ifdef RINZU_AOT_HAS_JSON
extern const uint32_t rinzu_program_json_errors[];
extern const RinzuJsonType rinzu_program_json_types[];
extern const RinzuJsonField rinzu_program_json_fields[];
extern const unsigned char rinzu_program_json_names[];
#endif
extern const rinzu_value_type_t rinzu_program_types[];
extern const unsigned char rinzu_program_slot_references[];
extern const rinzu_display_type_t rinzu_program_display[];
extern const rinzu_source_entry_t rinzu_program_sources[];
extern const char rinzu_program_resource_sources[];
extern rinzu_value_schema_t rinzu_program_schema;
RINZU_PROGRAM_API void rinzu_configure_program_sources(rinzu_managed_data_t *);
#ifdef RINZU_AOT_HAS_SOURCE_NATIVE
extern const RinzuNativePlan rinzu_native_plans[];
extern const size_t rinzu_native_plan_count, rinzu_native_target_count;
extern const void *rinzu_native_addresses[], *rinzu_native_target_addresses[];
extern const RinzuNativeTarget rinzu_native_targets[];
#endif
extern const rinzu_native_record_layout_t rinzu_native_record_layouts[];
extern const size_t rinzu_native_record_layout_count;
RINZU_PROGRAM_API int rinzu_native_receive_record_prepared(rinzu_managed_data_t *,
                                                           const rinzu_managed_root_set_t *,
                                                           uint64_t, const unsigned char *,
                                                           uint64_t *);
#define ASYNC_EVENT(raw, kind, future) rinzu_aot_async_event(raw, kind, future)
#define printf rinzu_jit_printf
#endif

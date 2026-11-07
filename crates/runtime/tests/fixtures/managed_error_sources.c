#include <assert.h>
#include "rinzu/managed_errors.h"

static const size_t location_fields[] = {0, 1, 1}, source_fields[] = {4};
static const size_t error_fields[] = {0, 0, 3, 5}, cause_fields[] = {2};
static const rinzu_key_variant_t source_variants[] = {{0, NULL}, {1, source_fields}};
static const rinzu_key_variant_t cause_variants[] = {{0, NULL}, {1, cause_fields}};
static const rinzu_value_type_t types[] = {
    {.kind = RINZU_VALUE_STRING},
    {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64},
    {.kind = RINZU_VALUE_ERROR},
    {.kind = RINZU_VALUE_ENUM,
     .identity = "Option<SourceLocation>",
     .variant_count = 2,
     .variants = source_variants},
    {.kind = RINZU_VALUE_RECORD,
     .identity = "SourceLocation",
     .field_count = 3,
     .fields = location_fields},
    {.kind = RINZU_VALUE_ENUM,
     .identity = "Option<Error>",
     .variant_count = 2,
     .variants = cause_variants},
    {.kind = RINZU_VALUE_ERROR_RECORD, .identity = "A", .field_count = 4, .fields = error_fields}};
static const rinzu_source_entry_t sources[] = {{1, 17, 2, 9, "absent.rinzu", 12},
                                               {2, 33, 3, 8, "other.rinzu", 11}};

static void inspect(rinzu_managed_data_t *data, uint64_t error) {
  uint64_t option = rinzu_managed_error_metadata(data, 3, error, 2);
  rinzu_managed_value_t *some = rinzu_managed_find(data, option);
  assert(some->variant == 1);
  rinzu_managed_value_t *at = rinzu_managed_find(data, some->slots[0].bits);
  assert(at->slots[1].bits == 2 && at->slots[2].bits == 9);
  rinzu_managed_value_t *file = rinzu_managed_find(data, at->slots[0].bits);
  assert(file->length == 12 && !memcmp(file->bytes, "absent.rinzu", 12));
}

int main(void) {
  rinzu_value_schema_t schema = {0};
  assert(!rinzu_value_schema_init(&schema, types, sizeof types / sizeof *types));
  rinzu_managed_data_t data = {0};
  assert(!rinzu_managed_configure_values(&data, &schema));
  data.source_entries = sources;
  data.source_entry_count = sizeof sources / sizeof *sources;
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  uint64_t absent = 0, cause = 0, error = 0;
  assert(!rinzu_managed_composite(&data, 3, 0, NULL, 0, &absent));
  assert(!rinzu_managed_composite(&data, 5, 0, NULL, 0, &cause));
  uint64_t name = rinzu_managed_string(&data, (const unsigned char *)"A", 1);
  uint64_t message = rinzu_managed_string(&data, (const unsigned char *)"message", 7);
  rinzu_managed_slot_t fields[] = {{name, 1}, {message, 1}, {absent, 1}, {cause, 1}};
  assert(!rinzu_managed_composite(&data, 6, -1, fields, 4, &error));
  assert(rinzu_managed_error_metadata(&data, 3, error, 2) == absent);
  rinzu_source_position_t first = {.present = 1, .file = 1, .start_byte = 17, .end_byte = 29};
  rinzu_source_position_t later = {.present = 1, .file = 2, .start_byte = 33, .end_byte = 40};
  assert(rinzu_managed_first_error_source(&data, error, first).file == 1);
  assert(rinzu_managed_first_error_source(&data, error, later).start_byte == 17);
  inspect(&data, error);
  assert(rinzu_managed_find(&data, absent)->variant == 0);
  rinzu_error_buffer_t bytes = rinzu_error_single(7, (const unsigned char *)"legacy", 6);
  uint64_t legacy = rinzu_managed_rich_error(&data, bytes.bytes, bytes.length);
  free(bytes.bytes);
  assert(!rinzu_managed_error_source(&data, legacy).present);
  assert(rinzu_managed_first_error_source(&data, legacy, first).end_byte == 29);
  assert(rinzu_managed_first_error_source(&data, legacy, later).file == 1);
  inspect(&data, legacy);
  rinzu_failure_t failure = {.primary = 5, .primary_source = first};
  uint64_t operation = rinzu_managed_error(&data, &failure);
  rinzu_error_trace_frame_t frame = {"operation", 9, first};
  rinzu_error_trace_t *trace = rinzu_error_trace_capture(&frame, 1);
  assert(rinzu_managed_first_error_trace(&data, legacy, trace) == trace);
  assert(rinzu_managed_first_error_trace(&data, operation, trace) == trace);
  inspect(&data, operation);
  uint64_t unknown = rinzu_managed_source_option(
      &data, 3,
      (rinzu_source_position_t){.present = 1, .file = 1, .start_byte = 18, .end_byte = 20});
  assert(rinzu_managed_find(&data, unknown)->variant == 0);
  uint64_t live[] = {error, legacy, operation};
  rinzu_managed_publish(&roots, live, 3);
  for (int i = 0; i < 3; ++i) {
    rinzu_managed_collect(&data, &roots);
    inspect(&data, error);
    inspect(&data, legacy);
    inspect(&data, operation);
    assert(rinzu_managed_error_trace(&data, legacy) == trace);
    assert(rinzu_managed_error_trace(&data, operation) == trace);
    assert(rinzu_managed_error_source(&data, error).end_byte == 29);
  }
  rinzu_managed_publish(&roots, NULL, 0);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !data.error_references);
  assert(atomic_load(&trace->references) == 1);
  rinzu_error_trace_release(trace);
  rinzu_managed_leave(&roots);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_value_schema_dispose(&schema);
  return 0;
}

#ifndef RINZU_JSON_H
#define RINZU_JSON_H

/* Private JSON core shared by the Rust and generated C runtimes. All spans
   are offsets. The caller may move input storage between consecutive steps. */
#include <stddef.h>
#include <stdint.h>

enum { RZ_JSON_NULL, RZ_JSON_BOOLEAN, RZ_JSON_NUMBER, RZ_JSON_TEXT, RZ_JSON_ARRAY, RZ_JSON_OBJECT };
enum {
  RZ_JSON_OK,
  RZ_JSON_DECODE,
  RZ_JSON_INVALID_LIMIT,
  RZ_JSON_LIMIT,
  RZ_JSON_TYPE,
  RZ_JSON_NUMBER_ERROR,
  RZ_JSON_SCHEMA
};
enum {
  RZ_JSON_END,
  RZ_JSON_TOKEN,
  RZ_JSON_TRAILING,
  RZ_JSON_UTF8,
  RZ_JSON_CONTROL,
  RZ_JSON_ESCAPE,
  RZ_JSON_SURROGATE,
  RZ_JSON_BAD_NUMBER,
  RZ_JSON_DUPLICATE
};
enum { RZ_JSON_INPUT, RZ_JSON_DEPTH, RZ_JSON_NODES, RZ_JSON_OUTPUT };
enum { RZ_JSON_SYNTAX, RZ_JSON_INTEGER, RZ_JSON_RANGE, RZ_JSON_NONFINITE };
enum {
  RZ_JSON_WRONG_KIND,
  RZ_JSON_EXPECT_INT,
  RZ_JSON_INT_RANGE,
  RZ_JSON_FLOAT_RANGE,
  RZ_JSON_MISSING,
  RZ_JSON_UNKNOWN
};
enum {
  RZ_JSON_UNSUPPORTED,
  RZ_JSON_BOOL_TY,
  RZ_JSON_INT_TY,
  RZ_JSON_FLOAT_TY,
  RZ_JSON_STRING_TY,
  RZ_JSON_LIST_TY,
  RZ_JSON_MAP_TY,
  RZ_JSON_RECORD_TY,
  RZ_JSON_OPTION_TY
};
enum { RZ_JSON_SCALAR, RZ_JSON_OPEN, RZ_JSON_KEY, RZ_JSON_CLOSE };
enum { RZ_JSON_DONE, RZ_JSON_MORE, RZ_JSON_FAILED };
#define RZ_JSON_NONE UINT32_MAX
#define RZ_JSON_DEFAULT_BYTES (128 * 1024 * 1024)
#define RZ_JSON_DEFAULT_DEPTH 1024
#define RZ_JSON_DEFAULT_NODES (16 * 1024 * 1024)
#define RZ_JSON_BATCH_SIZE 256

typedef struct {
  int64_t bytes, depth, nodes;
} RinzuJsonLimits;
typedef struct {
  const unsigned char *data;
  size_t length;
} RinzuJsonSlice;
typedef struct {
  unsigned char *data;
  size_t length, capacity, moved;
} RinzuJsonBuffer;
typedef struct {
  uint32_t code, reason, offset, first, expected, actual;
  int64_t limit, supplied, minimum, maximum;
  RinzuJsonSlice path, key;
} RinzuJsonError;
typedef struct {
  uint32_t kind, child, first, count;
} RinzuJsonType;
typedef struct {
  uint32_t name, length, type;
} RinzuJsonField;
typedef struct {
  const RinzuJsonType *types;
  const RinzuJsonField *fields;
  const unsigned char *names;
  uint32_t root;
} RinzuJsonSchema;
typedef struct {
  uint64_t bits;
  uint32_t event, kind, type, field, start, length, scalars, decoded;
} RinzuJsonEvent;
typedef struct {
  RinzuJsonEvent events[RZ_JSON_BATCH_SIZE];
  RinzuJsonSlice decoded;
  size_t count;
} RinzuJsonBatch;

typedef struct RinzuJsonParser RinzuJsonParser;
typedef struct {
  const unsigned char *index;
  RinzuJsonSlice source;
  uint32_t nodes, containers, texts, edges, hashes, decoded;
} RinzuJsonDocument;
typedef struct {
  uint32_t kind, start, end, depth, nodes, length;
} RinzuJsonNode;

int rinzu_json_limits(RinzuJsonLimits limits, unsigned bytes_kind, RinzuJsonError *error);
RinzuJsonParser *rinzu_json_parser_new(size_t length, RinzuJsonLimits limits,
                                       const RinzuJsonSchema *schema);
void rinzu_json_parser_free(RinzuJsonParser *parser);
int rinzu_json_step(RinzuJsonParser *parser, const unsigned char *bytes, RinzuJsonBatch *batch);
const RinzuJsonError *rinzu_json_error(const RinzuJsonParser *parser);
RinzuJsonBuffer rinzu_json_index(RinzuJsonParser *parser);
size_t rinzu_json_scratch_bytes(const RinzuJsonParser *parser);
void rinzu_json_document(RinzuJsonDocument *doc, RinzuJsonSlice source, RinzuJsonSlice index);
RinzuJsonNode rinzu_json_node(const RinzuJsonDocument *doc, uint32_t node);
uint32_t rinzu_json_child(const RinzuJsonDocument *doc, uint32_t node, uint32_t index);
uint32_t rinzu_json_find(const RinzuJsonDocument *doc, uint32_t node, RinzuJsonSlice key);
RinzuJsonSlice rinzu_json_text(const RinzuJsonDocument *doc, uint32_t node);
RinzuJsonSlice rinzu_json_event_text(const RinzuJsonEvent *event, RinzuJsonSlice source,
                                     RinzuJsonSlice decoded);
int rinzu_json_number_valid(RinzuJsonSlice token);
int rinzu_json_int(RinzuJsonSlice token, uint64_t *bits);
int rinzu_json_float(RinzuJsonSlice token, uint64_t *bits);
size_t rinzu_json_format_int(uint64_t bits, unsigned char out[32]);
size_t rinzu_json_format_float(uint64_t bits, unsigned char out[32]);
void rinzu_json_buffer_free(RinzuJsonBuffer *buffer);
int rinzu_json_append(RinzuJsonBuffer *buffer, size_t limit, RinzuJsonSlice text);
int rinzu_json_escape(RinzuJsonBuffer *buffer, size_t limit, RinzuJsonSlice text);
#endif

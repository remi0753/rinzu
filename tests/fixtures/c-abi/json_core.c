/* The same core runs with an inaccessible page immediately after the input.
   This translation unit also serves as a libFuzzer target. */
#include "rinzu/json.c"

#include <assert.h>
#include <stdio.h>
#include <sys/mman.h>

static const RinzuJsonLimits defaults = {RZ_JSON_DEFAULT_BYTES, RZ_JSON_DEFAULT_DEPTH,
                                         RZ_JSON_DEFAULT_NODES};

static int parse(RinzuJsonParser *p, const unsigned char *data, RinzuJsonBatch *batch) {
  int status;
  do {
    status = rinzu_json_step(p, data, batch);
  } while (status == RZ_JSON_MORE);
  return status;
}

int LLVMFuzzerTestOneInput(const unsigned char *data, size_t length) {
  RinzuJsonParser *p = rinzu_json_parser_new(length, defaults, NULL);
  if (parse(p, data, NULL) == RZ_JSON_DONE) {
    RinzuJsonBuffer tape = rinzu_json_index(p);
    RinzuJsonDocument doc;
    rinzu_json_document(&doc, (RinzuJsonSlice){data, length},
                        (RinzuJsonSlice){tape.data, tape.length});
    RinzuJsonNode root = rinzu_json_node(&doc, 0);
    for (uint32_t id = 0; id < root.nodes; ++id) {
      RinzuJsonNode node = rinzu_json_node(&doc, id);
      assert(node.start < node.end && node.end <= length);
      if (node.kind == RZ_JSON_ARRAY || node.kind == RZ_JSON_OBJECT) {
        for (uint32_t i = 0; i < node.length; ++i) {
          uint32_t child = rinzu_json_child(&doc, id, i);
          assert(child > id && child < id + node.nodes);
          if (node.kind == RZ_JSON_OBJECT) {
            assert(rinzu_json_find(&doc, id, rinzu_json_text(&doc, child)) == child + 1);
          }
        }
      }
      if (node.kind == RZ_JSON_NUMBER) {
        RinzuJsonSlice number = rinzu_json_text(&doc, id);
        uint64_t bits;
        assert(rinzu_json_number_valid(number));
        (void)rinzu_json_int(number, &bits);
        (void)rinzu_json_float(number, &bits);
      }
    }
    rinzu_json_buffer_free(&tape);
  }
  rinzu_json_parser_free(p);

  const RinzuJsonType types[] = {{.kind = RZ_JSON_MAP_TY, .child = 1},
                                 {.kind = RZ_JSON_LIST_TY, .child = 2},
                                 {.kind = RZ_JSON_OPTION_TY, .child = 3},
                                 {.kind = RZ_JSON_INT_TY}};
  RinzuJsonSchema schema = {.types = types, .root = 0};
  RinzuJsonBatch batch;
  p = rinzu_json_parser_new(length, defaults, &schema);
  (void)parse(p, data, &batch);
  rinzu_json_parser_free(p);
  return 0;
}

#ifndef RINZU_JSON_FUZZ
static void page_end(void) {
  size_t page = (size_t)sysconf(_SC_PAGESIZE);
  assert(page > 256);
  unsigned char *memory =
      mmap(NULL, page * 2, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
  assert(memory != MAP_FAILED);
  assert(!mprotect(memory + page, page, PROT_NONE));
  const char *documents[] = {"\"日本語😀\\u0000\n\"",
                             "\"unterminated",
                             "\"日本語😀\\u0000\"",
                             "\"\\uD800",
                             "\"\\uD800\\uDC00\"",
                             "[1e+0000]",
                             "1e",
                             "1.7976931348623157e308",
                             "\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"",
                             "              \t \ttrue                "};
  for (size_t i = 0; i < sizeof documents / sizeof *documents; ++i) {
    size_t length = strlen(documents[i]);
    unsigned char *start = memory + page - length;
    memcpy(start, documents[i], length);
    LLVMFuzzerTestOneInput(start, length);
  }
  assert(!munmap(memory, page * 2));
}

static void wide_object(void) {
  RinzuJsonBuffer source = {0};
  append(&source, "{", 1);
  for (unsigned i = 0; i < 1300; ++i) {
    char entry[32];
    int n = snprintf(entry, sizeof entry, "%s\"field%u\":%u", i ? "," : "", i, i);
    assert(n > 0 && n < (int)sizeof entry);
    append(&source, entry, (size_t)n);
  }
  append(&source, "}", 1);
  LLVMFuzzerTestOneInput(source.data, source.length);

  --source.length;
  uint32_t repeated = (uint32_t)source.length + 1;
  const char *duplicates = ",\"\\u0066ield20\":-1,\"field0\":5}";
  append(&source, duplicates, strlen(duplicates));
  RinzuJsonParser *p = rinzu_json_parser_new(source.length, defaults, NULL);
  assert(parse(p, source.data, NULL) == RZ_JSON_FAILED);
  const RinzuJsonError *error = rinzu_json_error(p);
  assert(error->code == RZ_JSON_DECODE && error->reason == RZ_JSON_DUPLICATE);
  assert(error->offset == repeated);
  assert(!memcmp(source.data + error->first, "\"field20\"", 9));
  rinzu_json_parser_free(p);
  rinzu_json_buffer_free(&source);
}

static void long_keys(unsigned count) {
  unsigned char key[SCAN_BYTES + 128];
  const char digits[] = "0123456789abcdef";
  memset(key, 'a', sizeof key);
  RinzuJsonBuffer source = {0};
  uint32_t offset = 0;
  append(&source, "{", 1);
  for (unsigned i = 0; i <= count; ++i) {
    offset = (uint32_t)source.length + (i != 0);
    append(&source, i ? ",\"" : "\"", i ? 2 : 1);
    unsigned suffix = i % count;
    key[sizeof key - 2] = digits[suffix >> 4];
    key[sizeof key - 1] = digits[suffix & 15];
    append(&source, key, sizeof key);
    append(&source, "\":0", 3);
  }
  append(&source, "}", 1);
  RinzuJsonParser *p = rinzu_json_parser_new(source.length, defaults, NULL);
  assert(parse(p, source.data, NULL) == RZ_JSON_FAILED);
  const RinzuJsonError *error = rinzu_json_error(p);
  assert(error->code == RZ_JSON_DECODE && error->reason == RZ_JSON_DUPLICATE);
  assert(error->offset == offset && error->first == 1);
  rinzu_json_parser_free(p);
  rinzu_json_buffer_free(&source);
}

static void depth_and_interruption(void) {
  unsigned char deep[8194];
  memset(deep, '[', 4097);
  memset(deep + 4097, ']', 4097);
  RinzuJsonLimits max = defaults;
  max.depth = 4096;
  RinzuJsonParser *p = rinzu_json_parser_new(sizeof deep, max, NULL);
  assert(parse(p, deep, NULL) == RZ_JSON_FAILED);
  const RinzuJsonError *error = rinzu_json_error(p);
  assert(error->code == RZ_JSON_LIMIT && error->reason == RZ_JSON_DEPTH && error->offset == 4096);
  rinzu_json_parser_free(p);
  p = rinzu_json_parser_new(sizeof deep - 2, max, NULL);
  assert(parse(p, deep + 1, NULL) == RZ_JSON_DONE);
  rinzu_json_parser_free(p);

  unsigned char *partial = malloc(2 * SCAN_BYTES);
  assert(partial);
  memset(partial, 'a', 2 * SCAN_BYTES);
  partial[0] = '"';
  partial[1] = '\\';
  partial[2] = 't';
  p = rinzu_json_parser_new(2 * SCAN_BYTES, defaults, NULL);
  assert(rinzu_json_step(p, partial, NULL) == RZ_JSON_MORE);
  rinzu_json_parser_free(p);
  free(partial);
}

int main(void) {
  atomic_store(&rinzu_resource_enabled, 1);
  page_end();
  wide_object();
  long_keys(2);
  long_keys(34);
  depth_and_interruption();
  assert(rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live == 0);
}
#endif

#include <assert.h>
int main(int argc, char **argv) {
  assert(argc == 2);
  const char *mode = argv[1];
  if (!strcmp(mode, "program")) {
    int result = unused_source_main(1, argv);
    if (!result)
      puts("BYTES_INITIALIZATION=program");
    return result;
  }
  assert(!strcmp(mode, "padding"));
  RinzuContext context = {0};
  context.shared_data = &context.data;
  rinzu_managed_data_t *data = rinzu_context_data(&context);
  rinzu_managed_attach_roots(data, &context.domain_roots, &context.roots);
  rinzu_managed_enter(&context.roots);
  const size_t lengths[] = {0, 1, 15, 16, 17, 255, 256, 511, 512, 513, 4096, 65536};
  for (size_t k = 0; k < sizeof(lengths) / sizeof(*lengths); ++k) {
    size_t to = lengths[k];
    const size_t prefixes[] = {0, to / 2, to};
    for (size_t i = 0; i < 3; ++i) {
      size_t from = prefixes[i];
      uint8_t original[65536];
      for (size_t j = 0; j < from; ++j)
        original[j] = (uint8_t)(j * 139 + 43);
      uint64_t input;
      assert(!rinzu_managed_bytes(data, from, original, from, &input));
      uint64_t output =
          (uint64_t)rinzu_baseline_bytes_pad(&context, (int64_t)input, (int64_t)from, (int64_t)to);
      uint64_t roots[] = {input, output};
      rinzu_managed_publish(&context.roots, roots, 2);
      for (unsigned gc = 0; gc < 2; ++gc) {
        rinzu_managed_value_t *value = rinzu_managed_find(data, output);
        assert(value->length == to && !value->bytes[to]);
        for (size_t j = 0; j < to; ++j)
          assert(value->bytes[j] == (j < from ? original[j] : 0));
        assert(rinzu_managed_find(data, input)->length == from);
        assert(!memcmp(rinzu_managed_find(data, input)->bytes, original, from));
        rinzu_managed_collect(data, &context.roots);
      }
    }
  }
  rinzu_managed_leave(&context.roots);
  rinzu_managed_collect(data, &context.roots);
  assert(!data->objects);
  rinzu_managed_detach_roots(data, &context.domain_roots);
  rinzu_managed_dispose(&context.roots);
  rinzu_managed_data_dispose(data);
  puts("BYTES_INITIALIZATION=padding");
}

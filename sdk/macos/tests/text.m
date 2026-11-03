#include "../native/src/macos.m"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>

static void *layout(void *font, const char *value, double width, int64_t align) {
  int64_t id = rinzu_macos_text_layout(font, (const uint8_t *)value, strlen(value), width, align);
  if (id < 0) {
    const rinzu_native_byte_array_t *diagnostic = rinzu_macos_diagnostic();
    fwrite(diagnostic->data, 1, diagnostic->length, stderr);
  }
  assert(id > 0);
  return rinzu_macos_text(id);
}
static struct MacTextMetrics metrics(void *text) {
  struct MacTextMetrics result;
  assert(rinzu_macos_text_metrics(text, &result) == 0);
  assert(isfinite(result.width) && result.height > 0 && result.lines > 0);
  assert(result.baseline > 0 && result.ascent > 0 && result.descent >= 0);
  assert(result.rasterWidth >= result.width && result.rasterHeight >= result.height);
  return result;
}
static void *foreign(void *font) {
  @autoreleasepool {
    assert(rinzu_macos_text_layout(font, (const uint8_t *)"x", 1, 0, 0) == RZ_OWNER);
    assert(rinzu_macos_font_destroy(font) == RZ_OWNER);
  }
  return NULL;
}
static void measure(void *session, void *font) {
  void *empty = layout(font, "", 0, 0);
  struct MacTextMetrics e = metrics(empty);
  assert(e.width == 0 && e.lines == 1);
  void *one = layout(font, "Rinzu 日本語 😀", 0, 0);
  struct MacTextMetrics a = metrics(one);
  assert(a.width > 60 && a.lines == 1);
  void *multi = layout(font, "First\n日本語\n", 0, 0);
  struct MacTextMetrics b = metrics(multi);
  assert(b.lines == 3 && b.height > a.height * 2);
  void *wrapped =
      layout(font, "A bounded layout wraps Japanese 日本語 and emoji 😀 safely.", 90, 1);
  struct MacTextMetrics c = metrics(wrapped);
  assert(c.width == 90 && c.lines >= 3);
  assert(rinzu_macos_font_destroy(font) == RZ_BUSY);
  assert(rinzu_macos_text_metrics(session, &c) == RZ_INVALID);
  assert(rinzu_macos_text_layout(font, (const uint8_t *)"x", 1, NAN, 0) == RZ_ARGUMENT);
  assert(rinzu_macos_text_layout(font, (const uint8_t *)"x", 1, 0, INT64_C(4294967296)) ==
         RZ_ARGUMENT);
  assert(rinzu_macos_text_layout(font, (const uint8_t *)"\xff", 1, 0, 0) == RZ_ARGUMENT);
  assert(rinzu_macos_text_layout(font, NULL, 65537, 0, 0) == RZ_ARGUMENT);
  assert(rinzu_macos_font_create(session, NULL, 0, INFINITY, 0) == RZ_ARGUMENT);
  assert(rinzu_macos_font_create(session, (const uint8_t *)"No-Such-Rinzu-Font-123", 22, 14, 0) ==
         RZ_ARGUMENT);
  pthread_t worker;
  assert(pthread_create(&worker, NULL, foreign, font) == 0);
  assert(pthread_join(worker, NULL) == 0);
  assert(rinzu_macos_text_destroy(empty) == 0);
  assert(rinzu_macos_text_destroy(one) == 0);
  assert(rinzu_macos_text_destroy(multi) == 0);
  assert(rinzu_macos_text_destroy(wrapped) == 0);
  assert(rinzu_macos_text_metrics(one, &c) == RZ_INVALID);
  for (unsigned i = 0; i < 1000; ++i) {
    __weak RZMacText *released;
    @autoreleasepool {
      void *value = layout(font, "repeat 日本", 0, 0);
      released = (RZMacText *)lookup(value, RZ_TEXT);
      assert(rinzu_macos_text_destroy(value) == 0);
    }
    assert(!released);
  }
  puts("PASS:text:measure:unicode:empty:multiline:wrap:bounds:owner:1000-retire");
}
static void raster(void *device, void *font) {
  void *text = layout(font, "Rinzu 日本語 😀", 0, 0);
  struct MacTextMetrics m = metrics(text);
  int64_t id = rinzu_macos_text_texture(device, text, 2);
  assert(id > 0);
  void *texture = rinzu_macos_texture(id);
  struct MacTextureInfo info;
  assert(rinzu_macos_texture_info(texture, &info) == 0);
  assert(info.width == ceil(m.rasterWidth * 2) && info.height == ceil(m.rasterHeight * 2));
  struct MacScissor all = {0, 0, info.width, info.height};
  assert(rinzu_macos_texture_read(texture, &all) == 0);
  const rinzu_native_byte_array_t *bytes = rinzu_macos_texture_bytes(texture);
  size_t visible = 0, colored = 0;
  for (size_t i = 0; i < bytes->length; i += 4) {
    visible += bytes->data[i + 3] > 0;
    colored += bytes->data[i] != bytes->data[i + 1] || bytes->data[i + 1] != bytes->data[i + 2];
    assert(bytes->data[i] <= bytes->data[i + 3] && bytes->data[i + 1] <= bytes->data[i + 3] &&
           bytes->data[i + 2] <= bytes->data[i + 3]);
  }
  assert(visible > 100 && colored > 10);
  assert(rinzu_macos_text_texture(device, text, 0) == RZ_ARGUMENT);
  assert(rinzu_macos_text_destroy(text) == 0);
  // The GPU texture is an independent snapshot of the retired layout.
  assert(rinzu_macos_texture_read(texture, &all) == 0);
  assert(rinzu_macos_texture_destroy(texture) == 0);
  puts("PASS:text:raster:retina:premultiplied:color-emoji:owned-snapshot");
}
int main(int argc, const char **argv) {
  @autoreleasepool {
    assert(argc == 2);
    void *session = rinzu_macos_session(rinzu_macos_open());
    assert(session);
    void *font = rinzu_macos_font(rinzu_macos_font_create(session, NULL, 0, 18, 0));
    assert(font);
    if (!strcmp(argv[1], "measure"))
      measure(session, font);
    else if (!strcmp(argv[1], "raster")) {
      if (!rinzu_macos_metal_available())
        return 77;
      void *device = rinzu_macos_device(rinzu_macos_device_create(session));
      raster(device, font);
    } else
      assert(!"unknown mode");
    assert(rinzu_macos_font_destroy(font) == 0);
    assert(rinzu_macos_close(session) == 0);
    struct MacStatistics s;
    assert(rinzu_macos_statistics(&s) == 0 && !s.retainedControls && !s.textBytes &&
           !s.textureBytes);
    puts("PASS:text:closed");
  }
  return 0;
}

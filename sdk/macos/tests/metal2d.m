// Real Metal pixel tests. Private registry access is restricted to this fixture.
#include "../native/src/macos.m"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
#include <unistd.h>

static void clean(void) {
  struct MacStatistics s;
  assert(rinzu_macos_statistics(&s) == 0);
  assert(!s.sessions && !s.retainedControls && !s.textures && !s.samplers && !s.textureBytes &&
         !s.snapshotBytes && !s.bufferBytes && !s.commands);
}
static void wait_gpu(void *command) {
  int64_t token = rinzu_macos_wait_token();
  assert(rinzu_macos_command_wait_begin(command, token) == 0);
  rinzu_native_completion_t *completion = rinzu_macos_command_completion(command, token);
  assert(completion);
  for (unsigned i = 0; !rinzu_native_completion_is_done(completion); ++i) {
    assert(i < 20000);
    usleep(1000);
  }
  assert(rinzu_macos_command_status(command) == 0);
  assert(rinzu_macos_command_finish(command, token) == 0);
}
static void *texture(void *device, int64_t w, int64_t h, int format, int usage) {
  int64_t id = rinzu_macos_texture_create(device, w, h, format, usage);
  assert(id > 0);
  return rinzu_macos_texture(id);
}
static void pixel(void *raw, int64_t x, int64_t y, unsigned r, unsigned g, unsigned b, unsigned a) {
  struct MacScissor region = {x, y, 1, 1};
  assert(rinzu_macos_texture_read(raw, &region) == 0);
  const rinzu_native_byte_array_t *bytes = rinzu_macos_texture_bytes(raw);
  assert(bytes && bytes->length == 4);
  unsigned expected[4] = {r, g, b, a};
  for (unsigned i = 0; i < 4; ++i) {
    if (abs((int)bytes->data[i] - (int)expected[i]) > 1) {
      fprintf(stderr, "pixel (%lld,%lld) channel %u: got %u expected %u\n", x, y, i, bytes->data[i],
              expected[i]);
      assert(!"GPU pixel mismatch");
    }
  }
}
static void *wrong_thread(void *raw) {
  @autoreleasepool {
    struct MacTextureInfo info;
    assert(rinzu_macos_texture_info(raw, &info) == RZ_OWNER);
    assert(rinzu_macos_texture_destroy(raw) == RZ_OWNER);
    assert(rinzu_macos_sampler_create(raw, 0, 0) == RZ_OWNER);
  }
  return NULL;
}
static void transfer(void *session, void *device) {
  assert(rinzu_macos_texture_create(device, 8193, 1, 1, 1) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_create(device, INT64_MAX, INT64_MAX, 1, 1) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_create(device, 1, 1, INT64_MAX, 1) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_create(device, 1, 1, INT64_C(4294967297), 1) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_create(device, 1, 1, 2, 4) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_create(device, 1, 1, 1, 0) == RZ_ARGUMENT);
  assert(rinzu_macos_sampler_create(device, INT64_MAX, 0) == RZ_ARGUMENT);
  void *raw = texture(device, 2, 2, 1, 1);
  struct MacTextureInfo info;
  assert(rinzu_macos_texture_info(raw, &info) == 0 && info.width == 2 && info.height == 2 &&
         info.format == 1 && info.usage == 1 && info.bytes == 16);
  assert(rinzu_macos_texture_info(session, &info) == RZ_INVALID);
  assert(rinzu_macos_texture_info(raw, NULL) == RZ_ARGUMENT);
  pthread_t thread;
  assert(pthread_create(&thread, NULL, wrong_thread, raw) == 0);
  assert(pthread_join(thread, NULL) == 0);
  const uint8_t padded[20] = {255, 0,  0, 255, 0,   255, 0,   255, 77,  77,
                              77,  77, 0, 0,   255, 255, 128, 128, 128, 128};
  struct MacScissor all = {0, 0, 2, 2}, invalid = {INT64_MAX, 0, 2, 1};
  assert(rinzu_macos_texture_write(raw, &all, 12, padded, sizeof(padded)) == 0);
  assert(rinzu_macos_texture_write(raw, &all, 12, padded, 19) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_write(raw, &all, INT64_MAX, padded, 20) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_write(raw, &all, 7, padded, 20) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_write(raw, &invalid, 8, padded, 20) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_write(raw, NULL, 8, padded, 20) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_read(raw, &invalid) == RZ_ARGUMENT);
  pixel(raw, 0, 0, 255, 0, 0, 255);
  pixel(raw, 1, 0, 0, 255, 0, 255);
  pixel(raw, 0, 1, 0, 0, 255, 255);
  pixel(raw, 1, 1, 128, 128, 128, 128);
  assert(rinzu_macos_device_destroy(device) == RZ_BUSY);
  assert(rinzu_macos_texture_destroy(raw) == 0);
  assert(rinzu_macos_texture_info(raw, &info) == RZ_INVALID);
  for (unsigned i = 0; i < 1000; ++i) {
    __weak RZMacTexture *released;
    @autoreleasepool {
      void *next = texture(device, 1, 1, 2, 3);
      assert(next != raw);
      released = (RZMacTexture *)lookup(next, RZ_TEXTURE);
      assert(rinzu_macos_texture_destroy(next) == 0);
    }
    assert(!released);
  }
  struct MacStatistics stats;
  assert(rinzu_macos_statistics(&stats) == 0 && !stats.textures && !stats.textureBytes &&
         !stats.snapshotBytes && stats.retainedControls == 2);
  puts("PASS:metal2d-transfer:stride:region:overflow:owner:stale:1000-retire");
}
static NSData *encode(const uint8_t *pixels, size_t width, size_t height, int orientation) {
  CGColorSpaceRef color = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  CGContextRef bitmap =
      CGBitmapContextCreate((void *)pixels, width, height, 8, width * 4, color,
                            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
  assert(bitmap);
  CGImageRef image = CGBitmapContextCreateImage(bitmap);
  NSMutableData *bytes = [NSMutableData new];
  CGImageDestinationRef output = CGImageDestinationCreateWithData(
      (__bridge CFMutableDataRef)bytes, orientation ? CFSTR("public.tiff") : CFSTR("public.png"), 1,
      NULL);
  assert(output && image);
  NSDictionary *properties = orientation
                                 ? @{(id)kCGImagePropertyOrientation : @(orientation)}
                                 : @{};
  CGImageDestinationAddImage(output, image, (__bridge CFDictionaryRef)properties);
  assert(CGImageDestinationFinalize(output));
  CFRelease(output);
  CGImageRelease(image);
  CGContextRelease(bitmap);
  CGColorSpaceRelease(color);
  return bytes;
}
static void images(void *device) {
  const uint8_t pixels[24] = {255, 0, 0, 255, 0, 255, 0, 255, 0,   0,   255, 255,
                              128, 0, 0, 128, 0, 0,   0, 0,   255, 255, 255, 255};
  NSData *png = encode(pixels, 2, 3, 0);
  int64_t id = rinzu_macos_texture_image(device, png.bytes, png.length);
  assert(id > 0);
  void *raw = rinzu_macos_texture(id);
  struct MacTextureInfo info;
  assert(rinzu_macos_texture_info(raw, &info) == 0 && info.width == 2 && info.height == 3);
  pixel(raw, 0, 0, 255, 0, 0, 255);
  pixel(raw, 1, 1, 128, 0, 0, 128);
  pixel(raw, 0, 2, 0, 0, 0, 0);
  assert(rinzu_macos_texture_destroy(raw) == 0);
  NSData *tiff = encode(pixels, 2, 3, 6);
  id = rinzu_macos_texture_image(device, tiff.bytes, tiff.length);
  assert(id > 0);
  raw = rinzu_macos_texture(id);
  assert(rinzu_macos_texture_info(raw, &info) == 0 && info.width == 3 && info.height == 2);
  pixel(raw, 2, 0, 255, 0, 0, 255);
  pixel(raw, 1, 1, 128, 0, 0, 128);
  assert(rinzu_macos_texture_destroy(raw) == 0);
  assert(rinzu_macos_texture_image(device, png.bytes, 8) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_image(device, (const uint8_t *)"invalid", 7) == RZ_ARGUMENT);
  assert(rinzu_macos_texture_image(device, NULL, 67108865) == RZ_ARGUMENT);
  NSData *large = [NSMutableData dataWithLength:8193 * 4];
  NSData *oversize = encode(large.bytes, 8193, 1, 0);
  assert(rinzu_macos_texture_image(device, oversize.bytes, oversize.length) == RZ_ARGUMENT);
  puts("PASS:metal2d-images:rgba:premultiply:orientation:truncated:bounds");
}
static const char *shader = "#include <metal_stdlib>\nusing namespace metal;\n"
                            "struct V { float2 p; float2 uv; float4 color; };\n"
                            "struct O { float4 p [[position]]; float2 uv; float4 color; };\n"
                            "vertex O vs(uint i [[vertex_id]], const device V *v [[buffer(0)]]) { "
                            "return {float4(v[i].p,0,1),v[i].uv,v[i].color}; }\n"
                            "fragment float4 fs(O v [[stage_in]], texture2d<float> t "
                            "[[texture(0)]], sampler s [[sampler(0)]]) { "
                            "return t.sample(s,v.uv) * float4(v.color.rgb*v.color.a,v.color.a); }\n"
                            "kernel void fill(texture2d<float,access::write> t [[texture(0)]], "
                            "uint2 p [[thread_position_in_grid]]) { "
                            "t.write(float4(0.25,0.5,0.75,1),p); }\n";
static void *library(void *device) {
  int64_t id = rinzu_macos_library_create(device, (const uint8_t *)shader, strlen(shader), 0);
  if (id < 0) {
    const rinzu_native_byte_array_t *message = rinzu_macos_diagnostic();
    fwrite(message->data, 1, message->length, stderr);
  }
  assert(id > 0);
  return rinzu_macos_library(id);
}
static void *pipeline(void *lib, int format, int blend) {
  int64_t id = rinzu_macos_render_pipeline_options(lib, (const uint8_t *)"vs", 2,
                                                   (const uint8_t *)"fs", 2, format, blend);
  assert(id > 0);
  return rinzu_macos_render(id);
}
static void rendering(void *session, void *device) {
  void *lib = library(device), *p = pipeline(lib, 1, 1), *wrong = pipeline(lib, 2, 0);
  void *cmd = rinzu_macos_command(rinzu_macos_command_create(device));
  void *other = rinzu_macos_command(rinzu_macos_command_create(device));
  void *target = texture(device, 8, 8, 1, 3), *source = texture(device, 1, 1, 1, 1);
  void *sampler = rinzu_macos_sampler(rinzu_macos_sampler_create(device, 0, 0));
  void *vertices = rinzu_macos_buffer(rinzu_macos_buffer_allocate(device, 192));
  struct MacQuad2D quad = {.width = 8,
                           .height = 8,
                           .uvWidth = 1,
                           .uvHeight = 1,
                           .red = 1,
                           .green = 1,
                           .blue = 1,
                           .alpha = 1,
                           .a = 0.25,
                           .d = -0.25,
                           .tx = -1,
                           .ty = 1};
  assert(rinzu_macos_buffer_quad2d(vertices, 0, &quad) == 0);
  assert(rinzu_macos_buffer_quad2d(vertices, INT64_MAX, &quad) == RZ_ARGUMENT);
  quad.tx = NAN;
  assert(rinzu_macos_buffer_quad2d(vertices, 0, &quad) == RZ_ARGUMENT);
  quad.tx = -1;
  struct MacVertex2D invalid_vertex = {.x = INFINITY};
  assert(rinzu_macos_buffer_vertex2d(vertices, 0, &invalid_vertex) == RZ_ARGUMENT);
  const uint8_t red[4] = {128, 0, 0, 128};
  struct MacScissor one = {0, 0, 1, 1}, all = {0, 0, 8, 8}, scissor = {3, 2, 2, 3};
  assert(rinzu_macos_texture_write(source, &one, 4, red, 4) == 0);
  struct MacColor blue = {0, 0, 1, 1};
  struct MacRect viewport = {2, 1, 4, 6};
  assert(rinzu_macos_command_render_texture(cmd, target, &blue) == 0);
  assert(rinzu_macos_command_pipeline(cmd, wrong) == RZ_ARGUMENT);
  assert(rinzu_macos_command_pipeline(cmd, p) == 0);
  assert(rinzu_macos_command_viewport(cmd, &viewport) == 0);
  assert(rinzu_macos_command_scissor(cmd, &scissor) == 0);
  assert(rinzu_macos_command_texture(cmd, target, 0, 2) == RZ_ARGUMENT);
  assert(rinzu_macos_command_texture(cmd, source, 16, 2) == RZ_ARGUMENT);
  assert(rinzu_macos_command_texture(cmd, source, 0, 2) == 0);
  assert(rinzu_macos_command_sampler(cmd, sampler, 0, 2) == 0);
  assert(rinzu_macos_command_buffer(cmd, vertices, 0, 0, 1) == 0);
  assert(rinzu_macos_command_draw(cmd, 0, 6) == 0);
  assert(rinzu_macos_texture_write(source, &one, 4, red, 4) == RZ_BUSY);
  assert(rinzu_macos_texture_read(source, &one) == RZ_BUSY);
  assert(rinzu_macos_texture_destroy(source) == RZ_BUSY);
  assert(rinzu_macos_sampler_destroy(sampler) == RZ_BUSY);
  assert(rinzu_macos_buffer_writable(vertices) == RZ_BUSY);
  assert(rinzu_macos_command_render_texture(other, source, &blue) == RZ_ARGUMENT);
  assert(rinzu_macos_command_render_texture(other, target, &blue) == RZ_BUSY);
  assert(rinzu_macos_close(session) == RZ_BUSY);
  assert(rinzu_macos_command_commit(cmd) == 0);
  assert(rinzu_macos_texture_read(target, &all) == RZ_BUSY);
  assert(rinzu_macos_command_abandon(cmd) == RZ_BUSY);
  wait_gpu(cmd);
  pixel(target, 3, 2, 128, 0, 127, 255);
  pixel(target, 4, 4, 128, 0, 127, 255);
  pixel(target, 2, 2, 0, 0, 255, 255);
  pixel(target, 5, 2, 0, 0, 255, 255);
  pixel(target, 3, 5, 0, 0, 255, 255);
  assert(rinzu_macos_command_render_texture(cmd, target, &blue) == 0);
  assert(rinzu_macos_command_texture(cmd, source, 0, 2) == 0);
  assert(rinzu_macos_command_sampler(cmd, sampler, 0, 2) == 0);
  assert(rinzu_macos_command_abandon(cmd) == 0);
  assert(rinzu_macos_texture_destroy(source) == 0);
  assert(rinzu_macos_sampler_destroy(sampler) == 0);
  puts("PASS:metal2d-render:typed-quad:blend:viewport:scissor:feedback:leases:abandon");
}
static void compute(void *device) {
  void *lib = library(device);
  void *p = rinzu_macos_compute(rinzu_macos_compute_pipeline(lib, (const uint8_t *)"fill", 4));
  assert(p);
  void *cmd = rinzu_macos_command(rinzu_macos_command_create(device));
  void *raw = texture(device, 2, 2, 1, 5);
  assert(rinzu_macos_command_compute(cmd, p) == 0);
  assert(rinzu_macos_command_texture(cmd, raw, 0, 0) == 0);
  assert(rinzu_macos_command_dispatch(cmd, 2, 2, 1, 2, 2, 1) == 0);
  assert(rinzu_macos_command_commit(cmd) == 0);
  wait_gpu(cmd);
  pixel(raw, 1, 1, 64, 128, 191, 255);
  puts("PASS:metal2d-compute:storage-texture:physical-readback");
}
static void sampling(void *device) {
  void *lib = library(device), *p = pipeline(lib, 1, 0);
  void *cmd = rinzu_macos_command(rinzu_macos_command_create(device));
  void *vertices = rinzu_macos_buffer(rinzu_macos_buffer_allocate(device, 192));
  void *source = texture(device, 2, 1, 1, 1), *target = texture(device, 1, 1, 1, 2);
  const uint8_t colors[8] = {255, 0, 0, 255, 0, 0, 255, 255};
  struct MacScissor region = {0, 0, 2, 1};
  assert(rinzu_macos_texture_write(source, &region, 8, colors, 8) == 0);
  struct MacColor clear = {0, 0, 1, 1};
  const struct {
    int filter, address;
    double u;
    unsigned r, b;
  } cases[] = {{0, 0, 0.5, 0, 255},
               {1, 0, 0.5, 128, 128},
               {0, 1, 1.1, 255, 0},
               {0, 2, 1.1, 0, 255},
               {0, 0, -0.25, 255, 0}};
  for (unsigned i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i) {
    struct MacQuad2D quad = {.x = -1,
                             .y = -1,
                             .width = 2,
                             .height = 2,
                             .u = cases[i].u,
                             .v = 0.5,
                             .red = 1,
                             .green = 1,
                             .blue = 1,
                             .alpha = 1,
                             .a = 1,
                             .d = 1};
    assert(rinzu_macos_buffer_quad2d(vertices, 0, &quad) == 0);
    void *sampler =
        rinzu_macos_sampler(rinzu_macos_sampler_create(device, cases[i].filter, cases[i].address));
    assert(rinzu_macos_command_render_texture(cmd, target, &clear) == 0);
    assert(rinzu_macos_command_pipeline(cmd, p) == 0);
    assert(rinzu_macos_command_texture(cmd, source, 0, 2) == 0);
    assert(rinzu_macos_command_sampler(cmd, sampler, 0, 2) == 0);
    assert(rinzu_macos_command_buffer(cmd, vertices, 0, 0, 1) == 0);
    assert(rinzu_macos_command_draw(cmd, 0, 6) == 0);
    assert(rinzu_macos_command_commit(cmd) == 0);
    wait_gpu(cmd);
    pixel(target, 0, 0, cases[i].r, 0, cases[i].b, 255);
    assert(rinzu_macos_sampler_destroy(sampler) == 0);
  }
  // Straight-alpha blend applies srcAlpha to RGB exactly once.
  const uint8_t straight[8] = {255, 0, 0, 128, 255, 0, 0, 128};
  assert(rinzu_macos_texture_write(source, &region, 8, straight, 8) == 0);
  void *straight_pipeline = pipeline(lib, 1, 2);
  void *sampler = rinzu_macos_sampler(rinzu_macos_sampler_create(device, 0, 0));
  assert(rinzu_macos_command_render_texture(cmd, target, &clear) == 0);
  assert(rinzu_macos_command_pipeline(cmd, straight_pipeline) == 0);
  struct MacRect bad_viewport = {0, 0, INFINITY, 1};
  struct MacScissor bad_scissor = {0, 0, INT64_MAX, 1};
  assert(rinzu_macos_command_viewport(cmd, &bad_viewport) == RZ_ARGUMENT);
  assert(rinzu_macos_command_scissor(cmd, &bad_scissor) == RZ_ARGUMENT);
  assert(rinzu_macos_command_texture(cmd, source, 0, 2) == 0);
  assert(rinzu_macos_command_sampler(cmd, sampler, 0, 2) == 0);
  assert(rinzu_macos_command_buffer(cmd, vertices, 0, 0, 1) == 0);
  assert(rinzu_macos_command_draw(cmd, 0, 6) == 0);
  assert(rinzu_macos_command_commit(cmd) == 0);
  wait_gpu(cmd);
  pixel(target, 0, 0, 128, 0, 127, 255);
  void *bgra = texture(device, 1, 1, 2, 2);
  assert(rinzu_macos_command_render_texture(cmd, bgra, &clear) == 0);
  assert(rinzu_macos_command_commit(cmd) == 0);
  wait_gpu(cmd);
  pixel(bgra, 0, 0, 255, 0, 0, 255);
  puts("PASS:metal2d-sampling:nearest:linear:clamp:repeat:mirror:straight:bgra");
}
int main(int argc, const char **argv) {
  @autoreleasepool {
    assert(argc == 2);
    void *session = rinzu_macos_session(rinzu_macos_open());
    assert(session);
    if (!rinzu_macos_metal_available()) {
      assert(rinzu_macos_close(session) == 0);
      return 77;
    }
    void *device = rinzu_macos_device(rinzu_macos_device_create(session));
    if (!strcmp(argv[1], "transfer"))
      transfer(session, device);
    else if (!strcmp(argv[1], "images"))
      images(device);
    else if (!strcmp(argv[1], "render"))
      rendering(session, device);
    else if (!strcmp(argv[1], "compute"))
      compute(device);
    else if (!strcmp(argv[1], "sampling"))
      sampling(device);
    else
      assert(!"unknown mode");
    assert(rinzu_macos_close(session) == 0);
    clean();
    puts("PASS:metal2d-closed");
  }
  return 0;
}

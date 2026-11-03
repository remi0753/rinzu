#include "../native/src/macos.m"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
static void *foreign(void *session) {
  assert(rinzu_macos_resource_root_size(session) == RZ_OWNER);
  assert(!rinzu_macos_resource_root_bytes(session));
  return NULL;
}
int main(int argc, const char **argv) {
  @autoreleasepool {
    (void)argv;
    void *session = rinzu_macos_session(rinzu_macos_open());
    assert(session);
    int64_t size = rinzu_macos_resource_root_size(session);
    const rinzu_native_byte_array_t *root = rinzu_macos_resource_root_bytes(session);
    assert(root && root->length == (size_t)size);
    NSString *owned = [[NSString alloc] initWithBytes:root->data
                                               length:root->length
                                             encoding:NSUTF8StringEncoding];
    if (argc == 2)
      assert(size > 0 && [owned hasSuffix:@"/Contents/Resources/app"]);
    else
      assert(size == 0);
    const char *valid = "assets/日本語.txt";
    assert(rinzu_macos_resource_relative((const uint8_t *)valid, strlen(valid)) == 0);
    for (const char **path = (const char *[]){"", "/tmp/x", "./x", "a/../b", "a//b", "a/", NULL};
         *path; ++path)
      assert(rinzu_macos_resource_relative((const uint8_t *)*path, strlen(*path)) == RZ_ARGUMENT);
    assert(rinzu_macos_resource_relative((const uint8_t *)"a\0b", 3) == RZ_ARGUMENT);
    assert(rinzu_macos_resource_relative((const uint8_t *)"\xff", 1) == RZ_ARGUMENT);
    assert(rinzu_macos_resource_relative(NULL, SIZE_MAX) == RZ_ARGUMENT);
    pthread_t thread;
    assert(pthread_create(&thread, NULL, foreign, session) == 0 && pthread_join(thread, NULL) == 0);
    assert(rinzu_macos_close(session) == 0);
    if (argc == 2)
      assert([owned hasSuffix:@"/Contents/Resources/app"]);
    assert(rinzu_macos_resource_root_size(session) == RZ_INVALID);
    assert(!rinzu_macos_resource_root_bytes(session));
    struct MacStatistics stats;
    assert(rinzu_macos_statistics(&stats) == 0 && stats.retainedControls == 0);
    puts("PASS:resources:relative:utf8:owner:closed");
  }
}

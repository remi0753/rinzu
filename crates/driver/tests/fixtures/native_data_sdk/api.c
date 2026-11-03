#include <assert.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static _Thread_local unsigned char text_data[16];
static _Thread_local unsigned char byte_data[4];
static _Thread_local unsigned char long_data[65537];
static pthread_once_t page_once = PTHREAD_ONCE_INIT;
static void *private_page;
static void entry_marker(void) {
  const char *path = getenv("RINZU_TEST_DATA_ENTRY");
  if (!path)
    return;
  int fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0600);
  if (fd < 0 || write(fd, "E", 1) != 1 || close(fd))
    _Exit(72);
}
static void make_page(void) {
  private_page = mmap(NULL, (size_t)getpagesize(), PROT_NONE, MAP_ANON | MAP_PRIVATE, -1, 0);
  assert(private_page != MAP_FAILED);
}
static const void *unreadable(void) {
  assert(!pthread_once(&page_once, make_page));
  return private_page;
}
const char *buffer_text(int32_t kind) {
  entry_marker();
  static const unsigned char empty[] = {0, 255, 0};
  static const unsigned char invalid[][5] = {
      {0xe3, 0x81, 0}, {0xe3, 0x41, 0x82, 0}, {0x80, 0},
      {0xc0, 0xaf, 0}, {0xed, 0xa0, 0x80, 0}, {0xf4, 0x90, 0x80, 0x80, 0}};
  if (kind == 0)
    return NULL;
  if (kind == 1)
    return (const char *)empty;
  if (kind == 2) {
    /* Trailing invalid bytes after the first NUL are outside the C string. */
    const unsigned char bytes[] = {0xe6, 0x97, 0xa5, 0xe6, 0x9c, 0xac, 0xe8, 0xaa, 0x9e, 0, 255, 0};
    memcpy(text_data, bytes, sizeof bytes);
    return (const char *)text_data;
  }
  if (kind >= 3 && kind <= 8)
    return (const char *)invalid[kind - 3];
  if (kind == 9)
    return "\xef\xbf\xbd";
  if (kind == 12)
    return unreadable();
  assert(kind == 13);
  memset(long_data, 'A', 65536);
  long_data[65536] = 0;
  return (const char *)long_data;
}
const unsigned char *buffer_bytes(int32_t kind) {
  entry_marker();
  if (kind == 0)
    return NULL;
  if (kind == 2)
    return unreadable();
  assert(kind == 1);
  const unsigned char bytes[4] = {0, 255, 7, 0};
  memcpy(byte_data, bytes, 4);
  return byte_data;
}
const unsigned char *buffer_empty_bytes(int32_t kind) {
  return kind ? unreadable() : NULL;
}
void buffer_mutate(void) {
  memset(text_data, 'X', sizeof text_data);
  memset(byte_data, 42, 4);
  memset(long_data, 'B', 65536);
}
uint64_t buffer_check_input(const char *text, uint64_t length, const unsigned char *bytes,
                            const unsigned char *empty) {
  const unsigned char expected[] = {0xe6, 0x97, 0xa5, 0xe6, 0x9c, 0xac, 0xe8, 0xaa,
                                    0x9e, 0,    0xf0, 0x9f, 0x99, 0x82, 0};
  const unsigned char expected_bytes[4] = {65, 255, 0, 0};
  assert(text && bytes && empty && length == 14);
  assert(!memcmp(text, expected, sizeof expected) && !memcmp(bytes, expected_bytes, 4));
  return length + 4;
}
const char *buffer_text_callback(uint64_t (*callback)(const unsigned char *)) {
  const char *result = buffer_text(2);
  const unsigned char input[4] = {0, 255, 7, 0};
  for (unsigned round = 0; round < 32; ++round)
    assert(callback(input) == 4);
  return result;
}
int32_t buffer_verify_long(const char *text) {
  for (size_t index = 0; index < 65536; ++index)
    if (text[index] != 'A')
      return 0;
  return text[65536] == 0;
}
void buffer_fixture_cleanup(void) {
  if (private_page) {
    assert(!munmap(private_page, (size_t)getpagesize()));
    private_page = NULL;
  }
}

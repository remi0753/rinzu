#include "../native/src/io.c"
#include <assert.h>
static void settled(void *file, int expected) {
  struct timespec pause = {0, 1000000};
  while (!rinzu_io_ready(file))
    nanosleep(&pause, NULL);
  assert(rinzu_io_status(file) == expected);
  rinzu_io_finish(file);
}
int main(void) {
  char directory[] = "/tmp/rinzu-temporary-XXXXXX";
  assert(mkdtemp(directory));
  char pattern[256], destination[256], missing[256];
  assert(snprintf(pattern, sizeof(pattern), "%s/document-XXXXXX", directory) > 0);
  assert(snprintf(destination, sizeof(destination), "%s/document.txt", directory) > 0);
  assert(snprintf(missing, sizeof(missing), "%s/missing/document.txt", directory) > 0);
  FILE *old = fopen(destination, "w");
  assert(old);
  assert(fputs("old", old) >= 0);
  assert(!fclose(old));
  struct File *first = rinzu_io_temporary(pattern);
  settled(first, 0);
  struct File *second = rinzu_io_temporary(pattern);
  settled(second, 0);
  char *first_path = strdup(first->temporary_path), *second_path = strdup(second->temporary_path);
  assert(first_path && second_path && strcmp(first_path, second_path) != 0);
  struct stat info;
  assert(!fstat(first->fd, &info) && (info.st_mode & 0777) == 0600);
  assert(fcntl(first->fd, F_GETFD) & FD_CLOEXEC);
  assert(!rinzu_io_write_all(first, (const uint8_t *)"new\0UTF8", 8));
  settled(first, 0);
  assert(!rinzu_io_flush(first));
  settled(first, 0);
  assert(!rinzu_io_install_temporary(first, destination));
  settled(first, 0);
  assert(!rinzu_io_close(first));
  settled(first, 0);
  assert(access(first_path, F_OK) && errno == ENOENT);
  FILE *readback = fopen(destination, "r");
  assert(readback);
  char bytes[8];
  assert(fread(bytes, 1, 8, readback) == 8 && !memcmp(bytes, "new\0UTF8", 8));
  assert(!fclose(readback));
  assert(!rinzu_io_install_temporary(second, missing));
  settled(second, ENOENT);
  assert(!rinzu_io_close(second));
  settled(second, 0);
  assert(access(second_path, F_OK) && errno == ENOENT);
  // Native acquisition returned before Source receipt: Owner recovery still removes the file.
  struct File *unreceived = rinzu_io_temporary(pattern);
  while (!rinzu_io_ready(unreceived)) {
    struct timespec pause = {0, 1000000};
    nanosleep(&pause, NULL);
  }
  char *unreceived_path = strdup(unreceived->temporary_path);
  assert(unreceived_path);
  assert(rinzu_io_owner_file() == unreceived);
  settled(unreceived, 0);
  assert(!rinzu_io_close(unreceived));
  settled(unreceived, 0);
  assert(access(unreceived_path, F_OK) && errno == ENOENT);
  assert(!rinzu_io_owner_has_file() && !rinzu_io_live_files() && !rinzu_io_live_buffers() &&
         !rinzu_io_live_operations());
  free(first_path);
  free(second_path);
  free(unreceived_path);
  assert(!unlink(destination));
  assert(!rmdir(directory));
  puts("PASS:io-temporary:unique:atomic:failed-install:unreceived-cleanup");
}

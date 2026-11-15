#include "rinzu/elf_image.h"

int rinzu_linux_loaded_image(const char *path, const char *build_id, const void *address,
                             void *handle) {
  return rinzu_elf_loaded(path, build_id, address, handle);
}

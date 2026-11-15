#ifndef RINZU_ELF_IMAGE_H
#define RINZU_ELF_IMAGE_H

#include <dlfcn.h>
#include <elf.h>
#include <errno.h>
#include <link.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

typedef struct {
  struct stat file;
  const char *build_id;
  const struct link_map *map;
  uintptr_t address;
  int found;
} rinzu_elf_query_t;

static inline int rinzu_elf_range(const struct dl_phdr_info *image, uintptr_t address, size_t size,
                                  unsigned required) {
  for (size_t i = 0; i < image->dlpi_phnum; ++i) {
    const ElfW(Phdr) *segment = &image->dlpi_phdr[i];
    if (segment->p_type != PT_LOAD || (segment->p_flags & required) != required ||
        segment->p_vaddr > UINTPTR_MAX - image->dlpi_addr)
      continue;
    uintptr_t base = image->dlpi_addr + segment->p_vaddr;
    if (address >= base && size <= segment->p_memsz && address - base <= segment->p_memsz - size)
      return 1;
  }
  return 0;
}

static inline int rinzu_elf_build_id(const struct dl_phdr_info *image, const char *expected) {
  static const char hex[] = "0123456789abcdef";
  size_t length = strlen(expected);
  for (size_t i = 0; i < image->dlpi_phnum; ++i) {
    const ElfW(Phdr) *segment = &image->dlpi_phdr[i];
    if (segment->p_type != PT_NOTE || segment->p_vaddr > UINTPTR_MAX - image->dlpi_addr)
      continue;
    uintptr_t address = image->dlpi_addr + segment->p_vaddr;
    if (!rinzu_elf_range(image, address, segment->p_filesz, PF_R))
      continue;
    const unsigned char *note = (const void *)address;
    size_t left = segment->p_filesz;
    while (left >= sizeof(ElfW(Nhdr))) {
      ElfW(Nhdr) header;
      memcpy(&header, note, sizeof header);
      note += sizeof header;
      left -= sizeof header;
      size_t name = ((size_t)header.n_namesz + 3) & ~(size_t)3;
      size_t data = ((size_t)header.n_descsz + 3) & ~(size_t)3;
      if (name > left || data > left - name)
        break;
      if (header.n_type == NT_GNU_BUILD_ID && header.n_namesz == 4 && !memcmp(note, "GNU", 4)) {
        if (length != (size_t)header.n_descsz * 2)
          return 0;
        for (size_t byte = 0; byte < header.n_descsz; ++byte) {
          unsigned char value = note[name + byte];
          if (expected[byte * 2] != hex[value >> 4] || expected[byte * 2 + 1] != hex[value & 15])
            return 0;
        }
        return 1;
      }
      note += name + data;
      left -= name + data;
    }
  }
  return 0;
}

static int rinzu_elf_visit(struct dl_phdr_info *image, size_t size, void *raw) {
  (void)size;
  rinzu_elf_query_t *query = raw;
  if (!image->dlpi_name || !*image->dlpi_name ||
      (query->map && image->dlpi_addr != query->map->l_addr) ||
      (query->address && !rinzu_elf_range(image, query->address, 1, PF_X)))
    return 0;
  struct stat actual;
  if (stat(image->dlpi_name, &actual)) {
    if (errno == ENOMEM)
      _Exit(71);
    return 0;
  }
  if (actual.st_dev != query->file.st_dev || actual.st_ino != query->file.st_ino)
    return 0;
  query->found = !query->build_id || rinzu_elf_build_id(image, query->build_id);
  return 1;
}

static inline int rinzu_elf_loaded(const char *path, const char *build_id, const void *address,
                                   void *handle) {
  rinzu_elf_query_t query = {.build_id = build_id, .address = (uintptr_t)address};
  if (stat(path, &query.file)) {
    if (errno == ENOMEM)
      _Exit(71);
    return 0;
  }
  if (address) {
    Dl_info info;
    struct stat owner;
    if (!dladdr(address, &info) || !info.dli_fname || stat(info.dli_fname, &owner) ||
        owner.st_dev != query.file.st_dev || owner.st_ino != query.file.st_ino)
      return 0;
  }
  if (handle) {
    struct link_map *map = NULL;
    if (dlinfo(handle, RTLD_DI_LINKMAP, &map) || !map)
      return 0;
    query.map = map;
  }
  dl_iterate_phdr(rinzu_elf_visit, &query);
  return query.found;
}
#endif

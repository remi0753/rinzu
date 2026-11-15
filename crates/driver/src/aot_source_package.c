/* Expected canonical wire/keys are compiler-validated immutable program data.
   All facade addresses are staged until the entire startup transaction passes. */
#include <dlfcn.h>
#include <errno.h>
#ifdef __linux__
#include "rinzu/elf_image.h"
#include <openssl/evp.h>
#define RINZU_RTLD_FIRST 0
#else
#include <CommonCrypto/CommonDigest.h>
#include <mach-o/dyld.h>
#include <mach-o/loader.h>
#define RINZU_RTLD_FIRST RTLD_FIRST
#endif
#include <sys/stat.h>
typedef struct {
  unsigned char key[32];
  const char *symbol;
} RinzuExpectedImport;
typedef struct {
  const char *path;
  int bundled;
  const char *uuid;
  int check_digest;
  unsigned char digest[32];
} RinzuNativeImage;
typedef struct {
  size_t image;
  const char *symbol;
  const char *resolver;
  const char *owner;
  int owner_bundled;
  const char *uuid;
  int check_digest;
  unsigned char digest[32];
} RinzuNativeSlot;
static _Noreturn void rinzu_source_package_failure(void) {
  static const char message[] = "rinzu: invalid source package artifact during AOT startup\n";
  (void)write(STDERR_FILENO, message, sizeof message - 1);
  _Exit(78);
}
static _Noreturn void rinzu_native_binding_failure(const char *reason, const char *subject) {
  static const char prefix[] = "rinzu: native binding startup failure: ";
  (void)write(STDERR_FILENO, prefix, sizeof prefix - 1);
  (void)write(STDERR_FILENO, reason, strlen(reason));
  if (subject) {
    (void)write(STDERR_FILENO, " / ", 3);
    (void)write(STDERR_FILENO, subject, strlen(subject));
  }
  (void)write(STDERR_FILENO, "\n", 1);
  rinzu_source_package_failure();
}
static char *rinzu_source_executable_directory(void) {
#ifdef __linux__
  errno = 0;
  char *resolved = realpath("/proc/self/exe", NULL);
  int error = errno;
#else
  uint32_t size = 0;
  if (_NSGetExecutablePath(NULL, &size) != -1 || !size)
    rinzu_source_package_failure();
  char *path = rinzu_failure_allocate(size);
  if (_NSGetExecutablePath(path, &size))
    rinzu_source_package_failure();
  errno = 0;
  char *resolved = realpath(path, NULL);
  int error = errno;
  free(path);
#endif
  if (!resolved) {
    if (error == ENOMEM)
      _Exit(71);
    rinzu_source_package_failure();
  }
  char *slash = strrchr(resolved, '/');
  if (!slash)
    rinzu_source_package_failure();
  *slash = 0;
  return resolved;
}
static char *rinzu_source_path(const char *root, const char *relative) {
  size_t a = strlen(root), b = strlen(relative);
  if (a > SIZE_MAX - b - 2)
    _Exit(71);
  char *path = rinzu_failure_allocate(a + b + 2);
  memcpy(path, root, a);
  path[a] = '/';
  memcpy(path + a + 1, relative, b + 1);
  errno = 0;
  char *resolved = realpath(path, NULL);
  int error = errno;
  free(path);
  if (!resolved) {
    if (error == ENOMEM)
      _Exit(71);
    rinzu_source_package_failure();
  }
  if (strncmp(resolved, root, a) != 0 || resolved[a] != '/')
    rinzu_source_package_failure();
  return resolved;
}
static void rinzu_source_file(const char *path, const unsigned char expected[32]) {
  errno = 0;
  FILE *file = fopen(path, "rb");
  if (!file) {
    if (errno == ENOMEM)
      _Exit(71);
    rinzu_source_package_failure();
  }
#ifdef __linux__
  EVP_MD_CTX *digest = EVP_MD_CTX_new();
  if (!digest)
    _Exit(71);
  if (EVP_DigestInit_ex(digest, EVP_sha256(), NULL) != 1)
    rinzu_source_package_failure();
#else
  CC_SHA256_CTX digest;
  if (!CC_SHA256_Init(&digest))
    rinzu_source_package_failure();
#endif
  unsigned char block[16384], actual[32];
  size_t size;
  for (;;) {
    errno = 0;
    size = fread(block, 1, sizeof block, file);
    if (ferror(file)) {
      if (errno == ENOMEM)
        _Exit(71);
      rinzu_source_package_failure();
    }
    if (!size)
      break;
#ifdef __linux__
    if (EVP_DigestUpdate(digest, block, size) != 1)
#else
    if (!CC_SHA256_Update(&digest, block, (CC_LONG)size))
#endif
      rinzu_source_package_failure();
    if (feof(file))
      break;
  }
  errno = 0;
  if (fclose(file)) {
    if (errno == ENOMEM)
      _Exit(71);
    rinzu_source_package_failure();
  }
#ifdef __linux__
  unsigned length = 0;
  if (EVP_DigestFinal_ex(digest, actual, &length) != 1 || length != sizeof actual)
    rinzu_source_package_failure();
  EVP_MD_CTX_free(digest);
#else
  if (!CC_SHA256_Final(actual, &digest))
    rinzu_source_package_failure();
#endif
  if (memcmp(actual, expected, 32) != 0)
    rinzu_source_package_failure();
}
static int rinzu_source_owns(const char *path, const void *address) {
#ifdef __linux__
  return address && rinzu_elf_loaded(path, NULL, address, NULL);
#else
  Dl_info info;
  struct stat expected, actual;
  if (!address || !dladdr(address, &info) || !info.dli_fname)
    return 0;
  errno = 0;
  if (stat(path, &expected)) {
    if (errno == ENOMEM)
      _Exit(71);
    return 0;
  }
  errno = 0;
  if (stat(info.dli_fname, &actual)) {
    if (errno == ENOMEM)
      _Exit(71);
    return 0;
  }
  return expected.st_dev == actual.st_dev && expected.st_ino == actual.st_ino;
#endif
}
static void *rinzu_source_symbol(void *handle, const char *name) {
  errno = 0;
  void *address = dlsym(handle, name);
  if (!address && errno == ENOMEM)
    _Exit(71);
  return address;
}
#ifdef __linux__
static int rinzu_native_loaded(const char *path, const char *build_id, void *handle) {
  return rinzu_elf_loaded(path, build_id, NULL, handle);
}
static int rinzu_native_address(const char *path, const char *build_id, void *address) {
  return address && rinzu_elf_loaded(path, build_id, address, NULL);
}
#else
static int rinzu_native_same_image(const char *a, const char *b) {
  if (!strcmp(a, b))
    return 1;
  struct stat left, right;
  errno = 0;
  if (stat(a, &left)) {
    if (errno == ENOMEM)
      _Exit(71);
    return 0;
  }
  errno = 0;
  if (stat(b, &right)) {
    if (errno == ENOMEM)
      _Exit(71);
    return 0;
  }
  return left.st_dev == right.st_dev && left.st_ino == right.st_ino;
}
static int rinzu_native_uuid(const struct mach_header *header, const char *expected) {
  if (!header || header->magic != MH_MAGIC_64 || header->cputype != CPU_TYPE_ARM64 ||
      header->ncmds > 16384 || header->sizeofcmds > 16 * 1024 * 1024)
    return 0;
  const unsigned char *cursor = (const unsigned char *)header + sizeof(struct mach_header_64);
  const unsigned char *end = cursor + header->sizeofcmds;
  for (uint32_t i = 0; i < header->ncmds; ++i) {
    if ((size_t)(end - cursor) < sizeof(struct load_command))
      return 0;
    const struct load_command *command = (const struct load_command *)cursor;
    if (command->cmdsize < sizeof(*command) || command->cmdsize > (size_t)(end - cursor))
      return 0;
    if (command->cmd == LC_UUID) {
      if (command->cmdsize != sizeof(struct uuid_command))
        return 0;
      const struct uuid_command *uuid = (const struct uuid_command *)command;
      static const char hex[] = "0123456789abcdef";
      char actual[33];
      for (size_t j = 0; j < 16; ++j) {
        actual[j * 2] = hex[uuid->uuid[j] >> 4];
        actual[j * 2 + 1] = hex[uuid->uuid[j] & 15];
      }
      actual[32] = 0;
      return !strcmp(actual, expected);
    }
    cursor += command->cmdsize;
  }
  return 0;
}
static int rinzu_native_loaded(const char *path, const char *uuid, void *handle) {
  (void)handle;
  for (uint32_t i = 0; i < _dyld_image_count(); ++i) {
    const char *name = _dyld_get_image_name(i);
    if (name && rinzu_native_same_image(path, name))
      return rinzu_native_uuid(_dyld_get_image_header(i), uuid);
  }
  return 0;
}
static int rinzu_native_address(const char *path, const char *uuid, void *address) {
  Dl_info info;
  return address && dladdr(address, &info) && info.dli_fname && info.dli_fbase &&
         rinzu_native_same_image(path, info.dli_fname) && rinzu_native_uuid(info.dli_fbase, uuid);
}
#endif
static void rinzu_native_initialize(const char *facade, void *facade_handle, const char *directory,
                                    const RinzuNativeImage *images, size_t image_count,
                                    const RinzuNativeSlot *slots, size_t slot_count,
                                    const char *digest) {
  if (!digest) {
    if (image_count || slot_count)
      rinzu_source_package_failure();
    return;
  }
  if (!image_count || !slot_count || !images || !slots || image_count > SIZE_MAX / sizeof(void *) ||
      slot_count > SIZE_MAX / sizeof(uintptr_t))
    rinzu_source_package_failure();
  void **handles = rinzu_failure_allocate(image_count * sizeof(*handles));
  uintptr_t *addresses = rinzu_failure_allocate(slot_count * sizeof(*addresses));
  // A root image may load another inventory image before its own dlopen turn.
  // Check the entire dependency inventory before loading any provider.
  for (size_t i = 0; i < image_count; ++i) {
    const RinzuNativeImage *image = &images[i];
    char *bundled = image->bundled ? rinzu_source_path(directory, image->path) : NULL;
    if (image->check_digest)
      rinzu_source_file(bundled ? bundled : image->path, image->digest);
    free(bundled);
  }
  for (size_t i = 0; i < image_count; ++i) {
    const RinzuNativeImage *image = &images[i];
    char *bundled = image->bundled ? rinzu_source_path(directory, image->path) : NULL;
    const char *path = bundled ? bundled : image->path;
    if (image->check_digest)
      rinzu_source_file(path, image->digest);
    errno = 0;
    handles[i] = dlopen(path, RTLD_NOW | RTLD_LOCAL | RINZU_RTLD_FIRST);
    if (!handles[i]) {
      if (errno == ENOMEM)
        _Exit(71);
      rinzu_native_binding_failure("cannot open explicit provider", path);
    }
    if (!rinzu_native_loaded(path, image->uuid, handles[i]))
      rinzu_native_binding_failure("provider image identity mismatch", path);
    if (image->check_digest)
      rinzu_source_file(path, image->digest);
    free(bundled);
  }
  for (size_t i = 0; i < slot_count; ++i) {
    const RinzuNativeSlot *slot = &slots[i];
    if (slot->image >= image_count)
      rinzu_source_package_failure();
    char *bundled = slot->owner_bundled ? rinzu_source_path(directory, slot->owner) : NULL;
    const char *owner = bundled ? bundled : slot->owner;
    if (slot->check_digest)
      rinzu_source_file(owner, slot->digest);
    void *address =
        rinzu_source_symbol(handles[slot->image], slot->resolver ? slot->resolver : slot->symbol);
    if (slot->resolver) {
      if (!rinzu_native_address(owner, slot->uuid, address))
        rinzu_native_binding_failure("address getter provider mismatch", slot->symbol);
      uintptr_t (*get_address)(void);
      memcpy(&get_address, &address, sizeof get_address);
      address = (void *)get_address();
    }
    if (!rinzu_native_address(owner, slot->uuid, address))
      rinzu_native_binding_failure("symbol provider mismatch", slot->symbol);
    addresses[i] = (uintptr_t)address;
    free(bundled);
  }
  void *address = rinzu_source_symbol(facade_handle, "__rinzu_bind_native_v1");
  if (!rinzu_source_owns(facade, address))
    rinzu_native_binding_failure("initializer provider mismatch", facade);
  int (*initialize)(const char *, const uintptr_t *, size_t);
  memcpy(&initialize, &address, sizeof initialize);
  if (initialize(digest, addresses, slot_count) != 1)
    rinzu_native_binding_failure("initializer rejected plan", facade);
  /* Providers and facade are pinned; only temporary arrays are released. */
  free(handles);
  free(addresses);
}
static void rinzu_source_table(const char *path, const uint8_t *wire, size_t wire_length,
                               const RinzuExpectedImport *expected, size_t count,
                               const void **staged, const char *facade_identity,
                               const char *directory, const RinzuNativeImage *images,
                               size_t image_count, const RinzuNativeSlot *slots, size_t slot_count,
                               const char *digest) {
  errno = 0;
  void *handle = dlopen(path, RTLD_NOW | RTLD_LOCAL);
  if (!handle) {
    if (errno == ENOMEM)
      _Exit(71);
    const char *error = dlerror();
    if (error) {
      (void)write(STDERR_FILENO, error, strlen(error));
      (void)write(STDERR_FILENO, "\n", 1);
    }
    rinzu_source_package_failure();
  }
  if (facade_identity && !rinzu_native_loaded(path, facade_identity, handle))
    rinzu_source_package_failure();
  rinzu_native_initialize(path, handle, directory, images, image_count, slots, slot_count, digest);
  void *address = rinzu_source_symbol(handle, RINZU_SOURCE_PACKAGE_ENTRY_V2);
  if (!rinzu_source_owns(path, address))
    rinzu_source_package_failure();
  rinzu_source_package_entry_v2_t entry;
  memcpy(&entry, &address, sizeof entry);
  const rinzu_source_package_descriptor_v2_t *table = entry();
  if (!table || table->struct_size != sizeof(*table) ||
      table->major != RINZU_SOURCE_PACKAGE_MAJOR || table->minor != RINZU_SOURCE_PACKAGE_MINOR)
    rinzu_source_package_failure();
  if (!table->descriptor || table->descriptor_len != wire_length ||
      memcmp(table->descriptor, wire, wire_length) != 0 || table->import_count != count ||
      (count && !table->imports))
    rinzu_source_package_failure();
  for (size_t index = 0; index < count; ++index) {
    const rinzu_source_import_v2_t *import = &table->imports[index];
    size_t match;
    if (!import->symbol || !rinzu_source_owns(path, import->address))
      rinzu_source_package_failure();
    for (match = 0; match < count; ++match)
      if (!strcmp(import->symbol, expected[match].symbol))
        break;
    if (match == count || memcmp(import->import_key, expected[match].key, 32) != 0 || staged[match])
      rinzu_source_package_failure();
    if (rinzu_source_symbol(handle, expected[match].symbol) != import->address)
      rinzu_source_package_failure();
    staged[match] = import->address;
  }
  for (size_t index = 0; index < count; ++index)
    if (!staged[index])
      rinzu_source_package_failure();
  /* The successful facade handle remains pinned until process exit. */
}

#include "rinzu_io.h"
#include "io_pool.h"
#include "rinzu/posix_write.h"
#include <fcntl.h>
#include <inttypes.h>
#include <pthread.h>
#include <stdio.h>
#include <strings.h>
#include <sys/acl.h>
#include <sys/attr.h>
#include <sys/stat.h>
#include <sys/xattr.h>
#include <unistd.h>
#include <xattr_flags.h>

enum WholeStep { WHOLE_OPEN, WHOLE_READ, WHOLE_WRITE, WHOLE_INSTALL, WHOLE_CLEANUP };
enum CleanupStep { CLOSE_TEMPORARY, CLOSE_ORIGINAL, REMOVE_NAME, CLOSE_DIRECTORY, CLEANUP_COUNT };

struct WholeFile {
  /* The opaque WholeFile also serves as the completion protocol handle. */
  rinzu_native_completion_v2_t completion;
  pthread_t owner;
  char *path;
  const char *directory, *name;
  char temporary[40];
  int dir_fd, source_fd, temporary_fd;
  struct stat source_info, temporary_info;
  bool temporary_identified;
  enum WholeStep step;
  enum RinzuIoFilePhase phase;
  int32_t status, cleanup[CLEANUP_COUNT];
  enum RinzuIoInstallResult installed;
  bool replacing, reserved, cleaning, eof, limit_exceeded;
  size_t maximum, capacity, write_length;
  rinzu_native_byte_array_t bytes;
  uint8_t *write_chunk;
};

static struct WholeFile *whole_owner(void *value) {
  struct WholeFile *file = value;
  if (!file || !pthread_equal(file->owner, pthread_self()))
    fatal();
  return file;
}

static bool whole_done(const struct WholeFile *file) {
  return rinzu_native_completion_is_done(&file->completion.base);
}

static struct WholeFile *whole_ready(void *value) {
  struct WholeFile *file = whole_owner(value);
  if (!whole_done(file))
    fatal();
  return file;
}

static bool whole_cancelled(const struct WholeFile *file) {
  return atomic_load_explicit(&file->completion.base.cancelled, memory_order_acquire) != 0;
}

static int32_t opened_fd(int *slot, int fd) {
  if (fd < 0)
    return native_error();
  *slot = fd;
  rinzu_io_track_resource(true, true);
  return 0;
}

static int32_t close_fd(int *slot) {
  if (*slot < 0)
    return 0;
  /* Darwin consumes the fd even on error. Retrying could close a reused fd. */
  int32_t status = close(*slot) ? native_error() : 0;
  *slot = -1;
  rinzu_io_track_resource(true, false);
  return status;
}

static bool same_inode(const struct stat *a, const struct stat *b) {
  return a->st_dev == b->st_dev && a->st_ino == b->st_ino && a->st_gen == b->st_gen;
}

static bool same_time(struct timespec a, struct timespec b) {
  return a.tv_sec == b.tv_sec && a.tv_nsec == b.tv_nsec;
}

static bool same_permissions(const struct stat *a, const struct stat *b) {
  return a->st_uid == b->st_uid && a->st_gid == b->st_gid &&
         (a->st_mode & ACCESSPERMS) == (b->st_mode & ACCESSPERMS);
}

static bool unchanged(const struct stat *a, const struct stat *b) {
  return same_inode(a, b) && same_permissions(a, b) && a->st_mode == b->st_mode &&
         a->st_flags == b->st_flags && a->st_size == b->st_size &&
         same_time(a->st_ctimespec, b->st_ctimespec) &&
         same_time(a->st_mtimespec, b->st_mtimespec) &&
         same_time(a->st_birthtimespec, b->st_birthtimespec);
}

static const int regular_flags = O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK;

static int32_t open_input(struct WholeFile *file) {
  file->phase = RINZU_IO_OPEN_FILE;
  struct stat info;
  if (!lstat(file->path, &info) && !S_ISREG(info.st_mode))
    return RINZU_IO_REGULAR;
  int fd;
  do {
    fd = open(file->path, regular_flags);
  } while (fd < 0 && errno == EINTR && !whole_cancelled(file));
  int32_t status = opened_fd(&file->source_fd, fd);
  if (status)
    return status;
  if (fstat(fd, &info))
    return native_error();
  return S_ISREG(info.st_mode) ? 0 : RINZU_IO_REGULAR;
}

static bool protected_file(const struct stat *info) {
  return (info->st_mode & (S_ISUID | S_ISGID)) ||
         (info->st_flags & (UF_IMMUTABLE | SF_IMMUTABLE | UF_APPEND | SF_APPEND));
}

static int32_t inspect_destination(struct WholeFile *file) {
  struct stat entry;
  if (fstatat(file->dir_fd, file->name, &entry, AT_SYMLINK_NOFOLLOW))
    return errno == ENOENT ? 0 : native_error();
  if (!S_ISREG(entry.st_mode))
    return RINZU_IO_REGULAR;
  if (protected_file(&entry))
    return EPERM;
  int fd;
  do {
    fd = openat(file->dir_fd, file->name, regular_flags);
  } while (fd < 0 && errno == EINTR && !whole_cancelled(file));
  int32_t status = opened_fd(&file->source_fd, fd);
  if (status)
    return status;
  struct stat *info = &file->source_info;
  if (fstat(fd, info))
    return native_error();
  if (!S_ISREG(info->st_mode))
    return RINZU_IO_REGULAR;
  if (protected_file(info))
    return EPERM;
  if (!unchanged(&entry, info))
    return ESTALE;
  return 0;
}

static int32_t create_temporary(struct WholeFile *file) {
  for (int attempt = 0; attempt < 32; attempt++) {
    if (whole_cancelled(file))
      return ECANCELED;
    uint64_t random;
    arc4random_buf(&random, sizeof(random));
    char name[sizeof(file->temporary)];
    snprintf(name, sizeof(name), ".rinzu-replace-%016" PRIx64, random);
    if (!strcasecmp(name, file->name))
      continue;
    int fd = openat(file->dir_fd, name, O_RDWR | O_CLOEXEC | O_CREAT | O_EXCL, 0600);
    if (fd < 0) {
      if (errno == EEXIST || errno == EINTR)
        continue;
      return native_error();
    }
    opened_fd(&file->temporary_fd, fd);
    strcpy(file->temporary, name);
    if (fstat(fd, &file->temporary_info))
      return native_error();
    file->temporary_identified = true;
    return 0;
  }
  return EEXIST;
}

static int32_t open_replacement(struct WholeFile *file) {
  file->phase = RINZU_IO_OPEN_DIRECTORY;
  int fd;
  do {
    fd = open(file->directory, O_SEARCH | O_CLOEXEC);
  } while (fd < 0 && errno == EINTR && !whole_cancelled(file));
  int32_t status = opened_fd(&file->dir_fd, fd);
  if (status || whole_cancelled(file))
    return status ? status : ECANCELED;
  file->phase = RINZU_IO_INSPECT_DESTINATION;
  status = inspect_destination(file);
  if (status)
    return status;
  file->phase = RINZU_IO_CREATE_TEMPORARY;
  return create_temporary(file);
}

static int32_t read_chunk(struct WholeFile *file) {
  uint8_t chunk[RINZU_IO_CHUNK];
  size_t remaining = file->maximum - file->bytes.length;
  size_t requested = !remaining ? 1 : remaining < sizeof(chunk) ? remaining : sizeof(chunk);
  ssize_t count;
  do {
    if (whole_cancelled(file))
      return ECANCELED;
    count = read(file->source_fd, chunk, requested);
  } while (count < 0 && errno == EINTR);
  if (count < 0)
    return native_error();
  if (!count) {
    file->eof = true;
    return 0;
  }
  if (!remaining) {
    file->limit_exceeded = true;
    return 0;
  }
  size_t required = file->bytes.length + (size_t)count;
  if (required > file->capacity) {
    size_t grown = file->capacity <= file->maximum / 2 ? file->capacity * 2 : file->maximum;
    size_t capacity = grown > required ? grown : required;
    uint8_t *data = realloc(file->bytes.data, capacity);
    if (!data)
      _Exit(70);
    if (!file->capacity)
      rinzu_io_track_resource(false, true);
    file->bytes.data = data;
    file->capacity = capacity;
  }
  memcpy(file->bytes.data + file->bytes.length, chunk, (size_t)count);
  file->bytes.length = required;
  return 0;
}

static int32_t write_chunk(struct WholeFile *file) {
  size_t offset = 0;
  while (offset < file->write_length) {
    if (whole_cancelled(file))
      return ECANCELED;
    ssize_t count = rinzu_posix_write(file->temporary_fd, file->write_chunk + offset,
                                      file->write_length - offset);
    if (count < 0 && errno == EINTR)
      continue;
    if (count < 0)
      return native_error();
    if (!count)
      return RINZU_IO_PROGRESS;
    offset += (size_t)count;
  }
  return 0;
}

static const char quarantine[] = "com.apple.quarantine";

struct SavedXattr {
  struct SavedXattr *next;
  char *name;
  void *data;
  size_t length;
};

struct Metadata {
  struct SavedXattr *xattrs;
  acl_t acl;
  bool has_birthtime;
  struct timespec birthtime;
};

static void free_metadata(struct Metadata *metadata) {
  if (metadata->acl)
    acl_free(metadata->acl);
  while (metadata->xattrs) {
    struct SavedXattr *attribute = metadata->xattrs;
    metadata->xattrs = attribute->next;
    free(attribute->name);
    free(attribute->data);
    free(attribute);
  }
}

static int32_t get_acl(int fd, acl_t *acl) {
  errno = 0;
  *acl = acl_get_fd_np(fd, ACL_TYPE_EXTENDED);
  return *acl || errno == ENOENT || errno == ENOTSUP ? 0 : native_error();
}

static int32_t check_acl(int fd, acl_t expected) {
  acl_t actual;
  int32_t status = get_acl(fd, &actual);
  if (status || (!actual && !expected))
    return status;
  if (actual && expected) {
    ssize_t size = acl_size(expected);
    ssize_t other = acl_size(actual);
    if (size < 0 || other < 0)
      status = native_error();
    else if (size != other)
      status = ESTALE;
    else {
      void *a = allocate((size_t)size);
      void *b = allocate((size_t)size);
      if (acl_copy_ext(a, expected, size) < 0 || acl_copy_ext(b, actual, size) < 0)
        status = native_error();
      else if (memcmp(a, b, (size_t)size) != 0)
        status = ESTALE;
      free(a);
      free(b);
    }
  } else {
    status = ESTALE;
  }
  if (actual)
    acl_free(actual);
  return status;
}

static int32_t capture_birthtime(int fd, const struct stat *info, struct Metadata *metadata) {
  struct attrlist attributes = {.bitmapcount = ATTR_BIT_MAP_COUNT,
                                .commonattr = ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_CRTIME};
  struct {
    uint32_t length;
    attribute_set_t returned;
    struct timespec time;
  } result = {0};
  if (fgetattrlist(fd, &attributes, &result, sizeof(result), 0)) {
    /* stat uses zero for volumes without a recorded birth time. */
    if (errno == ENOTSUP && same_time(info->st_birthtimespec, (struct timespec){0}))
      return 0;
    return native_error();
  }
  if (result.returned.commonattr & ATTR_CMN_CRTIME) {
    metadata->has_birthtime = true;
    metadata->birthtime = result.time;
  }
  return 0;
}

static int32_t list_xattrs(int fd, char **names, size_t *length) {
  ssize_t size = flistxattr(fd, NULL, 0, 0);
  if (size < 0)
    return errno == ENOTSUP ? 0 : native_error();
  if (!size)
    return 0;
  *names = allocate((size_t)size);
  ssize_t used = flistxattr(fd, *names, (size_t)size, 0);
  if (used < 0)
    return native_error();
  *length = (size_t)used;
  return 0;
}

static bool save_xattr(const char *name) {
  /* Quarantine is copied explicitly. Provenance is maintained by the OS. */
  return strcmp(name, quarantine) != 0 && strcmp(name, "com.apple.provenance") != 0 &&
         xattr_preserve_for_intent(name, XATTR_OPERATION_INTENT_SAVE);
}

static int32_t capture_xattr(int fd, const char *name, bool optional, struct Metadata *metadata) {
  ssize_t size = fgetxattr(fd, name, NULL, 0, 0, 0);
  if (size < 0)
    return optional && (errno == ENOATTR || errno == ENOTSUP) ? 0 : native_error();
  struct SavedXattr *attribute = allocate(sizeof(*attribute));
  attribute->next = metadata->xattrs;
  metadata->xattrs = attribute;
  attribute->name = copy_text(name);
  attribute->length = (size_t)size;
  attribute->data = allocate(size ? (size_t)size : 1);
  ssize_t received = fgetxattr(fd, name, attribute->data, (size_t)size, 0, 0);
  return received < 0 ? native_error() : received != size ? ESTALE : 0;
}

static int32_t capture_metadata(int fd, const struct stat *info, struct Metadata *metadata) {
  int32_t status = get_acl(fd, &metadata->acl);
  if (!status)
    status = capture_birthtime(fd, info, metadata);
  char *names = NULL;
  size_t length = 0;
  if (!status)
    status = list_xattrs(fd, &names, &length);
  bool listed_quarantine = false;
  for (size_t offset = 0; !status && offset < length;) {
    const char *name = names + offset;
    if (!strcmp(name, quarantine))
      listed_quarantine = true;
    else if (save_xattr(name))
      status = capture_xattr(fd, name, false, metadata);
    offset += strlen(name) + 1;
  }
  free(names);
  return status ? status : capture_xattr(fd, quarantine, !listed_quarantine, metadata);
}

static int32_t apply_metadata(int fd, const struct stat *source, const struct stat *written,
                              const struct Metadata *metadata) {
  if ((source->st_uid != written->st_uid || source->st_gid != written->st_gid) &&
      fchown(fd, source->st_uid, source->st_gid))
    return native_error();
  if (metadata->has_birthtime) {
    struct attrlist attributes = {.bitmapcount = ATTR_BIT_MAP_COUNT, .commonattr = ATTR_CMN_CRTIME};
    struct timespec time = metadata->birthtime;
    if (fsetattrlist(fd, &attributes, &time, sizeof(time), 0))
      return native_error();
  }
  for (const struct SavedXattr *item = metadata->xattrs; item; item = item->next)
    if (fsetxattr(fd, item->name, item->data, item->length, 0, 0))
      return native_error();

  if (fchmod(fd, source->st_mode & ACCESSPERMS))
    return native_error();
  if (metadata->acl)
    return acl_set_fd(fd, metadata->acl) ? native_error() : 0;
  acl_t inherited;
  int32_t status = get_acl(fd, &inherited);
  if (inherited) {
    acl_free(inherited);
    if (acl_set_fd(fd, _FILESEC_REMOVE_ACL))
      return native_error();
  }
  return status;
}

static int32_t check_xattrs(int fd, const struct Metadata *metadata, bool exact) {
  size_t expected = 0;
  bool has_quarantine = false;
  for (const struct SavedXattr *item = metadata->xattrs; item; item = item->next) {
    void *data = allocate(item->length ? item->length : 1);
    ssize_t used = fgetxattr(fd, item->name, data, item->length, 0, 0);
    int32_t status = used < 0 ? native_error()
                     : (size_t)used != item->length || memcmp(data, item->data, item->length) != 0
                         ? ESTALE
                         : 0;
    free(data);
    if (status)
      return status;
    if (!strcmp(item->name, quarantine))
      has_quarantine = true;
    else
      expected++;
  }
  if (!exact)
    return 0;
  if (!has_quarantine) {
    if (fgetxattr(fd, quarantine, NULL, 0, 0, 0) >= 0)
      return ESTALE;
    if (errno != ENOATTR && errno != ENOTSUP)
      return native_error();
  }
  char *names = NULL;
  size_t length = 0, count = 0;
  int32_t status = list_xattrs(fd, &names, &length);
  for (size_t offset = 0; !status && offset < length; offset += strlen(names + offset) + 1)
    if (save_xattr(names + offset))
      count++;
  free(names);
  return status ? status : count == expected ? 0 : ESTALE;
}

static int32_t verify_metadata(int fd, const struct stat *source, const struct stat *written,
                               const struct Metadata *metadata, struct stat *current) {
  if (fstat(fd, current))
    return native_error();
  if (!same_permissions(source, current) || written->st_size != current->st_size ||
      !same_time(written->st_mtimespec, current->st_mtimespec) ||
      (metadata->has_birthtime && !same_time(metadata->birthtime, current->st_birthtimespec)))
    return ESTALE;
  int32_t status = check_acl(fd, metadata->acl);
  return status ? status : check_xattrs(fd, metadata, false);
}

static int32_t recheck_destination(struct WholeFile *file, const struct Metadata *metadata) {
  if (file->source_fd < 0)
    return 0; /* The exclusive rename checks absence at installation. */
  int32_t status = check_acl(file->source_fd, metadata->acl);
  if (!status)
    status = check_xattrs(file->source_fd, metadata, true);
  if (status)
    return status;
  struct stat current;
  if (fstatat(file->dir_fd, file->name, &current, AT_SYMLINK_NOFOLLOW))
    return native_error();
  return unchanged(&file->source_info, &current) ? 0 : ESTALE;
}

static bool install_rejected(int32_t status) {
  /* Darwin's documented refusals. I/O or transport failures may leave the
     installation outcome unknown, so keep an explicit allowlist. */
  switch (status) {
  case EACCES:
  case EBADF:
  case EDEADLK:
  case EDQUOT:
  case EEXIST:
  case EFAULT:
  case EINVAL:
  case EISDIR:
  case ELOOP:
  case ENAMETOOLONG:
  case ENOENT:
  case ENOSPC:
  case ENOTDIR:
  case ENOTEMPTY:
  case ENOTSUP:
  case ENOSYS:
  case EPERM:
  case EROFS:
  case EXDEV:
    return true;
  default:
    return false;
  }
}

static int32_t prepare_install(struct WholeFile *file, struct Metadata *metadata) {
  struct stat prepared;
  if (file->source_fd >= 0) {
    file->phase = RINZU_IO_COPY_METADATA;
    struct stat written;
    if (fstat(file->temporary_fd, &written))
      return native_error();
    int32_t status = capture_metadata(file->source_fd, &file->source_info, metadata);
    if (status || whole_cancelled(file))
      return status ? status : ECANCELED;
    status = apply_metadata(file->temporary_fd, &file->source_info, &written, metadata);
    if (status || whole_cancelled(file))
      return status ? status : ECANCELED;
    status = verify_metadata(file->temporary_fd, &file->source_info, &written, metadata, &prepared);
    if (status)
      return status;
  } else if (fstat(file->temporary_fd, &prepared)) {
    return native_error();
  }
  file->phase = RINZU_IO_FLUSH;
  int result;
  do {
    if (whole_cancelled(file))
      return ECANCELED;
    result = fsync(file->temporary_fd);
  } while (result && errno == EINTR);
  if (result)
    return native_error();
  file->phase = RINZU_IO_INSPECT_DESTINATION;
  int32_t status = recheck_destination(file, metadata);
  if (status)
    return status;
  struct stat entry;
  if (fstatat(file->dir_fd, file->temporary, &entry, AT_SYMLINK_NOFOLLOW))
    return native_error();
  if (!unchanged(&prepared, &entry) || entry.st_nlink != 1)
    return ESTALE;
  return 0;
}

static int32_t install_file(struct WholeFile *file) {
  struct Metadata metadata = {0};
  int32_t status = prepare_install(file, &metadata);
  free_metadata(&metadata);
  if (status)
    return status;

  file->phase = RINZU_IO_INSTALL;
  /* The RMW orders the install decision against both the runtime's cancel or
     timeout store and the Owner's cleanup request. Past this point only the
     syscall's result decides the recorded outcome. */
  uint32_t expected = 0;
  if (!atomic_compare_exchange_strong_explicit(&file->completion.base.cancelled, &expected, 0,
                                               memory_order_acq_rel, memory_order_acquire))
    return ECANCELED;
  file->installed = RINZU_IO_INSTALLING;
  int result =
      file->source_fd >= 0
          ? renameat(file->dir_fd, file->temporary, file->dir_fd, file->name)
          : renameatx_np(file->dir_fd, file->temporary, file->dir_fd, file->name, RENAME_EXCL);
  if (!result) {
    file->installed = RINZU_IO_INSTALLED;
    return 0;
  }
  status = native_error();
  file->installed = install_rejected(status) ? RINZU_IO_NOT_INSTALLED : RINZU_IO_INDETERMINATE;
  return status;
}

static int32_t remove_temporary(struct WholeFile *file, int32_t identity_status) {
  if (!file->temporary[0] || file->installed == RINZU_IO_INSTALLED)
    return 0;
  struct stat current;
  if (fstatat(file->dir_fd, file->temporary, &current, AT_SYMLINK_NOFOLLOW))
    return errno == ENOENT ? 0 : native_error();
  if (identity_status)
    return identity_status;
  if (same_inode(&file->temporary_info, &current) && unlinkat(file->dir_fd, file->temporary, 0) &&
      errno != ENOENT)
    return native_error();
  return 0;
}

static void cleanup_file(struct WholeFile *file) {
  int32_t identity_status = 0;
  if (file->temporary_fd >= 0 && !file->temporary_identified &&
      fstat(file->temporary_fd, &file->temporary_info))
    identity_status = native_error();
  file->cleanup[CLOSE_TEMPORARY] = close_fd(&file->temporary_fd);
  file->cleanup[CLOSE_ORIGINAL] = close_fd(&file->source_fd);
  file->cleanup[REMOVE_NAME] = remove_temporary(file, identity_status);
  file->cleanup[CLOSE_DIRECTORY] = close_fd(&file->dir_fd);
}

static void perform_whole(void *context) {
  struct WholeFile *file = context;
  if (file->step == WHOLE_CLEANUP) {
    cleanup_file(file);
  } else if (whole_cancelled(file)) {
    file->status = ECANCELED;
  } else {
    switch (file->step) {
    case WHOLE_OPEN:
      file->status = file->replacing ? open_replacement(file) : open_input(file);
      break;
    case WHOLE_READ:
      file->status = read_chunk(file);
      break;
    case WHOLE_WRITE:
      file->status = write_chunk(file);
      break;
    case WHOLE_INSTALL:
      file->status = install_file(file);
      break;
    case WHOLE_CLEANUP:
      fatal();
    }
  }
  rinzu_io_publish_reserved(&file->completion);
}

static void submit_whole(struct WholeFile *file, enum WholeStep step, enum RinzuIoFilePhase phase) {
  if (!file->reserved)
    fatal();
  file->step = step;
  if (step != WHOLE_CLEANUP) {
    file->phase = phase;
    file->status = 0;
  }
  rinzu_native_completion_v2_init(&file->completion);
  rinzu_io_submit_reserved(perform_whole, file);
}

void *rinzu_io_whole_new(const char *path, int64_t maximum, bool replacing) {
  if (maximum < 0 || (uint64_t)maximum > SIZE_MAX)
    fatal();
  struct WholeFile *file = allocate(sizeof(*file));
  file->owner = pthread_self();
  file->path = copy_text(path);
  file->dir_fd = file->source_fd = file->temporary_fd = -1;
  file->replacing = replacing;
  file->maximum = (size_t)maximum;
  rinzu_native_completion_v2_init(&file->completion);
  rinzu_native_completion_v2_publish(&file->completion);
  return file;
}

void rinzu_io_whole_open(void *record) {
  struct WholeFile *file = whole_ready(record);
  if (file->replacing) {
    char *slash = strrchr(file->path, '/');
    file->name = slash ? slash + 1 : file->path;
    if (!*file->name || !strcmp(file->name, ".") || !strcmp(file->name, "..")) {
      file->phase = RINZU_IO_VALIDATE_PATH;
      file->status = RINZU_IO_ARGUMENT;
      return;
    }
    file->directory = !slash ? "." : slash == file->path ? "/" : file->path;
    if (slash)
      *slash = '\0';
  }
  file->phase = RINZU_IO_SUBMIT;
  file->status = rinzu_io_reserve(sizeof(*file));
  if (!file->status) {
    file->reserved = true;
    submit_whole(file, WHOLE_OPEN, file->replacing ? RINZU_IO_OPEN_DIRECTORY : RINZU_IO_OPEN_FILE);
  }
}

void rinzu_io_whole_read(void *record) {
  submit_whole(whole_ready(record), WHOLE_READ, RINZU_IO_READ);
}

void rinzu_io_whole_write(void *record, const uint8_t *data, size_t length) {
  struct WholeFile *file = whole_ready(record);
  if (!length || length > RINZU_IO_CHUNK)
    fatal();
  if (!file->write_chunk) {
    file->write_chunk = allocate(RINZU_IO_CHUNK);
    rinzu_io_track_resource(false, true);
  }
  memcpy(file->write_chunk, data, length);
  file->write_length = length;
  submit_whole(file, WHOLE_WRITE, RINZU_IO_WRITE);
}

void rinzu_io_whole_install(void *record) {
  submit_whole(whole_ready(record), WHOLE_INSTALL, RINZU_IO_INSTALL);
}

bool rinzu_io_whole_cancel_pending(void *record) {
  struct WholeFile *file = whole_owner(record);
  if (whole_done(file))
    return false;
  atomic_store_explicit(&file->completion.base.cancelled, 1, memory_order_release);
  return true;
}

bool rinzu_io_whole_cleanup(void *record) {
  struct WholeFile *file = whole_ready(record);
  if (!file->reserved || file->cleaning)
    return false;
  file->cleaning = true;
  submit_whole(file, WHOLE_CLEANUP, RINZU_IO_CLOSE_FILE);
  return true;
}

void rinzu_io_whole_release(void *record) {
  struct WholeFile *file = whole_ready(record);
  if (file->reserved && !file->cleaning)
    fatal();
  if (file->bytes.data || file->write_chunk)
    rinzu_io_track_resource(false, false);
  free(file->bytes.data);
  free(file->write_chunk);
  free(file->path);
  if (file->reserved)
    rinzu_io_release_reservation(sizeof(*file));
  free(file);
}

int64_t rinzu_io_whole_status(void *record) {
  return whole_ready(record)->status;
}
int64_t rinzu_io_whole_phase(void *record) {
  return whole_ready(record)->phase;
}
bool rinzu_io_whole_eof(void *record) {
  return whole_ready(record)->eof;
}
bool rinzu_io_whole_limit_exceeded(void *record) {
  return whole_ready(record)->limit_exceeded;
}
const rinzu_native_byte_array_t *rinzu_io_whole_bytes(void *record) {
  struct WholeFile *file = whole_ready(record);
  if (!file->eof || file->status)
    fatal();
  return &file->bytes;
}
int64_t rinzu_io_whole_install_result(void *record) {
  return whole_ready(record)->installed;
}
int64_t rinzu_io_whole_cleanup_status(void *record, int64_t index) {
  if (index < 0 || index >= CLEANUP_COUNT)
    fatal();
  return whole_ready(record)->cleanup[index];
}
const char *rinzu_io_whole_temporary_name(void *record) {
  return whole_ready(record)->temporary;
}

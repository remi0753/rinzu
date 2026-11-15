#include "rinzu_io.h"
#include "io_pool.h"
#include "rinzu/posix_write.h"
#include <fcntl.h>
#include <inttypes.h>
#include <pthread.h>
#include <stdio.h>
#include <strings.h>
#include <sys/stat.h>
#include <unistd.h>

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
  /* Retrying close could close a reused fd. */
  int32_t status = close(*slot) ? native_error() : 0;
  *slot = -1;
  rinzu_io_track_resource(true, false);
  return status;
}

#if defined(__linux__)
#include "file_metadata_linux.h"
#else
#include "file_metadata_macos.h"
#endif

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

static int32_t inspect_destination(struct WholeFile *file) {
  struct stat entry;
  if (fstatat(file->dir_fd, file->name, &entry, AT_SYMLINK_NOFOLLOW))
    return errno == ENOENT ? 0 : native_error();
  if (!S_ISREG(entry.st_mode))
    return RINZU_IO_REGULAR;
  int32_t status = protected_status(-1, &entry);
  if (status)
    return status;
  int fd;
  do {
    fd = openat(file->dir_fd, file->name, regular_flags);
  } while (fd < 0 && errno == EINTR && !whole_cancelled(file));
  status = opened_fd(&file->source_fd, fd);
  if (status)
    return status;
  struct stat *info = &file->source_info;
  if (fstat(fd, info))
    return native_error();
  if (!S_ISREG(info->st_mode))
    return RINZU_IO_REGULAR;
  status = protected_status(fd, info);
  if (status)
    return status;
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
    fd = open(file->directory, directory_flags);
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
  if (!unchanged(&file->source_info, &current))
    return ESTALE;
  return protected_status(file->source_fd, &current);
}

static bool install_rejected(int32_t status) {
  /* These errors indicate a rejected rename. I/O or transport failures may
     leave the installation outcome unknown. */
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
  int result = file->source_fd >= 0
                   ? renameat(file->dir_fd, file->temporary, file->dir_fd, file->name)
                   : install_exclusive(file->dir_fd, file->temporary, file->name);
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

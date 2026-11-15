#include <acl/libacl.h>
#include <linux/fs.h>
#include <sys/ioctl.h>
#include <sys/xattr.h>

#include "file_metadata.h"

static const mode_t metadata_mode = S_IRWXU | S_IRWXG | S_IRWXO | S_ISVTX;
static const int directory_flags = O_PATH | O_DIRECTORY | O_CLOEXEC;

static bool same_inode(const struct stat *a, const struct stat *b) {
  return a->st_dev == b->st_dev && a->st_ino == b->st_ino;
}

static bool same_time(struct timespec a, struct timespec b) {
  return a.tv_sec == b.tv_sec && a.tv_nsec == b.tv_nsec;
}

static bool same_permissions(const struct stat *a, const struct stat *b) {
  return a->st_uid == b->st_uid && a->st_gid == b->st_gid &&
         (a->st_mode & metadata_mode) == (b->st_mode & metadata_mode);
}

static bool unchanged(const struct stat *a, const struct stat *b) {
  return same_inode(a, b) && same_permissions(a, b) && a->st_mode == b->st_mode &&
         a->st_size == b->st_size && same_time(a->st_ctim, b->st_ctim) &&
         same_time(a->st_mtim, b->st_mtim);
}

static int32_t protected_status(int fd, const struct stat *info) {
  if (info->st_mode & (S_ISUID | S_ISGID))
    return EPERM;
  if (fd < 0)
    return 0;
  int flags = 0;
  if (ioctl(fd, FS_IOC_GETFLAGS, &flags)) {
    if (errno == ENOTTY || errno == ENOTSUP || errno == ENOSYS)
      return 0;
    return native_error();
  }
  return flags & (FS_IMMUTABLE_FL | FS_APPEND_FL) ? EPERM : 0;
}

static int32_t get_acl(int fd, acl_t *acl) {
  *acl = acl_get_fd(fd);
  return *acl || errno == ENOTSUP ? 0 : native_error();
}

static int32_t check_acl(int fd, acl_t expected) {
  acl_t actual = NULL;
  int32_t status = get_acl(fd, &actual);
  if (status)
    return status;
  if (actual && expected) {
    int comparison = acl_cmp(expected, actual);
    status = comparison < 0 ? native_error() : comparison ? ESTALE : 0;
  } else if (actual || expected) {
    status = ESTALE;
  }
  if (actual)
    acl_free(actual);
  return status;
}

static int32_t list_xattrs(int fd, char **names, size_t *length) {
  ssize_t size = flistxattr(fd, NULL, 0);
  if (size < 0)
    return errno == ENOTSUP ? 0 : native_error();
  if (!size)
    return 0;
  *names = allocate((size_t)size);
  ssize_t used = flistxattr(fd, *names, (size_t)size);
  if (used < 0)
    return native_error();
  *length = (size_t)used;
  return 0;
}

static bool save_xattr(const char *name) {
  return !strncmp(name, "user.", 5);
}

static int32_t capture_metadata(int fd, const struct stat *info, struct Metadata *metadata) {
  (void)info;
  int32_t status = get_acl(fd, &metadata->acl);
  char *names = NULL;
  size_t length = 0;
  if (!status)
    status = list_xattrs(fd, &names, &length);
  for (size_t offset = 0; !status && offset < length; offset += strlen(names + offset) + 1) {
    const char *name = names + offset;
    if (!save_xattr(name))
      continue;
    ssize_t size = fgetxattr(fd, name, NULL, 0);
    if (size < 0) {
      status = native_error();
      break;
    }
    struct SavedXattr *attribute = allocate(sizeof(*attribute));
    *attribute = (struct SavedXattr){metadata->xattrs, copy_text(name),
                                     allocate(size ? (size_t)size : 1), (size_t)size};
    metadata->xattrs = attribute;
    ssize_t received = fgetxattr(fd, name, attribute->data, (size_t)size);
    status = received < 0 ? native_error() : received != size ? ESTALE : 0;
  }
  free(names);
  return status;
}

static int32_t apply_metadata(int fd, const struct stat *source, const struct stat *written,
                              const struct Metadata *metadata) {
  if ((source->st_uid != written->st_uid || source->st_gid != written->st_gid) &&
      fchown(fd, source->st_uid, source->st_gid))
    return native_error();
  if (fchmod(fd, source->st_mode & metadata_mode))
    return native_error();
  if (metadata->acl && acl_set_fd(fd, metadata->acl))
    return native_error();
  for (const struct SavedXattr *item = metadata->xattrs; item; item = item->next)
    if (fsetxattr(fd, item->name, item->data, item->length, 0))
      return native_error();
  return 0;
}

static int32_t check_xattrs(int fd, const struct Metadata *metadata, bool exact) {
  size_t expected = 0;
  for (const struct SavedXattr *item = metadata->xattrs; item; item = item->next) {
    void *data = allocate(item->length ? item->length : 1);
    ssize_t used = fgetxattr(fd, item->name, data, item->length);
    int32_t status = used < 0 ? native_error()
                     : (size_t)used != item->length || memcmp(data, item->data, item->length)
                         ? ESTALE
                         : 0;
    free(data);
    if (status)
      return status;
    expected++;
  }
  if (!exact)
    return 0;
  char *names = NULL;
  size_t length = 0, count = 0;
  int32_t status = list_xattrs(fd, &names, &length);
  for (size_t offset = 0; !status && offset < length; offset += strlen(names + offset) + 1)
    count += save_xattr(names + offset);
  free(names);
  return status ? status : count == expected ? 0 : ESTALE;
}

static int32_t verify_metadata(int fd, const struct stat *source, const struct stat *written,
                               const struct Metadata *metadata, struct stat *current) {
  if (fstat(fd, current))
    return native_error();
  if (!same_permissions(source, current) || written->st_size != current->st_size ||
      !same_time(written->st_mtim, current->st_mtim))
    return ESTALE;
  int32_t status = check_acl(fd, metadata->acl);
  return status ? status : check_xattrs(fd, metadata, true);
}

static int install_exclusive(int directory, const char *temporary, const char *name) {
  return renameat2(directory, temporary, directory, name, RENAME_NOREPLACE);
}

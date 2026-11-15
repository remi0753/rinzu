#include <sys/acl.h>
#include <sys/attr.h>
#include <sys/xattr.h>
#include <xattr_flags.h>

static const int directory_flags = O_SEARCH | O_CLOEXEC;

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

static int32_t protected_status(int fd, const struct stat *info) {
  (void)fd;
  return (info->st_mode & (S_ISUID | S_ISGID)) ||
                 (info->st_flags & (UF_IMMUTABLE | SF_IMMUTABLE | UF_APPEND | SF_APPEND))
             ? EPERM
             : 0;
}

static const char quarantine[] = "com.apple.quarantine";

#include "file_metadata.h"

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

static int install_exclusive(int directory, const char *temporary, const char *name) {
  return renameatx_np(directory, temporary, directory, name, RENAME_EXCL);
}

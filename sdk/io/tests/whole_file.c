#include "rinzu_io.h"
#include "rinzu/posix_write.h"
#include <assert.h>
#include <dirent.h>
#include <fcntl.h>
#include <inttypes.h>
#include <membership.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <sys/acl.h>
#include <sys/attr.h>
#include <sys/stat.h>
#include <sys/xattr.h>
#include <time.h>
#include <unistd.h>

#ifndef RINZU_WHOLE_FILE_PROBE
#include "../native/src/io.c"
#endif

enum Fault {
  NONE,
  DENY_LIST,
  DENY_XATTR,
  DROP_XATTR,
  DENY_CHOWN,
  DROP_MODE,
  WRITE_STALLED,
  FLUSH_FAILED,
  INSTALL_DENIED,
  INSTALL_IO_ERROR,
  INSTALL_IO_ERROR_AFTER_RENAME,
  CREATE_COLLISION,
  EXCLUSIVE_UNSUPPORTED,
  CLOSES_FAILED,
  REMOVE_FAILED,
  PROTECTED_TARGET,
  QUARANTINE_UNREADABLE
};
enum Gate { NO_GATE, OPENED, READING, WRITING, APPLYING, BEFORE_INSTALL, IN_RENAME, IN_CLOSE };
static _Atomic int fault, gate, entered, close_count;
static _Atomic bool partial;

static void pause_once(void) {
  struct timespec interval = {0, 1000000};
  nanosleep(&interval, NULL);
}

static void pause_at(enum Gate stage) {
  if (atomic_load(&gate) != (int)stage)
    return;
  atomic_store(&entered, stage);
  while (atomic_load(&gate) == (int)stage)
    pause_once();
}

static int fail(int status) {
  errno = status;
  return -1;
}

static int probe_open(const char *path, int flags) {
  int fd = open(path, flags);
  if (fd >= 0)
    pause_at(OPENED);
  return fd;
}

static int probe_fstatat(int fd, const char *path, struct stat *info, int flags) {
  int result = fstatat(fd, path, info, flags);
  if (!result && atomic_load(&fault) == PROTECTED_TARGET && !strcmp(path, "destination"))
    info->st_mode |= S_ISUID;
  return result;
}

static ssize_t probe_read(int fd, void *data, size_t length) {
  pause_at(READING);
  return read(fd, data, atomic_load(&partial) && length > 3 ? 3 : length);
}

static ssize_t probe_write(int fd, const void *data, size_t length) {
  pause_at(WRITING);
  if (atomic_load(&fault) == WRITE_STALLED)
    return 0;
  return rinzu_posix_write(fd, data, atomic_load(&partial) && length > 3 ? 3 : length);
}

static ssize_t probe_list(int fd, char *names, size_t size, int options) {
  if (atomic_load(&fault) == DENY_LIST)
    return fail(EACCES);
  return flistxattr(fd, names, size, options);
}

static ssize_t probe_getxattr(int fd, const char *name, void *data, size_t length,
                              uint32_t position, int options) {
  if (atomic_load(&fault) == QUARANTINE_UNREADABLE && !strcmp(name, "com.apple.quarantine"))
    return fail(ENOTSUP);
  return fgetxattr(fd, name, data, length, position, options);
}

static int probe_xattr(int fd, const char *name, const void *data, size_t length, uint32_t position,
                       int options) {
  pause_at(APPLYING);
  if (atomic_load(&fault) == DENY_XATTR)
    return fail(EPERM);
  if (atomic_load(&fault) == DROP_XATTR)
    return 0;
  return fsetxattr(fd, name, data, length, position, options);
}

static int probe_chown(int fd, uid_t uid, gid_t gid) {
  if (atomic_load(&fault) == DENY_CHOWN)
    return fail(EPERM);
  return fchown(fd, uid, gid);
}

static int probe_chmod(int fd, mode_t mode) {
  if (atomic_load(&fault) == DROP_MODE)
    return 0;
  return fchmod(fd, mode);
}

static int probe_fsync(int fd) {
  if (atomic_load(&fault) == FLUSH_FAILED)
    return fail(EIO);
  int result = fsync(fd);
  if (!result)
    pause_at(BEFORE_INSTALL);
  return result;
}

static int probe_rename(int from_dir, const char *from, int to_dir, const char *to) {
  pause_at(IN_RENAME);
  int mode = atomic_load(&fault);
  if (mode == INSTALL_DENIED || mode == REMOVE_FAILED)
    return fail(EACCES);
  if (mode == INSTALL_IO_ERROR)
    return fail(EIO);
  int result = renameat(from_dir, from, to_dir, to);
  return !result && mode == INSTALL_IO_ERROR_AFTER_RENAME ? fail(EIO) : result;
}

static int probe_rename_exclusive(int from_dir, const char *from, int to_dir, const char *to,
                                  unsigned flags) {
  int mode = atomic_load(&fault);
  if (mode == CREATE_COLLISION) {
    int fd = openat(to_dir, to, O_WRONLY | O_CREAT | O_EXCL, 0600);
    assert(fd >= 0 && write(fd, "other", 5) == 5 && !close(fd));
  }
  if (mode == EXCLUSIVE_UNSUPPORTED)
    return fail(ENOTSUP);
  return renameatx_np(from_dir, from, to_dir, to, flags);
}

static int probe_close(int fd) {
  pause_at(IN_CLOSE);
  int result = close(fd);
  int mode = atomic_load(&fault);
  if (!result && (mode == CLOSES_FAILED || mode == REMOVE_FAILED)) {
    const int statuses[] = {EINTR, EIO, EBADF};
    int index = atomic_fetch_add(&close_count, 1);
    assert(index < 3);
    return fail(statuses[index]);
  }
  return result;
}

static int probe_unlink(int fd, const char *name, int flags) {
  if (atomic_load(&fault) == REMOVE_FAILED)
    return fail(EPERM);
  return unlinkat(fd, name, flags);
}

#define open probe_open
#define fstatat probe_fstatat
#define read probe_read
#define rinzu_posix_write probe_write
#define flistxattr probe_list
#define fgetxattr probe_getxattr
#define fsetxattr probe_xattr
#define fchown probe_chown
#define fchmod probe_chmod
#define fsync probe_fsync
#define renameat probe_rename
#define renameatx_np probe_rename_exclusive
#define close probe_close
#define unlinkat probe_unlink
#define rinzu_io_whole_new new_uninstrumented
#define rinzu_io_whole_release release_uninstrumented
#include "../native/src/whole_file.c"
#undef open
#undef fstatat
#undef read
#undef rinzu_posix_write
#undef flistxattr
#undef fgetxattr
#undef fsetxattr
#undef fchown
#undef fchmod
#undef fsync
#undef renameat
#undef renameatx_np
#undef close
#undef unlinkat
#undef rinzu_io_whole_new
#undef rinzu_io_whole_release

/* Also compiled as a private SDK for the Source cancellation tests. */
static struct WholeFile *last_replacement;
static int64_t last_outcome;

void *rinzu_io_whole_new(const char *path, int64_t maximum, bool replacing) {
  struct WholeFile *file = new_uninstrumented(path, maximum, replacing);
  if (replacing)
    last_replacement = file;
  return file;
}

void rinzu_io_whole_release(void *value) {
  struct WholeFile *file = value;
  if (file == last_replacement) {
    last_outcome = file->installed;
    last_replacement = NULL;
  }
  release_uninstrumented(value);
}

void whole_test_fault(int64_t mode) {
  atomic_store(&close_count, 0);
  atomic_store(&fault, (int)mode);
}

void whole_test_gate(int64_t stage) {
  atomic_store(&entered, 0);
  atomic_store(&gate, (int)stage);
}

bool whole_test_entered(void) {
  return atomic_load(&entered) != 0;
}

bool whole_test_cancelled(void) {
  return last_replacement && whole_cancelled(last_replacement);
}

int64_t whole_test_outcome(void) {
  return last_outcome;
}

bool whole_test_remove_name(const char *name) {
  return unlink(name) == 0;
}

#ifndef RINZU_WHOLE_FILE_PROBE
static void wait_done(struct WholeFile *file) {
  int64_t deadline = rinzu_io_monotonic() + 5000;
  while (!whole_done(file)) {
    assert(rinzu_io_monotonic() < deadline);
    pause_once();
  }
}

static void expect_status(struct WholeFile *file, enum RinzuIoFilePhase phase, int32_t status) {
  wait_done(file);
  int32_t actual = rinzu_io_whole_status(file);
  int32_t actual_phase = rinzu_io_whole_phase(file);
  if (actual != status || (status && actual_phase != (int32_t)phase)) {
    fprintf(stderr, "whole-file fault %d: expected phase %d status %d, got phase %d status %d\n",
            atomic_load(&fault), phase, status, actual_phase, actual);
    abort();
  }
}

static void settled(void) {
  assert(!rinzu_io_live_files() && !rinzu_io_live_buffers() && !rinzu_io_live_operations());
  assert(!rinzu_io_retained_controls() && !running && !queued && !slots);
}

static void finish(struct WholeFile *file) {
  assert(rinzu_io_whole_cleanup(file));
  wait_done(file);
  assert(!rinzu_io_whole_cleanup(file));
  for (int index = 0; index < CLEANUP_COUNT; index++)
    assert(!rinzu_io_whole_cleanup_status(file, index));
  rinzu_io_whole_release(file);
  settled();
}

static void write_input(const char *path, const char *contents, mode_t mode) {
  int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, mode);
  assert(fd >= 0 && write(fd, contents, strlen(contents)) == (ssize_t)strlen(contents));
  assert(!close(fd));
}

static void expect_contents(const char *path, const char *expected) {
  char contents[32];
  int fd = open(path, O_RDONLY);
  assert(fd >= 0 && read(fd, contents, sizeof(contents)) == (ssize_t)strlen(expected));
  assert(!memcmp(contents, expected, strlen(expected)) && !close(fd));
}

static void no_temporary(void) {
  DIR *directory = opendir(".");
  assert(directory);
  struct dirent *entry;
  while ((entry = readdir(directory)))
    assert(strncmp(entry->d_name, ".rinzu-replace-", strlen(".rinzu-replace-")));
  assert(!closedir(directory));
}

static struct WholeFile *begin_whole(const char *path, bool replacing, int64_t maximum) {
  struct WholeFile *file = rinzu_io_whole_new(path, maximum, replacing);
  rinzu_io_whole_open(file);
  expect_status(file, RINZU_IO_OPEN_FILE, 0);
  return file;
}

static struct WholeFile *prepared(const char *path) {
  struct WholeFile *file = begin_whole(path, true, 0);
  rinzu_io_whole_write(file, (const uint8_t *)"new value", 9);
  expect_status(file, RINZU_IO_WRITE, 0);
  return file;
}

static acl_t test_acl(bool inherit) {
  acl_t acl = acl_init(1);
  acl_entry_t entry;
  uuid_t user;
  acl_permset_t permissions;
  assert(acl && !acl_create_entry(&acl, &entry) && !mbr_uid_to_uuid(geteuid(), user));
  assert(!acl_set_tag_type(entry, ACL_EXTENDED_ALLOW) && !acl_set_qualifier(entry, user));
  assert(!acl_get_permset(entry, &permissions) && !acl_add_perm(permissions, ACL_READ_DATA));
  if (inherit) {
    acl_flagset_t flags;
    assert(!acl_get_flagset_np(entry, &flags) && !acl_add_flag_np(flags, ACL_ENTRY_FILE_INHERIT));
  }
  return acl;
}

static const char *attribute_names[] = {"com.apple.metadata:_kMDItemUserTags",
                                        "com.apple.metadata:kMDItemFinderComment", "org.rinzu.test",
                                        "com.apple.quarantine"};
static const char *attribute_values[] = {"tag", "comment", "app value", "0081;5f000000;Rinzu;"};

static void seed_metadata(void) {
  write_input("destination", "old", 0640);
  int fd = open("destination", O_RDWR);
  assert(fd >= 0 && !fchmod(fd, 0640));
  struct attrlist attributes = {.bitmapcount = ATTR_BIT_MAP_COUNT, .commonattr = ATTR_CMN_CRTIME};
  struct timespec birth = {1000000000, 0};
  struct timespec times[] = {{.tv_nsec = UTIME_OMIT}, {1100000000, 0}};
  assert(!fsetattrlist(fd, &attributes, &birth, sizeof(birth), 0) && !futimens(fd, times));
  for (size_t i = 0; i < sizeof(attribute_names) / sizeof(attribute_names[0]); i++)
    assert(
        !fsetxattr(fd, attribute_names[i], attribute_values[i], strlen(attribute_values[i]), 0, 0));
  assert(!fsetxattr(fd, "org.rinzu.content#C", "old content", 11, 0, 0));
  acl_t acl = test_acl(false);
  assert(!acl_set_fd(fd, acl) && !acl_free(acl) && !close(fd));
}

static void check_metadata(void) {
  seed_metadata();
  assert(!link("destination", "old-link"));
  struct stat original, current;
  assert(!stat("destination", &original));
  struct WholeFile *file = prepared("destination");
  rinzu_io_whole_install(file);
  expect_status(file, RINZU_IO_INSTALL, 0);
  assert(rinzu_io_whole_install_result(file) == RINZU_IO_INSTALLED);
  finish(file);
  expect_contents("destination", "new value");
  expect_contents("old-link", "old");
  int fd = open("destination", O_RDONLY);
  assert(fd >= 0 && !fstat(fd, &current));
  assert(!same_inode(&original, &current) && same_permissions(&original, &current));
  assert(same_time(original.st_birthtimespec, current.st_birthtimespec));
  assert(original.st_mtimespec.tv_sec < current.st_mtimespec.tv_sec);
  acl_t acl = test_acl(false);
  assert(!check_acl(fd, acl) && !acl_free(acl));
  char data[64];
  for (size_t i = 0; i < sizeof(attribute_names) / sizeof(attribute_names[0]); i++) {
    size_t length = strlen(attribute_values[i]);
    assert(fgetxattr(fd, attribute_names[i], data, sizeof(data), 0, 0) == (ssize_t)length);
    assert(!memcmp(data, attribute_values[i], length));
  }
  assert(fgetxattr(fd, "org.rinzu.content#C", NULL, 0, 0, 0) < 0 && errno == ENOATTR);
  assert(!close(fd) && !unlink("old-link") && !unlink("destination"));

  assert(!mkdir("parent", 0700));
  write_input("parent/no-acl", "old", 0640);
  acl = test_acl(true);
  assert(!acl_set_file("parent", ACL_TYPE_EXTENDED, acl) && !acl_free(acl));
  mode_t mask = umask(0200);
  file = prepared("parent/new");
  rinzu_io_whole_install(file);
  expect_status(file, RINZU_IO_INSTALL, 0);
  finish(file);
  umask(mask);
  fd = open("parent/new", O_RDONLY);
  assert(fd >= 0 && !fstat(fd, &current) && (current.st_mode & ACCESSPERMS) == 0400);
  assert(!get_acl(fd, &acl) && acl && !acl_free(acl) && !close(fd));
  file = prepared("parent/no-acl");
  rinzu_io_whole_install(file);
  expect_status(file, RINZU_IO_INSTALL, 0);
  finish(file);
  fd = open("parent/no-acl", O_RDONLY);
  assert(fd >= 0 && !check_acl(fd, NULL) && !close(fd));
  assert(!unlink("parent/new") && !unlink("parent/no-acl") && !rmdir("parent"));
}

static void metadata_and_install_failures(void) {
  seed_metadata();
  const struct {
    enum Fault fault;
    enum RinzuIoFilePhase phase;
    int32_t status;
    enum RinzuIoInstallResult installed;
  } cases[] = {{DENY_LIST, RINZU_IO_COPY_METADATA, EACCES, RINZU_IO_NOT_INSTALLED},
               {QUARANTINE_UNREADABLE, RINZU_IO_COPY_METADATA, ENOTSUP, RINZU_IO_NOT_INSTALLED},
               {DENY_XATTR, RINZU_IO_COPY_METADATA, EPERM, RINZU_IO_NOT_INSTALLED},
               {DROP_XATTR, RINZU_IO_COPY_METADATA, ENOATTR, RINZU_IO_NOT_INSTALLED},
               {DENY_CHOWN, RINZU_IO_COPY_METADATA, EPERM, RINZU_IO_NOT_INSTALLED},
               {DROP_MODE, RINZU_IO_COPY_METADATA, ESTALE, RINZU_IO_NOT_INSTALLED},
               {FLUSH_FAILED, RINZU_IO_FLUSH, EIO, RINZU_IO_NOT_INSTALLED},
               {INSTALL_DENIED, RINZU_IO_INSTALL, EACCES, RINZU_IO_NOT_INSTALLED},
               {INSTALL_IO_ERROR, RINZU_IO_INSTALL, EIO, RINZU_IO_INDETERMINATE},
               {INSTALL_IO_ERROR_AFTER_RENAME, RINZU_IO_INSTALL, EIO, RINZU_IO_INDETERMINATE}};
  for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
    struct stat before, after;
    assert(!stat("destination", &before));
    struct WholeFile *file = prepared("destination");
    if (cases[i].fault == DENY_CHOWN)
      file->source_info.st_uid ^= 1; /* Force an ownership change without privileged setup. */
    whole_test_fault(cases[i].fault);
    rinzu_io_whole_install(file);
    expect_status(file, cases[i].phase, cases[i].status);
    assert(rinzu_io_whole_install_result(file) == (int32_t)cases[i].installed);
    finish(file);
    whole_test_fault(NONE);
    no_temporary();
    assert(!stat("destination", &after));
    if (cases[i].fault == INSTALL_IO_ERROR_AFTER_RENAME) {
      expect_contents("destination", "new value");
      assert(!same_inode(&before, &after));
    } else {
      expect_contents("destination", "old");
      assert(unchanged(&before, &after));
    }
  }
  struct WholeFile *file = begin_whole("destination", true, 0);
  whole_test_fault(WRITE_STALLED);
  rinzu_io_whole_write(file, (const uint8_t *)"new", 3);
  expect_status(file, RINZU_IO_WRITE, RINZU_IO_PROGRESS);
  finish(file);
  whole_test_fault(NONE);

  whole_test_fault(PROTECTED_TARGET);
  file = rinzu_io_whole_new("destination", 0, true);
  rinzu_io_whole_open(file);
  expect_status(file, RINZU_IO_INSPECT_DESTINATION, EPERM);
  finish(file);
  whole_test_fault(NONE);
  assert(!unlink("destination"));
}

static void wait_gate(void) {
  int64_t deadline = rinzu_io_monotonic() + 5000;
  while (!whole_test_entered()) {
    assert(rinzu_io_monotonic() < deadline);
    pause_once();
  }
}

static void race_destination(void) {
  for (int mode = CREATE_COLLISION; mode <= EXCLUSIVE_UNSUPPORTED; mode++) {
    struct WholeFile *file = prepared("destination");
    whole_test_fault(mode);
    rinzu_io_whole_install(file);
    expect_status(file, RINZU_IO_INSTALL, mode == CREATE_COLLISION ? EEXIST : ENOTSUP);
    assert(rinzu_io_whole_install_result(file) == RINZU_IO_NOT_INSTALLED);
    finish(file);
    whole_test_fault(NONE);
    if (mode == CREATE_COLLISION) {
      expect_contents("destination", "other");
      assert(!unlink("destination"));
    }
  }
  for (int changed_inode = 0; changed_inode < 2; changed_inode++) {
    seed_metadata();
    struct WholeFile *file = prepared("destination");
    whole_test_gate(BEFORE_INSTALL);
    rinzu_io_whole_install(file);
    wait_gate();
    if (changed_inode) {
      write_input("other", "other", 0600);
      assert(!rename("other", "destination"));
    } else {
      assert(!chmod("destination", 0600));
    }
    whole_test_gate(NO_GATE);
    expect_status(file, RINZU_IO_INSPECT_DESTINATION, ESTALE);
    finish(file);
    expect_contents("destination", changed_inode ? "other" : "old");
    assert(!unlink("destination"));
  }
  no_temporary();
}

static void cancellation(void) {
  for (int stage = WRITING; stage <= IN_CLOSE; stage++) {
    seed_metadata();
    struct WholeFile *file = begin_whole("destination", true, 0);
    if (stage == WRITING) {
      atomic_store(&partial, true);
      whole_test_gate(stage);
    }
    rinzu_io_whole_write(file, (const uint8_t *)"new value", 9);
    if (stage != WRITING) {
      expect_status(file, RINZU_IO_WRITE, 0);
      if (stage != IN_CLOSE)
        whole_test_gate(stage);
      rinzu_io_whole_install(file);
      if (stage == IN_CLOSE) {
        expect_status(file, RINZU_IO_INSTALL, 0);
        whole_test_gate(stage);
        assert(rinzu_io_whole_cleanup(file));
      }
    }
    wait_gate();
    if (stage == IN_RENAME)
      assert(file->installed == RINZU_IO_INSTALLING);
    if (stage == IN_CLOSE)
      assert(file->installed == RINZU_IO_INSTALLED);
    atomic_store_explicit(&file->completion.base.cancelled, 1, memory_order_release);
    assert(!whole_done(file) && rinzu_io_live_operations() == 1 && rinzu_io_live_files() == 3);
    whole_test_gate(NO_GATE);
    wait_done(file);
    if (stage < IN_RENAME) {
      assert(rinzu_io_whole_status(file) == ECANCELED && file->installed == RINZU_IO_NOT_INSTALLED);
      expect_contents("destination", "old");
    } else {
      assert(!rinzu_io_whole_status(file) && file->installed == RINZU_IO_INSTALLED);
      expect_contents("destination", "new value");
    }
    if (stage == IN_CLOSE) {
      assert(!rinzu_io_whole_cleanup(file));
      rinzu_io_whole_release(file);
      settled();
    } else {
      finish(file);
    }
    no_temporary();
  }
  atomic_store(&partial, false);
  for (int stage = OPENED; stage <= READING; stage++) {
    struct WholeFile *file = rinzu_io_whole_new("destination", 20, false);
    if (stage == OPENED)
      whole_test_gate(stage);
    rinzu_io_whole_open(file);
    if (stage == READING) {
      expect_status(file, RINZU_IO_OPEN_FILE, 0);
      whole_test_gate(stage);
      rinzu_io_whole_read(file);
    }
    wait_gate();
    assert(rinzu_io_whole_cancel_pending(file));
    whole_test_gate(NO_GATE);
    wait_done(file);
    assert(rinzu_io_live_files() == 1);
    finish(file);
  }
  assert(!unlink("destination"));
}

static void short_reads_and_cleanup_reservation(void) {
  write_input("input", "0123456", 0600);
  atomic_store(&partial, true);
  struct WholeFile *records[QUEUE];
  for (size_t i = 0; i < QUEUE; i++)
    records[i] = begin_whole("input", false, 7);
  struct WholeFile *rejected = rinzu_io_whole_new("input", 7, false);
  rinzu_io_whole_open(rejected);
  expect_status(rejected, RINZU_IO_SUBMIT, RINZU_IO_SATURATED);
  assert(!rinzu_io_whole_cleanup(rejected));
  rinzu_io_whole_release(rejected);
  struct WholeFile *file = records[0];
  for (int step = 0; step < 3; step++) {
    rinzu_io_whole_read(file);
    expect_status(file, RINZU_IO_READ, 0);
    assert(!rinzu_io_whole_eof(file) && !rinzu_io_whole_limit_exceeded(file));
  }
  int fd = open("input", O_WRONLY | O_APPEND);
  assert(fd >= 0 && write(fd, "7", 1) == 1 && !close(fd));
  rinzu_io_whole_read(file);
  expect_status(file, RINZU_IO_READ, 0);
  assert(rinzu_io_whole_limit_exceeded(file) && file->bytes.length == 7);
  for (size_t i = 0; i < QUEUE; i++)
    assert(rinzu_io_whole_cleanup(records[i]));
  for (size_t i = 0; i < QUEUE; i++) {
    wait_done(records[i]);
    assert(!rinzu_io_whole_cleanup_status(records[i], CLOSE_ORIGINAL));
    rinzu_io_whole_release(records[i]);
  }
  settled();
  atomic_store(&partial, false);
  assert(!unlink("input"));
}

int main(void) {
  check_metadata();
  metadata_and_install_failures();
  race_destination();
  cancellation();
  short_reads_and_cleanup_reservation();
  puts("whole-file-native-ok");
}
#endif

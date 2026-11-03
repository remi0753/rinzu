#include <assert.h>
#include <errno.h>
#include <poll.h>
#include <stdatomic.h>
#include <signal.h>
#include <fcntl.h>
#include <time.h>
#include <unistd.h>
static _Atomic int polls, interrupted;
static int observed_poll(struct pollfd *fds, nfds_t count, int timeout) {
  atomic_fetch_add(&polls, 1);
  if (atomic_exchange(&interrupted, 0)) {
    errno = EINTR;
    return -1;
  }
  return poll(fds, count, timeout);
}
#define poll observed_poll
#include "../native/src/io.c"
#undef poll
static void pause_once(void) {
  struct timespec interval = {0, 1000000};
  nanosleep(&interval, NULL);
}
static _Atomic int observer_started, observer_stop;
static void *signal_observer(void *unused) {
  (void)unused;
  sigset_t mask;
  sigemptyset(&mask);
  sigaddset(&mask, SIGPIPE);
  assert(!pthread_sigmask(SIG_UNBLOCK, &mask, NULL));
  atomic_store(&observer_started, 1);
  while (!atomic_load(&observer_stop))
    pause_once();
  return NULL;
}
static void ready(void *file) {
  int64_t deadline = rinzu_io_monotonic() + 5000;
  while (!rinzu_io_ready(file)) {
    assert(rinzu_io_monotonic() < deadline);
    pause_once();
  }
}
static void finish(void *file, int expected) {
  ready(file);
  assert(rinzu_io_status(file) == expected);
  rinzu_io_finish(file);
}
static void *standard(int stream) {
  void *file = rinzu_io_standard(stream);
  finish(file, 0);
  struct File *native = file;
  assert(native->fd != stream && (fcntl(native->fd, F_GETFD) & FD_CLOEXEC));
  return file;
}
static void closed(void *file) {
  assert(!rinzu_io_close(file));
  finish(file, 0);
  assert(rinzu_io_close(file) == RINZU_IO_CLOSED);
}
static void blocked(void *file) {
  int64_t deadline = rinzu_io_monotonic() + 5000;
  while (!atomic_load(&polls)) {
    assert(rinzu_io_monotonic() < deadline);
    pause_once();
  }
  assert(!rinzu_io_ready(file) && rinzu_io_close(file) == RINZU_IO_BUSY);
  int64_t before = rinzu_io_monotonic();
  rinzu_io_cancel(file);
  finish(file, ECANCELED);
  assert(rinzu_io_monotonic() - before < 1000);
}
int main(void) {
  int saved_in = dup(STDIN_FILENO), saved_out = dup(STDOUT_FILENO);
  assert(saved_in >= 0 && saved_out >= 0);
  int p[2];
  assert(!pipe(p) && dup2(p[0], STDIN_FILENO) == STDIN_FILENO);
  close(p[0]);
  int original_flags = fcntl(STDIN_FILENO, F_GETFL);
  void *input = standard(STDIN_FILENO);
  assert(fcntl(STDIN_FILENO, F_GETFL) == original_flags);
  atomic_store(&polls, 0);
  atomic_store(&interrupted, 1);
  assert(!rinzu_io_read(input, 7));
  blocked(input);
  const uint8_t data[] = {0, 255, 127, 128, 42, 13, 7};
  assert(write(p[1], data, 3) == 3);
  assert(!rinzu_io_read(input, 7));
  ready(input);
  assert(!rinzu_io_status(input) && rinzu_io_length(input) == 3);
  assert(!memcmp(rinzu_io_bytes(input), data, 3));
  rinzu_io_finish(input);
  close(p[1]);
  assert(!rinzu_io_read(input, 7));
  ready(input);
  assert(!rinzu_io_status(input) && !rinzu_io_length(input));
  rinzu_io_finish(input);
  closed(input);
  assert(fcntl(STDIN_FILENO, F_GETFL) == original_flags);

  assert(!pipe(p) && dup2(p[1], STDOUT_FILENO) == STDOUT_FILENO);
  close(p[1]);
  /* Deliberately inherited nonblocking flags must be left unchanged. */
  original_flags = fcntl(STDOUT_FILENO, F_GETFL) | O_NONBLOCK;
  assert(!fcntl(STDOUT_FILENO, F_SETFL, original_flags));
  void *output = standard(STDOUT_FILENO);
  assert(fcntl(STDOUT_FILENO, F_GETFL) == original_flags);
  uint8_t full[RINZU_IO_CHUNK] = {0};
  while (write(STDOUT_FILENO, full, sizeof(full)) >= 0) {
  }
  assert(errno == EAGAIN);
  int after_write_flags = fcntl(STDOUT_FILENO, F_GETFL);
  assert((after_write_flags & (O_ACCMODE | O_NONBLOCK | O_APPEND | O_ASYNC)) == original_flags);
  atomic_store(&polls, 0);
  assert(!rinzu_io_write_all(output, full, sizeof(full)));
  blocked(output);
  assert(!rinzu_io_live_buffers());
  close(p[0]);
  struct sigaction before, after;
  assert(!sigaction(SIGPIPE, NULL, &before));
  assert(!rinzu_io_write(output, data, sizeof(data)));
  finish(output, EPIPE);
  assert(!sigaction(SIGPIPE, NULL, &after) && before.sa_handler == after.sa_handler);
  closed(output);
  assert(fcntl(STDOUT_FILENO, F_GETFL) == after_write_flags);

  /* A pending SIGPIPE from another operation must not be consumed. */
  sigset_t mask, previous, pending;
  sigemptyset(&mask);
  sigaddset(&mask, SIGPIPE);
  assert(!pthread_sigmask(SIG_BLOCK, &mask, &previous));
  assert(!pthread_kill(pthread_self(), SIGPIPE));
  assert(rinzu_posix_write(STDOUT_FILENO, data, 1) == -1 && errno == EPIPE);
  assert(!sigpending(&pending) && sigismember(&pending, SIGPIPE));
  int number;
  assert(!sigwait(&mask, &number) && number == SIGPIPE);
  assert(!pthread_sigmask(SIG_SETMASK, &previous, NULL));
  /* Exercise actual EPIPE generation, not only the poll error path. */
  pthread_t observer;
  assert(!pthread_create(&observer, NULL, signal_observer, NULL));
  while (!atomic_load(&observer_started))
    pause_once();
  int nosigpipe = fcntl(STDOUT_FILENO, F_GETNOSIGPIPE);
  assert(rinzu_posix_write(STDOUT_FILENO, data, 1) == -1 && errno == EPIPE);
  assert(fcntl(STDOUT_FILENO, F_GETNOSIGPIPE) == nosigpipe);
  assert(!fcntl(STDOUT_FILENO, F_SETNOSIGPIPE, 1));
  assert(rinzu_posix_write(STDOUT_FILENO, data, 1) == -1 && errno == EPIPE);
  assert(fcntl(STDOUT_FILENO, F_GETNOSIGPIPE) == 1);
  assert(!fcntl(STDOUT_FILENO, F_SETNOSIGPIPE, nosigpipe));
  atomic_store(&observer_stop, 1);
  assert(!pthread_join(observer, NULL));
  assert(!sigpending(&pending) && !sigismember(&pending, SIGPIPE));

  void *invalid = rinzu_io_standard(2);
  finish(invalid, RINZU_IO_ARGUMENT);
  assert(!rinzu_io_live_files() && !rinzu_io_live_buffers() && !operations && !running);
  assert(dup2(saved_in, STDIN_FILENO) == STDIN_FILENO);
  assert(dup2(saved_out, STDOUT_FILENO) == STDOUT_FILENO);
  close(saved_in);
  close(saved_out);
  puts("io-standard-ok");
}

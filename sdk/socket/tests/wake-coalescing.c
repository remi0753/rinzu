/* A queued EVFILT_USER event covers all work published before its receipt.
   Exercise the actual kernel event and the return-to-lock race boundary. */
#include "../native/src/socket.c"
#include <assert.h>
#include <stdio.h>

static struct kevent receive_user(void) {
  struct kevent event;
  struct timespec timeout = {.tv_sec = 1};
  int count = kevent(queue_fd, NULL, 0, &event, 1, &timeout);
  assert(count == 1 && event.filter == EVFILT_USER && event.ident == 1);
  return event;
}
static void generations(void) {
  for (unsigned turn = 0; turn < 64; ++turn) {
    pthread_mutex_lock(&lock);
    size_t before = user_wakes;
    /* The old stopped queue may have had an unconsumed stop trigger. */
    wake_pending = true;
    start();
    assert(!wake_pending);
    stopping = true;
    wake();
    assert(wake_pending && user_wakes == before + 1);
    pthread_mutex_unlock(&lock);
    assert(!pthread_join(worker, NULL));
    pthread_mutex_lock(&lock);
    assert(!close(queue_fd));
    queue_fd = -1;
    running = stopping = false;
    pthread_mutex_unlock(&lock);
  }
}
int main(void) {
  alarm(20);
  queue_fd = kqueue();
  assert(queue_fd >= 0);
  struct kevent event;
  EV_SET(&event, 1, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, NULL);
  assert(!kevent(queue_fd, &event, 1, NULL, 0, NULL));
  for (unsigned turn = 0; turn < 256; ++turn) {
    pthread_mutex_lock(&lock);
    size_t before = user_wakes, calls = kevent_calls;
    assert(!wake_pending);
    wake();
    for (unsigned duplicate = 0; duplicate < 64; ++duplicate)
      wake();
    assert(wake_pending && user_wakes == before + 1 && kevent_calls == calls + 1);
    pthread_mutex_unlock(&lock);
    event = receive_user();
    pthread_mutex_lock(&lock);
    /* The kernel returned the event, but the consumer has not observed it.
       This producer's state belongs to the following locked advance pass. */
    wake();
    assert(wake_pending && user_wakes == before + 1);
    struct kevent other = {.filter = EVFILT_USER, .ident = 77};
    observe_event(&other);
    assert(wake_pending);
    other = (struct kevent){.filter = EVFILT_READ, .ident = 999, .udata = NULL};
    observe_event(&other);
    assert(wake_pending);
    observe_event(&event);
    assert(!wake_pending);
    /* A producer after receipt needs another real event, even with the old
       operation table unchanged. This also covers cancel/close/stop requests. */
    wake();
    assert(wake_pending && user_wakes == before + 2);
    pthread_mutex_unlock(&lock);
    event = receive_user();
    pthread_mutex_lock(&lock);
    observe_event(&event);
    assert(!wake_pending);
    pthread_mutex_unlock(&lock);
  }
  assert(!close(queue_fd));
  queue_fd = -1;
  generations();
  assert(!handle_count && !operation_count && !pending_count && !running);
  puts("N08:wake:queued:returned-before-lock:foreign-event:after-receipt:restart64:OK");
}

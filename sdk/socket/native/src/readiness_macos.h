static int filter(enum Kind kind) {
  return kind == READ || kind == ACCEPT ? EVFILT_READ : EVFILT_WRITE;
}
static void unregister(struct Operation *op) {
  if (!op->registered)
    return;
  if (!op->handle)
    fatal();
  struct kevent event;
  EV_SET(&event, op->handle->fd, filter(op->kind), EV_DELETE, 0, 0, NULL);
  ++kevent_calls;
  if (kevent(queue_fd, &event, 1, NULL, 0, NULL) < 0 && errno != ENOENT && errno != EBADF)
    fatal();
  op->registered = false;
}
static void arm(struct Operation *op) {
  struct kevent event;
  EV_SET(&event, op->handle->fd, filter(op->kind), EV_ADD | EV_ENABLE | EV_ONESHOT, 0, 0,
         (void *)op->id);
  ++kevent_calls;
  if (kevent(queue_fd, &event, 1, NULL, 0, NULL) < 0)
    publish(op, failure());
  else
    op->registered = true;
}
/* Validate immutable operation identity before touching a reused slot/descriptor. */
static void observe_event(const struct kevent *event) {
  if (event->filter == EVFILT_USER) {
    /* The SDK lock orders receipt against every producer. Work enqueued
       before this receipt is visible to the next advance pass; a producer
       after it must publish a fresh notification. Other user IDs are not ours. */
    if (event->ident == 1)
      wake_pending = false;
    return;
  }
  ++event_count;
  uintptr_t id = (uintptr_t)event->udata;
  struct Operation *found = lookup_operation(id);
  if (!found || found->ready || found->handle->fd != (int)event->ident ||
      filter(found->kind) != event->filter) {
    ++stale_events;
    return;
  }
  found->registered = false;
  found->attempt = true;
  if (event->flags & EV_ERROR)
    publish(found, (int32_t)event->data);
}

static inline void readiness_waited(void) {
  ++kevent_calls;
}
static int readiness_wait(ReadinessEvent *events, int capacity, int milliseconds) {
  struct timespec timeout = {milliseconds / 1000, milliseconds % 1000 * 1000000};
  return kevent(queue_fd, NULL, 0, events, capacity, milliseconds < 0 ? NULL : &timeout);
}
static void readiness_signal(void) {
  struct kevent event;
  EV_SET(&event, 1, EVFILT_USER, 0, NOTE_TRIGGER, 0, NULL);
  ++kevent_calls;
  if (kevent(queue_fd, &event, 1, NULL, 0, NULL) < 0)
    fatal();
}
static void readiness_open(void) {
  queue_fd = kqueue();
  if (queue_fd < 0)
    fatal();
  if (fcntl(queue_fd, F_SETFD, FD_CLOEXEC) < 0)
    fatal();
  struct kevent event;
  EV_SET(&event, 1, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, NULL);
  ++kevent_calls;
  if (kevent(queue_fd, &event, 1, NULL, 0, NULL) < 0)
    fatal();
}

static void readiness_close(void) {
  (void)close(queue_fd);
  queue_fd = -1;
}

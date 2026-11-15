/* The socket lock protects direction IDs and epoll registration generations. */
static int wake_fd = -1;

static bool read_direction(enum Kind kind) {
  return kind == READ || kind == ACCEPT;
}

static int change_interest(struct Handle *handle, uintptr_t read, uintptr_t write) {
  if (!read && !write) {
    if (handle->readiness && epoll_ctl(queue_fd, EPOLL_CTL_DEL, handle->fd, NULL) &&
        errno != ENOENT && errno != EBADF)
      return -1;
    handle->readiness = 0;
  } else {
    uintptr_t generation = identity((size_t)(handle - handles));
    ReadinessEvent event = {
        .events = EPOLLONESHOT | (read ? EPOLLIN | EPOLLRDHUP : 0) | (write ? EPOLLOUT : 0),
        .data.u64 = generation,
    };
    int operation = handle->readiness ? EPOLL_CTL_MOD : EPOLL_CTL_ADD;
    if (epoll_ctl(queue_fd, operation, handle->fd, &event))
      return -1;
    handle->readiness = generation;
  }
  handle->ready_read = read;
  handle->ready_write = write;
  return 0;
}

static void unregister(struct Operation *op) {
  if (!op->registered)
    return;
  struct Handle *handle = op->handle;
  bool reading = read_direction(op->kind);
  if (!handle || (reading ? handle->ready_read : handle->ready_write) != op->id ||
      change_interest(handle, reading ? 0 : handle->ready_read, reading ? handle->ready_write : 0))
    fatal();
  op->registered = false;
}

static void arm(struct Operation *op) {
  struct Handle *handle = op->handle;
  bool reading = read_direction(op->kind);
  uintptr_t current = reading ? handle->ready_read : handle->ready_write;
  if (current && current != op->id)
    fatal();
  if (change_interest(handle, reading ? op->id : handle->ready_read,
                      reading ? handle->ready_write : op->id))
    publish(op, failure());
  else
    op->registered = true;
}

static bool mark_ready(struct Handle *handle, uintptr_t id) {
  if (!id)
    return false;
  struct Operation *op = lookup_operation(id);
  if (!op || op->ready || !op->registered || op->handle != handle) {
    ++stale_events;
    return true;
  }
  op->registered = false;
  op->attempt = true;
  return true;
}

static void observe_event(const ReadinessEvent *event) {
  uintptr_t generation = event->data.u64;
  if (!generation) {
    uint64_t value;
    ssize_t count;
    do {
      count = read(wake_fd, &value, sizeof value);
    } while (count < 0 && errno == EINTR);
    if (count != sizeof value)
      fatal();
    /* The same lock orders eventfd consumption and latch reset against wake. */
    wake_pending = false;
    return;
  }
  ++event_count;
  size_t slot = generation & TOKEN_SLOT_MASK;
  if (!slot || slot > HANDLES || handles[slot - 1].readiness != generation) {
    ++stale_events;
    return;
  }
  struct Handle *handle = &handles[slot - 1];
  uintptr_t read = handle->ready_read, write = handle->ready_write;
  uint32_t events = event->events;
  if ((events & (EPOLLIN | EPOLLRDHUP | EPOLLERR | EPOLLHUP)) && mark_ready(handle, read))
    read = 0;
  if ((events & (EPOLLOUT | EPOLLERR | EPOLLHUP)) && mark_ready(handle, write))
    write = 0;
  /* EPOLLONESHOT disables both directions, including any still pending. */
  if (change_interest(handle, read, write))
    fatal();
}

static inline void readiness_waited(void) {}

static int readiness_wait(ReadinessEvent *events, int capacity, int milliseconds) {
  return epoll_wait(queue_fd, events, capacity, milliseconds);
}

static void readiness_signal(void) {
  uint64_t value = 1;
  ssize_t count;
  do {
    count = write(wake_fd, &value, sizeof value);
  } while (count < 0 && errno == EINTR);
  if (count != sizeof value)
    fatal();
}

static void readiness_open(void) {
  queue_fd = epoll_create1(EPOLL_CLOEXEC);
  wake_fd = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);
  ReadinessEvent event = {.events = EPOLLIN, .data.u64 = 0};
  if (queue_fd < 0 || wake_fd < 0 || epoll_ctl(queue_fd, EPOLL_CTL_ADD, wake_fd, &event))
    fatal();
}

static void readiness_close(void) {
  (void)close(wake_fd);
  (void)close(queue_fd);
  queue_fd = wake_fd = -1;
}

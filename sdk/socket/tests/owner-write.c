/* One Owner send must never invalidate an unlocked worker's FD/buffer lease. */
#include <sys/socket.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stddef.h>
#include <errno.h>
static _Thread_local unsigned owner_calls, owner_thread, disable_inline, expire_inline, cancel_inline;
static _Thread_local int send_error, send_zero;
static _Thread_local size_t send_limit;
static ssize_t counted_send(int fd, const void *p, size_t n, int flags) {
  if (owner_thread) {
    ++owner_calls;
    if (send_error) { errno = send_error; send_error = 0; return -1; }
    if (send_zero) { send_zero = 0; return 0; }
    if (send_limit && n > send_limit) n = send_limit;
  }
  return send(fd, p, n, flags);
}
static void io_stage(void *, int);
static int allow_inline(void *);
#define send counted_send
#define RINZU_SOCKET_DATA_IO_STAGE(operation, stage) io_stage(operation, stage)
#define RINZU_SOCKET_OWNER_WRITE_ALLOWED() allow_inline(op)
#define RINZU_SOCKET_OWNER_READ_ALLOWED() allow_inline(op)
#include "../native/src/socket.c"
#undef send
#include "rinzu/source_async.h"
#include "accounting-check.h"
#include <assert.h>
#include <stdio.h>

static pthread_mutex_t probe_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t probe_changed = PTHREAD_COND_INITIALIZER;
static int pause_kind = -1, pause_stage, parked, resumed;
static int allow_inline(void *raw) {
  struct Operation *op = raw;
  if (disable_inline) return 0;
  if (expire_inline) { op->deadline = monotonic()-1; expire_inline = 0; }
  if (cancel_inline) { atomic_store(&op->completion.base.cancelled, true); cancel_inline = 0; }
  return 1;
}
static void io_stage(void *raw, int stage) {
  struct Operation *op = raw;
  assert(!pthread_mutex_lock(&probe_lock));
  if ((int)op->kind == pause_kind && stage == pause_stage && !parked) {
    parked = 1;
    assert(!pthread_cond_broadcast(&probe_changed));
    while (!resumed) assert(!pthread_cond_wait(&probe_changed, &probe_lock));
  }
  assert(!pthread_mutex_unlock(&probe_lock));
}
static void tick(void) { struct timespec delay = {0,1000000}; nanosleep(&delay, NULL); }
static void wait_ready(void *token) {
  rinzu_native_completion_t *c = rinzu_socket_completion(token);
  for (unsigned i=0; !rinzu_native_completion_is_done(c); ++i) { assert(i<10000); tick(); }
}
static void *admitted(int64_t id) { assert(id>0); return (void *)(uintptr_t)id; }
static void finish(void *op) { rinzu_socket_ack(op); rinzu_socket_finish(op); check_accounting(); }
static void *paired(int *peer) {
  int pair[2]; assert(!socketpair(AF_UNIX, SOCK_STREAM, 0, pair));
  pthread_mutex_lock(&lock);
  struct Handle *h = reserve_handle(); assert(h && !configure(pair[0])); h->fd=pair[0];
  void *token=(void *)h->id; pthread_mutex_unlock(&lock); *peer=pair[1]; return token;
}
static void close_stream(void *h) {
  void *op=admitted(rinzu_socket_close(h)); wait_ready(op); finish(op);
}
static void clean(void) {
  for (unsigned i=0; !rinzu_socket_join_idle(); ++i) { assert(i<10000); tick(); }
  check_accounting();
  assert(!handle_count && !operation_count && !pending_count && !buffer_bytes && !running && !worker_data_io);
}
static void receive(int peer, const uint8_t *expected, size_t size) {
  uint8_t bytes[65536]; size_t used=0;
  while (used<size) { ssize_t n=read(peer,bytes+used,size-used); assert(n>0); used+=(size_t)n; }
  assert(!memcmp(bytes,expected,size));
}
static void writes(size_t size, bool all, size_t limit, int error) {
  int peer; void *h=paired(&peer); uint8_t input[65536], expected[65536];
  for (size_t i=0;i<sizeof input;++i) input[i]=expected[i]=(uint8_t)i;
  unsigned before=owner_calls; send_limit=limit; send_error=error;
  void *op=admitted(rinzu_socket_write(h,input,size,all,0)); send_limit=0; send_error=0;
  assert(owner_calls==before+!!size);
  memset(input,0x71,sizeof input); /* All future SDK sends must use its snapshot. */
  size_t sent=(!all && limit && limit<size && !error) ? limit : size;
  receive(peer,expected,sent); wait_ready(op);
  assert(!rinzu_socket_status(op) && (size_t)rinzu_socket_length(op)==sent);
  assert(rinzu_socket_write(h,input,1,true,0)==RINZU_SOCKET_BUSY);
  assert(rinzu_socket_shutdown(h)==RINZU_SOCKET_BUSY);
  /* Physical completion before registration still supplies the packed v3 receipt. */
  rinzu_native_completion_t *c=rinzu_socket_transfer_completion((int64_t)(uintptr_t)op);
  assert(c->version==3 && rinzu_native_completion_transfer_result(c)==(int64_t)sent);
  rinzu_async_runtime_t runtime; rinzu_async_init(&runtime); runtime.native_notifications=1;
  rinzu_async_future_t *future=rinzu_async_new(&runtime,RINZU_ASYNC_COMPLETION,RINZU_TRANSFER_UNIT,(rinzu_source_position_t){0});
  future->native_completion=c; rinzu_async_start(future);
  assert(rinzu_async_try_collect(future) && future->settled && !future->failure.primary);
  finish(op); rinzu_async_dispose(&runtime); close_stream(h); close(peer); clean();
}
static void failed_write(int error, bool zero, bool expired, bool cancelled) {
  int peer; void *h=paired(&peer); uint8_t byte=7; unsigned before=owner_calls;
  send_error=error; send_zero=zero; expire_inline=expired; cancel_inline=cancelled;
  void *op=admitted(rinzu_socket_write(h,&byte,1,true,0));
  assert(owner_calls==before+!(expired||cancelled));
  assert(rinzu_native_completion_is_done(rinzu_socket_completion(op)));
  assert(rinzu_socket_status(op)==(expired ? -7509 : cancelled ? ECANCELED : zero ? -7508 : error));
  send_error=send_zero=0; finish(op); close(peer); clean();
}
static void paused_worker(int kind, int stage, int action) {
  int peer, other_peer; void *h=paired(&peer), *other=paired(&other_peer);
  uint8_t bytes[]={0,0x80,0xff,7};
  if (kind==READ) assert(write(peer,bytes,sizeof bytes)==sizeof bytes);
  assert(!pthread_mutex_lock(&probe_lock)); pause_kind=kind; pause_stage=stage; parked=resumed=0; assert(!pthread_mutex_unlock(&probe_lock));
  disable_inline=1;
  void *leased=admitted(kind==READ ? rinzu_socket_read(h,4,1000) : rinzu_socket_write(h,bytes,4,true,1000));
  disable_inline=0;
  assert(!pthread_mutex_lock(&probe_lock)); while(!parked) assert(!pthread_cond_wait(&probe_changed,&probe_lock)); assert(!pthread_mutex_unlock(&probe_lock));
  pthread_mutex_lock(&lock);
  struct Operation *op=owned(leased); struct Handle *handle=op->handle;
  int fd=handle->fd; uintptr_t id=handle->id; uint8_t *buffer=op->buffer;
  assert(worker_data_io && !op->ready); pthread_mutex_unlock(&lock);
  void *target=kind==READ ? h : other; unsigned before=owner_calls;
  send_error=EPIPE; /* A forbidden Owner send would close the opposite leased fd. */
  void *queued=admitted(rinzu_socket_write(target,bytes,4,true,1000)); send_error=0;
  assert(owner_calls==before);
  if (action==1) rinzu_socket_cancel(queued);
  void *closing=action==2 ? admitted(rinzu_socket_close(h)) : NULL;
  pthread_mutex_lock(&lock);
  assert(worker_data_io && !op->ready && op->buffer==buffer && handle->id==id && handle->fd==fd);
  assert(!owned(queued)->ready); pthread_mutex_unlock(&lock);
  assert(!pthread_mutex_lock(&probe_lock)); resumed=1; assert(!pthread_cond_broadcast(&probe_changed)); assert(!pthread_mutex_unlock(&probe_lock));
  wait_ready(leased); wait_ready(queued);
  assert(!rinzu_socket_status(leased) && rinzu_socket_length(leased)==4);
  if (kind==WRITE) receive(peer,bytes,4);
  int status=rinzu_socket_status(queued);
  assert(status==((action==1 || (action==2 && kind==READ)) ? ECANCELED : 0));
  if (!status) receive(kind==READ ? peer : other_peer,bytes,4);
  finish(leased); finish(queued);
  if (closing) { wait_ready(closing); finish(closing); }
  if (!closing && !(action==1 && kind==READ)) close_stream(h);
  if (!(action==1 && kind==WRITE)) close_stream(other);
  close(peer); close(other_peer);
  assert(!pthread_mutex_lock(&probe_lock)); pause_kind=-1; assert(!pthread_mutex_unlock(&probe_lock)); clean();
}
static void notify_opposite_read(void) {
  int peer; void *h=paired(&peer); uint8_t byte=9;
  void *pending=admitted(rinzu_socket_read(h,4,1000));
  for (unsigned i=0;;++i) {
    pthread_mutex_lock(&lock); bool armed=owned(pending)->registered && !worker_data_io; pthread_mutex_unlock(&lock);
    if (armed) break; assert(i<10000); tick();
  }
  rinzu_async_runtime_t runtime; rinzu_async_init(&runtime); runtime.native_notifications=1;
  rinzu_async_future_t *future=rinzu_async_new(&runtime,RINZU_ASYNC_COMPLETION,RINZU_TRANSFER_UNIT,(rinzu_source_position_t){0});
  future->native_completion=rinzu_socket_completion(pending); rinzu_async_start(future);
  assert(runtime.native_wake && atomic_load(&runtime.native_wake->references)==2);
  uint64_t epoch=runtime.completion_epoch; unsigned before=owner_calls; send_error=EPIPE;
  void *failed=admitted(rinzu_socket_write(h,&byte,1,true,0));
  assert(owner_calls==before+1 && rinzu_socket_status(failed)==EPIPE);
  assert(rinzu_socket_status(pending)==ECANCELED && runtime.completion_epoch>epoch);
  assert(atomic_load(&runtime.native_wake->references)==1);
  assert(rinzu_async_try_collect(future) && future->settled && !future->failure.primary);
  finish(failed); finish(pending); rinzu_async_dispose(&runtime); close(peer); clean();
}
int main(void) {
  alarm(60); owner_thread=1;
  const size_t sizes[]={0,1,127,128,511,512,1023,1024,1025,2048,4096,65536};
  for (size_t i=0;i<sizeof sizes/sizeof *sizes;++i) writes(sizes[i],true,0,0);
  writes(65536,true,127,0); writes(4096,false,127,0);
  writes(4096,true,0,EAGAIN); writes(4096,true,0,EINTR);
  failed_write(EPIPE,false,false,false); failed_write(0,true,false,false);
  failed_write(0,false,true,false); failed_write(0,false,false,true);
  for(int kind=READ;kind<=WRITE;++kind) for(int stage=0;stage<2;++stage) for(int action=0;action<3;++action) paused_worker(kind,stage,action);
  notify_opposite_read();
  puts("owner-write:singlecall;snapshot;partial;transient;deadline;cancel;v3;notify;worker-lease12;cleanup=0");
}

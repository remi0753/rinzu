/* One Owner recv must never invalidate an unlocked worker's FD/buffer lease. */
#include <sys/socket.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stddef.h>
#include <errno.h>
static _Thread_local unsigned owner_calls, owner_thread, disable_inline, expire_inline, cancel_inline;
static _Thread_local int recv_error, recv_zero;
static _Thread_local size_t recv_limit;
static ssize_t counted_recv(int fd, void *p, size_t n, int flags) {
  if (owner_thread) {
    ++owner_calls;
    if (recv_error) { errno = recv_error; recv_error = 0; return -1; }
    if (recv_zero) { recv_zero = 0; return 0; }
    if (recv_limit && n > recv_limit) n = recv_limit;
  }
  return recv(fd, p, n, flags);
}
static void io_stage(void *, int);
static int allow_inline(void *);
#define recv counted_recv
#define RINZU_SOCKET_DATA_IO_STAGE(operation, stage) io_stage(operation, stage)
#define RINZU_SOCKET_OWNER_WRITE_ALLOWED() allow_inline(op)
#define RINZU_SOCKET_OWNER_READ_ALLOWED() allow_inline(op)
#include "../native/src/socket.c"
#undef recv
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
static void reads(size_t size, size_t limit, int error) {
  int peer; void *h=paired(&peer); uint8_t input[65536];
  for(size_t i=0;i<sizeof input;++i)input[i]=(uint8_t)i;
  /* Queue the whole CHUNK without blocking this same-thread test before READ. */
  int capacity=262144;assert(!setsockopt(peer,SOL_SOCKET,SO_SNDBUF,&capacity,sizeof capacity));
  pthread_mutex_lock(&lock);struct Handle *stream=handle(h);assert(stream);
  assert(!setsockopt(stream->fd,SOL_SOCKET,SO_RCVBUF,&capacity,sizeof capacity));pthread_mutex_unlock(&lock);
  assert(send(peer,input,size,MSG_DONTWAIT)==(ssize_t)size);
  unsigned before=owner_calls;recv_limit=limit;recv_error=error;
  void *op=admitted(rinzu_socket_read(h,(int64_t)size,0));recv_limit=0;recv_error=0;
  assert(owner_calls==before+1);wait_ready(op);
  size_t received=limit&&limit<size&&!error?limit:size;
  assert(!rinzu_socket_status(op)&&(size_t)rinzu_socket_length(op)==received);
  const rinzu_native_byte_array_t *view=rinzu_socket_bytes_view(op);
  assert(view->length==received&&!memcmp(view->data,input,received));
  assert(rinzu_socket_read(h,1,0)==RINZU_SOCKET_BUSY);
  rinzu_native_completion_t *c=rinzu_socket_transfer_completion((int64_t)(uintptr_t)op);
  assert(c->version==3&&rinzu_native_completion_transfer_result(c)==(int64_t)received);
  rinzu_async_runtime_t runtime;rinzu_async_init(&runtime);runtime.native_notifications=1;
  rinzu_async_future_t *future=rinzu_async_new(&runtime,RINZU_ASYNC_COMPLETION,RINZU_TRANSFER_UNIT,(rinzu_source_position_t){0});
  future->native_completion=c;rinzu_async_start(future);
  assert(rinzu_async_try_collect(future)&&future->settled&&!future->failure.primary);
  finish(op);rinzu_async_dispose(&runtime);close_stream(h);close(peer);clean();
}
static void eof(void) {
  int peer;void *h=paired(&peer);assert(!shutdown(peer,SHUT_WR));
  for(unsigned round=0;round<2;++round){
    unsigned before=owner_calls;void *op=admitted(rinzu_socket_read(h,4,0));
    assert(owner_calls==before+!round);
    assert(rinzu_native_completion_is_done(rinzu_socket_completion(op)));
    assert(!rinzu_socket_status(op)&&!rinzu_socket_length(op));
    assert(!rinzu_socket_bytes_view(op)->length);finish(op);
  }
  close_stream(h);close(peer);clean();
}
static void failed_read(int error,bool expired,bool cancelled) {
  int peer;void *h=paired(&peer);unsigned before=owner_calls;
  recv_error=error;expire_inline=expired;cancel_inline=cancelled;
  void *op=admitted(rinzu_socket_read(h,4,0));
  assert(owner_calls==before+!(expired||cancelled));
  assert(rinzu_native_completion_is_done(rinzu_socket_completion(op)));
  assert(rinzu_socket_status(op)==(expired?-7509:cancelled?ECANCELED:error));
  recv_error=0;finish(op);close(peer);clean();
}
static void paused_worker(int kind,int stage,int action) {
  int peer,other_peer;void *h=paired(&peer),*other=paired(&other_peer);
  uint8_t bytes[]={0,0x80,0xff,7};
  if(kind==READ)assert(write(peer,bytes,sizeof bytes)==sizeof bytes);
  assert(!pthread_mutex_lock(&probe_lock));pause_kind=kind;pause_stage=stage;parked=resumed=0;assert(!pthread_mutex_unlock(&probe_lock));
  disable_inline=1;
  void *leased=admitted(kind==READ?rinzu_socket_read(h,4,1000):rinzu_socket_write(h,bytes,4,true,1000));
  disable_inline=0;
  assert(!pthread_mutex_lock(&probe_lock));while(!parked)assert(!pthread_cond_wait(&probe_changed,&probe_lock));assert(!pthread_mutex_unlock(&probe_lock));
  pthread_mutex_lock(&lock);
  struct Operation *op=owned(leased);struct Handle *handle=op->handle;
  int fd=handle->fd;uintptr_t id=handle->id;uint8_t *buffer=op->buffer;
  assert(worker_data_io&&!op->ready);pthread_mutex_unlock(&lock);
  if(kind==READ)assert(rinzu_socket_read(h,4,1000)==RINZU_SOCKET_BUSY);
  void *target=kind==WRITE?h:other;
  assert(write(kind==WRITE?peer:other_peer,bytes,sizeof bytes)==sizeof bytes);
  unsigned before=owner_calls;recv_error=ECONNRESET;
  void *queued=admitted(rinzu_socket_read(target,4,1000));recv_error=0;
  assert(owner_calls==before); /* An erroneous Owner recv would close the leased fd. */
  if(action==1)rinzu_socket_cancel(queued);
  void *closing=action==2?admitted(rinzu_socket_close(h)):NULL;
  pthread_mutex_lock(&lock);
  assert(worker_data_io&&!op->ready&&op->buffer==buffer&&handle->id==id&&handle->fd==fd);
  assert(!owned(queued)->ready);pthread_mutex_unlock(&lock);
  assert(!pthread_mutex_lock(&probe_lock));resumed=1;assert(!pthread_cond_broadcast(&probe_changed));assert(!pthread_mutex_unlock(&probe_lock));
  wait_ready(leased);wait_ready(queued);
  assert(!rinzu_socket_status(leased)&&rinzu_socket_length(leased)==4);
  if(kind==WRITE)receive(peer,bytes,4);
  int status=rinzu_socket_status(queued);
  assert(status==((action==1||(action==2&&kind==WRITE))?ECANCELED:0));
  if(!status){const rinzu_native_byte_array_t *view=rinzu_socket_bytes_view(queued);assert(view->length==4&&!memcmp(view->data,bytes,4));}
  finish(leased);finish(queued);
  if(closing){wait_ready(closing);finish(closing);}
  if(!closing&&!(action==1&&kind==WRITE))close_stream(h);
  if(!(action==1&&kind==READ))close_stream(other);
  close(peer);close(other_peer);
  assert(!pthread_mutex_lock(&probe_lock));pause_kind=-1;assert(!pthread_mutex_unlock(&probe_lock));clean();
}
static void notify_opposite_write(void) {
  int peer;void *h=paired(&peer);uint8_t bytes[4096];memset(bytes,0x55,sizeof bytes);
  pthread_mutex_lock(&lock);struct Handle *stream=handle(h);assert(stream);
  int capacity=4096;assert(!setsockopt(stream->fd,SOL_SOCKET,SO_SNDBUF,&capacity,sizeof capacity));
  for(;;){ssize_t n=send(stream->fd,bytes,sizeof bytes,0);if(n<0){assert(errno==EAGAIN||errno==EWOULDBLOCK);break;}}
  pthread_mutex_unlock(&lock);
  void *pending=admitted(rinzu_socket_write(h,bytes,sizeof bytes,true,1000));
  for(unsigned i=0;;++i){pthread_mutex_lock(&lock);bool armed=owned(pending)->registered&&!worker_data_io;pthread_mutex_unlock(&lock);if(armed)break;assert(i<10000);tick();}
  rinzu_async_runtime_t runtime;rinzu_async_init(&runtime);runtime.native_notifications=1;
  rinzu_async_future_t *future=rinzu_async_new(&runtime,RINZU_ASYNC_COMPLETION,RINZU_TRANSFER_UNIT,(rinzu_source_position_t){0});
  future->native_completion=rinzu_socket_completion(pending);rinzu_async_start(future);
  assert(runtime.native_wake&&atomic_load(&runtime.native_wake->references)==2);
  uint64_t epoch=runtime.completion_epoch;unsigned before=owner_calls;recv_error=ECONNRESET;
  void *failed=admitted(rinzu_socket_read(h,4,0));
  assert(owner_calls==before+1&&rinzu_socket_status(failed)==ECONNRESET);
  assert(rinzu_socket_status(pending)==ECANCELED&&runtime.completion_epoch>epoch);
  assert(atomic_load(&runtime.native_wake->references)==1);
  assert(rinzu_async_try_collect(future)&&future->settled&&!future->failure.primary);
  finish(failed);finish(pending);rinzu_async_dispose(&runtime);close(peer);clean();
}
int main(void) {
  alarm(60);owner_thread=1;
  const size_t sizes[]={1,127,128,511,512,1023,1024,1025,2048,4096,65536};
  for(size_t i=0;i<sizeof sizes/sizeof *sizes;++i)reads(sizes[i],0,0);
  reads(4096,127,0);reads(4096,0,EAGAIN);reads(4096,0,EINTR);eof();
  failed_read(ECONNRESET,false,false);failed_read(0,true,false);failed_read(0,false,true);
  for(int kind=READ;kind<=WRITE;++kind)for(int stage=0;stage<2;++stage)for(int action=0;action<3;++action)paused_worker(kind,stage,action);
  notify_opposite_write();
  puts("owner-read:singlecall;binary;partial;transient;eof;deadline;cancel;v3;notify;worker-lease12;cleanup=0");
}

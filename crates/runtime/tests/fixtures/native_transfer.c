#include "rinzu/source_async.h"
#include <assert.h>
#include <sched.h>
#include <stdio.h>

typedef struct {
  rinzu_native_completion_v3_t *carrier;
  int64_t value;
} transfer_publication_t;
static void *publish_transfer(void *raw) {
  transfer_publication_t *p = raw;
  p->carrier->result = p->value;
  rinzu_native_completion_v2_publish(&p->carrier->completion);
  return NULL;
}
static rinzu_async_future_t *wait_transfer(rinzu_async_runtime_t *runtime,
                                         rinzu_native_completion_v3_t *carrier,
                                         unsigned kind) {
  rinzu_async_future_t *future = rinzu_async_new(runtime, RINZU_ASYNC_COMPLETION,
      kind, (rinzu_source_position_t){0});
  future->native_completion = &carrier->completion.base;
  rinzu_async_start(future);
  return future;
}
static void receive_transfer(rinzu_async_future_t *future) {
  while (!future->settled) {
    uint64_t epoch = rinzu_async_completion_epoch(future->runtime);
    if (!rinzu_async_try_collect(future))
      rinzu_async_wait_progress(future->runtime, epoch);
  }
}
int main(void) {
  assert(sizeof(rinzu_native_completion_t)==16);
  assert(sizeof(rinzu_native_completion_v2_t)==24);
  assert(sizeof(rinzu_native_completion_v3_t)==32);
  const int64_t values[]={INT64_MIN,-7509*INT64_C(131072)+65536,-1,0,1,65536,INT64_MAX};
  unsigned cases=0;
  for(unsigned n=0;n<sizeof(values)/sizeof(*values);++n)
    for(int before=0;before<2;++before) for(int cancel=0;cancel<2;++cancel) {
      rinzu_async_runtime_t runtime;rinzu_async_init(&runtime);
      rinzu_native_completion_v3_t *carrier=malloc(sizeof(*carrier));assert(carrier);
      rinzu_native_completion_v2_init(&carrier->completion);
      carrier->completion.base.version=3;carrier->completion.base.size=sizeof(*carrier);
      assert(rinzu_native_completion_valid(&carrier->completion.base));
      transfer_publication_t publication={carrier,values[n]};
      if(before) publish_transfer(&publication);
      rinzu_async_future_t *first=wait_transfer(&runtime,carrier,RINZU_SCALAR_I64);
      rinzu_async_future_t *alias=wait_transfer(&runtime,carrier,RINZU_SCALAR_I64);
      rinzu_async_future_t *unit=wait_transfer(&runtime,carrier,RINZU_TRANSFER_UNIT);
      if(cancel && !before) rinzu_async_cancel(first);
      pthread_t publisher;
      if(!before) assert(!pthread_create(&publisher,NULL,publish_transfer,&publication));
      receive_transfer(unit);
      assert(atomic_load_explicit(&first->physical_done,memory_order_acquire));
      assert(atomic_load_explicit(&alias->physical_done,memory_order_acquire));
      assert((int64_t)first->result_slot==values[n] && (int64_t)alias->result_slot==values[n]);
      /* Publisher's final exchange is its last carrier access. Its wake may
         finish after free; later Future materialization/reawait uses the copy. */
      free(carrier);
      if(!before) assert(!pthread_join(publisher,NULL));
      receive_transfer(first);receive_transfer(alias);
      assert((int64_t)rinzu_async_materialize(alias)==values[n]);
      assert((int64_t)rinzu_async_materialize(alias)==values[n]);
      assert(first->failure.primary==(cancel && !before ? 2 : 0));
      rinzu_async_release(first);rinzu_async_release(alias);rinzu_async_release(unit);
      rinzu_async_dispose(&runtime);++cases;
    }
  for(unsigned i=0;i<RINZU_RESOURCE_COUNT;++i) assert(!rinzu_resource_counters[i].live);
  printf("native-transfer:%u;copy-before-free;aliases;cancel;cleanup:zero\n",cases);
}

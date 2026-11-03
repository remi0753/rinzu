#include "rinzu/managed_data.h"
#include <assert.h>
#include <stdio.h>
static void *foreign_attach(void *raw) {
  rinzu_managed_roots_t roots={0};rinzu_managed_root_set_t entry={0};
  rinzu_managed_attach_roots(raw,&entry,&roots);return NULL;
}
int main(int argc,char **argv) {
  atomic_store(&rinzu_resource_enabled,1);
  rinzu_managed_data_t data={0};rinzu_managed_roots_t a={0},b={0},fixed_state={0};
  rinzu_managed_root_set_t ae={0},be={0},fe={0};
  uint64_t fixed_words[2]={0};rinzu_managed_frame_t fixed={.roots=fixed_words,.capacity=2,.allocation=1};
  fixed_state.free_frames=&fixed;
  rinzu_managed_attach_roots(&data,&ae,&a);rinzu_managed_attach_roots(&data,&be,&b);rinzu_managed_attach_roots(&data,&fe,&fixed_state);
  if(argc>1) {
    if(!strcmp(argv[1],"foreign")){pthread_t t;assert(!pthread_create(&t,NULL,foreign_attach,&data));assert(!pthread_join(t,NULL));}
    else if(!strcmp(argv[1],"active-detach")){rinzu_managed_enter(&a);rinzu_managed_detach_roots(&data,&ae);}
    else if(!strcmp(argv[1],"borrowed-dispose")){rinzu_managed_frame_cache_t cache={.borrowed=1};rinzu_managed_frame_cache_dispose(&cache);}
    else if(!strcmp(argv[1],"writing")){rinzu_managed_enter(&a);(void)rinzu_managed_prepare_roots(&a,1);rinzu_managed_leave(&a);}
    else abort();
    return 3;
  }
  rinzu_managed_enter(&a);rinzu_managed_frame_t *first=a.current;
  uint64_t old=rinzu_managed_string(&data,(const unsigned char *)"retired root",12);
  rinzu_managed_publish(&a,&old,1);assert(data.published_root_count==1);
  rinzu_managed_leave(&a);assert(!data.published_root_count);
#if RINZU_MANAGED_FRAME_CACHE_COUNT
  assert(data.frame_cache.count==1 && data.frame_cache.borrowed==0);
#endif
  rinzu_managed_collect(&data,&a);assert(!data.objects);assert(!rinzu_managed_lookup_owned(&data,old));
  rinzu_managed_enter(&b);
#if RINZU_MANAGED_FRAME_CACHE_COUNT
  assert(b.current==first && !b.current->count && b.current->roots[0]==old);
#else
  (void)first;
#endif
  uint64_t value=rinzu_managed_string(&data,(const unsigned char *)"a payload longer than inline storage",36);assert(value!=old);
  uint64_t *slots=rinzu_managed_prepare_roots(&b,2);slots[0]=value;slots[1]=value;rinzu_managed_commit_roots(&b,2);
  rinzu_managed_enter(&a);rinzu_managed_publish(&a,&value,1);assert(data.published_root_count==3);
  rinzu_managed_collect(&data,&b);assert(data.objects==1 && rinzu_managed_find(&data,value)->length==36);
  rinzu_managed_leave(&a);rinzu_managed_leave(&b);
  // The embedded task argument frame must never enter the Domain free list.
  rinzu_managed_enter(&fixed_state);assert(fixed_state.current==&fixed);
  rinzu_managed_publish(&fixed_state,&value,1);rinzu_managed_enter(&fixed_state);
  rinzu_managed_publish(&fixed_state,&value,1);rinzu_managed_collect(&data,&fixed_state);assert(data.objects==1);
  rinzu_managed_leave(&fixed_state);rinzu_managed_leave(&fixed_state);assert(fixed_state.free_frames==&fixed && fixed.allocation==1);
#if RINZU_MANAGED_FRAME_CACHE_COUNT
  assert(!fixed.previous);
#endif
  // Distinct active chains must never alias cached storage. Large buffers and
  // recursion exceed cache bounds and are physically freed when inactive.
  for(unsigned i=0;i<130;++i){rinzu_managed_enter(&a);uint64_t *p=rinzu_managed_prepare_roots(&a,8);for(unsigned j=0;j<8;++j)p[j]=value;rinzu_managed_commit_roots(&a,8);}
  assert(data.published_root_count==1040);
  rinzu_managed_collect(&data,&a);assert(data.objects==1);
  for(unsigned i=0;i<130;++i)rinzu_managed_leave(&a);
  assert(data.frame_cache.count<=RINZU_MANAGED_FRAME_CACHE_COUNT && data.frame_cache.bytes<=RINZU_MANAGED_FRAME_CACHE_BYTES && !data.frame_cache.borrowed);
  rinzu_managed_enter(&b);(void)rinzu_managed_prepare_roots(&b,16384);
  // Complete every compiler store before committing; a zero publication later
  // makes all stale capacity inert for GC, including the over-budget frame.
  for(unsigned i=0;i<16384;++i)b.current->roots[i]=value;
  rinzu_managed_commit_roots(&b,16384);rinzu_managed_leave(&b);
  assert(!data.published_root_count && data.frame_cache.bytes<=RINZU_MANAGED_FRAME_CACHE_BYTES);
  rinzu_managed_collect(&data,&b);assert(!data.objects);
  uint64_t runtime=rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live;
  assert(runtime==data.frame_cache.bytes+data.metadata_cache_count*sizeof(rinzu_managed_value_t));
  rinzu_managed_detach_roots(&data,&ae);rinzu_managed_detach_roots(&data,&be);rinzu_managed_detach_roots(&data,&fe);
  fixed_state.free_frames=fixed.previous;
  rinzu_managed_dispose(&a);rinzu_managed_dispose(&b);rinzu_managed_dispose(&fixed_state);rinzu_managed_data_dispose(&data);
  for(unsigned i=0;i<RINZU_RESOURCE_COUNT;++i)assert(!rinzu_resource_counters[i].live);
  puts("R02:frame-cache:chains:stale-root:alias:embedded:bounded:gc:retained-charged:dispose-zero");
}

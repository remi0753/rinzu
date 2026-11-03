#include <assert.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
typedef struct { void *pointer; size_t bytes; } allocation_t;
static allocation_t allocations[32768];
static size_t allocation_calls, failure_at, live_bytes, peak_bytes;
static void note(void *p, size_t bytes) {
  if (!p) return;
  for (size_t i=0;i<sizeof allocations/sizeof *allocations;++i) {
    if (!allocations[i].pointer) {
      allocations[i]=(allocation_t){p,bytes}; live_bytes+=bytes;
      if (live_bytes>peak_bytes) peak_bytes=live_bytes;
      return;
    }
  }
  abort();
}
static void *guard_calloc(size_t n,size_t bytes) {
  if (++allocation_calls==failure_at) return NULL;
  void *p=calloc(n,bytes); note(p,n*bytes); return p;
}
static void *guard_malloc(size_t bytes) {
  if (++allocation_calls==failure_at) return NULL;
  void *p=malloc(bytes); note(p,bytes); return p;
}
static void guard_free(void *p) {
  if (!p) return;
  for (size_t i=0;i<sizeof allocations/sizeof *allocations;++i) {
    if (allocations[i].pointer==p) {
      assert(live_bytes>=allocations[i].bytes); live_bytes-=allocations[i].bytes;
      allocations[i]=(allocation_t){0}; free(p); return;
    }
  }
  abort();
}
#define calloc guard_calloc
#define malloc guard_malloc
#define free guard_free
#include "rinzu/managed_errors.h"
#undef calloc
#undef malloc
#undef free
#include <sys/wait.h>
#ifndef RINZU_MANAGED_SMALL_ENUM_MODE
#define RINZU_MANAGED_SMALL_ENUM_MODE 0
#define rinzu_managed_composite_fresh rinzu_managed_composite
#endif
static const size_t byte_child[]={0},bool_child[]={1},int_child[]={2},float_child[]={3};
static const size_t string_child[]={4},error_child[]={15},source_child[]={17};
static const size_t location_fields[]={4,2,2};
static const rinzu_key_variant_t byte_variants[]={{0,NULL},{1,byte_child}};
static const rinzu_key_variant_t bool_variants[]={{0,NULL},{1,bool_child}};
static const rinzu_key_variant_t int_variants[]={{0,NULL},{1,int_child}};
static const rinzu_key_variant_t float_variants[]={{0,NULL},{1,float_child}};
static const rinzu_key_variant_t string_variants[]={{0,NULL},{1,string_child}};
static const rinzu_key_variant_t error_variants[]={{0,NULL},{1,error_child}};
static const rinzu_key_variant_t source_variants[]={{0,NULL},{1,source_child}};
static const rinzu_key_variant_t empty_variants[]={{0,NULL},{0,NULL}};
static const rinzu_value_type_t types[]={
 {.kind=RINZU_VALUE_SCALAR,.scalar_kind=RINZU_SCALAR_U8},
 {.kind=RINZU_VALUE_SCALAR,.scalar_kind=RINZU_SCALAR_BOOL},
 {.kind=RINZU_VALUE_SCALAR,.scalar_kind=RINZU_SCALAR_I64},
 {.kind=RINZU_VALUE_SCALAR,.scalar_kind=RINZU_SCALAR_F64},
 {.kind=RINZU_VALUE_STRING},
 {.kind=RINZU_VALUE_ENUM,.identity="ByteOption",.variant_count=2,.variants=byte_variants},
 {.kind=RINZU_VALUE_ENUM,.identity="OtherByteOption",.variant_count=2,.variants=byte_variants},
 {.kind=RINZU_VALUE_ENUM,.identity="BoolOption",.variant_count=2,.variants=bool_variants},
 {.kind=RINZU_VALUE_ENUM,.identity="IntOption",.variant_count=2,.variants=int_variants},
 {.kind=RINZU_VALUE_ENUM,.identity="FloatOption",.variant_count=2,.variants=float_variants},
 {.kind=RINZU_VALUE_ENUM,.identity="StringOption",.variant_count=2,.variants=string_variants},
 {.kind=RINZU_VALUE_ENUM,.identity="EmptyEnum",.variant_count=2,.variants=empty_variants},
 {.kind=RINZU_VALUE_RECORD,.identity="ByteRecord",.field_count=1,.fields=byte_child},
 {.kind=RINZU_VALUE_STATE,.identity="ByteState",.field_count=1,.fields=byte_child},
 {.kind=RINZU_VALUE_LIST,.element=0},
 {.kind=RINZU_VALUE_ERROR},
 {.kind=RINZU_VALUE_ENUM,.identity="ErrorOption",.variant_count=2,.variants=error_variants},
 {.kind=RINZU_VALUE_RECORD,.identity="SourceLocation",.field_count=3,.fields=location_fields},
 {.kind=RINZU_VALUE_ENUM,.identity="SourceOption",.variant_count=2,.variants=source_variants}
};
static uint64_t make(rinzu_managed_data_t *d,size_t type,int64_t variant,uint64_t bits,unsigned ref,size_t count) {
  rinzu_managed_slot_t slot={bits,ref}; uint64_t out=0;
  assert(!rinzu_managed_composite(d,type,variant,&slot,count,&out)); return out;
}
static void inspect(rinzu_managed_data_t *d,uint64_t id,size_t type,int64_t variant,uint64_t bits,size_t count) {
  rinzu_managed_value_t *v=rinzu_managed_find(d,id);
  assert(v->kind==RINZU_MANAGED_COMPOSITE && v->type==type && v->count==count);
  assert(v->has_variant==(variant!=-1));
  if (variant!=-1) assert(v->variant==(uint32_t)variant);
  if (count) assert(v->slots[0].bits==bits);
}
static void *foreign(void *raw) { (void)make(raw,5,0,0,0,0); return NULL; }
static void retire_head(rinzu_managed_data_t *d,uint64_t id) {
  rinzu_managed_value_t *v=d->values; assert(v && v->identity==id && d->objects);
  d->values=v->next; rinzu_managed_index_remove(d,v); rinzu_managed_value_release(d,v); --d->objects;
}
static void child_exit(pid_t p,int expected) {
  int status; assert(p>0 && waitpid(p,&status,0)==p);
  assert(WIFEXITED(status) && WEXITSTATUS(status)==expected);
}
int main(int argc,char **argv) {
  assert(argc==2); atomic_store(&rinzu_resource_enabled,1);
  rinzu_value_schema_t schema={0}; assert(!rinzu_value_schema_init(&schema,types,sizeof types/sizeof *types));
  rinzu_managed_data_t d={0},other={0};
  assert(!rinzu_managed_configure_values(&d,&schema)); assert(!rinzu_managed_configure_values(&other,&schema));
  if (!strcmp(argv[1],"owner")) {
    (void)make(&d,5,0,0,0,0); /* Exercise the actual cache-hit candidate. */
    pthread_t thread; assert(!pthread_create(&thread,NULL,foreign,&d)); assert(!pthread_join(thread,NULL)); return 3;
  }
  assert(!strcmp(argv[1],"normal"));
  uint64_t none=make(&d,5,0,0,0,0),again_none=make(&d,5,0,0,0,0);
  uint64_t some=make(&d,5,1,13,0,1),again_some=make(&d,5,1,13,0,1);
  assert((none==again_none)==(RINZU_MANAGED_SMALL_ENUM_MODE>=1));
  assert((some==again_some)==(RINZU_MANAGED_SMALL_ENUM_MODE>=2));
  assert(make(&d,6,1,13,0,1)!=some); assert(make(&other,5,1,13,0,1)!=some);
  for (unsigned repeat=0;repeat<4;++repeat) for (unsigned byte=0;byte<256;++byte) {
    uint64_t id=make(&d,5,1,byte,0,1); inspect(&d,id,5,1,byte,1);
  }
  for (unsigned bit=0;bit<2;++bit) {
    uint64_t a=make(&d,7,1,bit,0,1),b=make(&d,7,1,bit,0,1);
    assert((a==b)==(RINZU_MANAGED_SMALL_ENUM_MODE>=2));
  }
  uint64_t text=rinzu_managed_string(&d,(const unsigned char *)"x",1);
  size_t excluded[]={8,9,10,12,13,14};
  for (size_t i=0;i<sizeof excluded/sizeof *excluded;++i) {
    size_t type=excluded[i]; uint64_t bits=type==10?text:0; unsigned ref=type==10;
    int64_t variant=type<11?1:-1;
    uint64_t a=make(&d,type,variant,bits,ref,1),b=make(&d,type,variant,bits,ref,1); assert(a!=b);
  }
  uint64_t output=UINT64_C(0x12345678); size_t objects=d.objects;
  rinzu_managed_slot_t bad={256,0};
  assert(rinzu_managed_composite(&d,5,1,&bad,1,&output)==RINZU_VALUE_TYPE);
  bad.bits=2; assert(rinzu_managed_composite(&d,7,1,&bad,1,&output)==RINZU_VALUE_TYPE);
  bad=(rinzu_managed_slot_t){some,1}; assert(rinzu_managed_composite(&d,5,1,&bad,1,&output)==RINZU_VALUE_TYPE);
  bad=(rinzu_managed_slot_t){rinzu_managed_string(&other,(const unsigned char *)"x",1),1};
  assert(rinzu_managed_composite(&d,10,1,&bad,1,&output)==RINZU_VALUE_TYPE);
  assert(rinzu_managed_composite(&d,5,-1,NULL,0,&output)==RINZU_VALUE_TYPE);
  assert(output==UINT64_C(0x12345678) && d.objects==objects);
  (void)make(&d,18,0,0,0,0); (void)make(&d,16,0,0,0,0);
  size_t before=d.objects;
  uint64_t a=rinzu_managed_source_option(&d,18,(rinzu_source_position_t){0});
  uint64_t b=rinzu_managed_source_option(&d,18,(rinzu_source_position_t){0});
  assert(a!=b && d.objects==before+2);
  rinzu_failure_t failure={.primary=5}; uint64_t error=rinzu_managed_error(&d,&failure); before=d.objects;
  a=rinzu_managed_error_metadata(&d,16,error,3); b=rinzu_managed_error_metadata(&d,16,error,3);
  assert(a!=b && d.objects==before+2);
  rinzu_managed_roots_t roots={0}; rinzu_managed_enter(&roots);
  uint64_t rooted_empty=make(&d,11,1,0,0,0),rooted_byte=make(&d,5,1,10,0,1);
  uint64_t live[]={rooted_byte,rooted_empty};
  rinzu_managed_publish(&roots,live,2); rinzu_managed_collect(&d,&roots);
  a=make(&d,5,1,10,0,1); assert((a==live[0])==(RINZU_MANAGED_SMALL_ENUM_MODE>=2));
  inspect(&d,a,5,1,10,1);
  rinzu_managed_publish(&roots,NULL,0); rinzu_managed_collect(&d,&roots);
  assert(!d.objects && !rinzu_managed_lookup(&d,live[0]));
  uint64_t reused=rinzu_managed_string(&d,(const unsigned char *)"reuse",5);
  a=make(&d,5,1,10,0,1); assert(a!=live[0] && a!=reused);
  retire_head(&d,a); b=make(&d,5,1,10,0,1); assert(b!=a); inspect(&d,b,5,1,10,1);
#if RINZU_MANAGED_SMALL_ENUM_MODE
  assert(d.small_enums && d.small_enum_accounted_bytes==RINZU_MANAGED_SMALL_ENUM_CAPACITY*sizeof(*d.small_enums));
  assert(d.small_enum_accounted_bytes==1536);
#endif
  rinzu_managed_leave(&roots); rinzu_managed_collect(&d,&roots); assert(!d.objects);
  rinzu_managed_dispose(&roots); rinzu_managed_data_dispose(&d); rinzu_managed_data_dispose(&other);
  rinzu_value_schema_dispose(&schema);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED_OBJECTS].live);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live);
  assert(!live_bytes);
  const char *lookup=getenv("RINZU_MANAGED_LOOKUP_CACHE");
  size_t failures=(!lookup || strcmp(lookup,"0"))?3:2;
  if (RINZU_MANAGED_SMALL_ENUM_MODE) ++failures;
  for (size_t fail=1;fail<=failures;++fail) {
    pid_t child=fork(); assert(child>=0);
    if (!child) {
      rinzu_value_schema_t schema2={0}; assert(!rinzu_value_schema_init(&schema2,types,sizeof types/sizeof *types));
      rinzu_managed_data_t oom={0}; assert(!rinzu_managed_configure_values(&oom,&schema2));
      allocation_calls=0; failure_at=fail; (void)make(&oom,5,0,0,0,0); _Exit(3);
    }
    child_exit(child,71);
  }
  printf("PASS small enum mode=%d peak=%zu: types, all bytes, exclusions, guards, weak GC, rollback, fresh Error quota, Owner, OOM, physical/resources0\n",RINZU_MANAGED_SMALL_ENUM_MODE,peak_bytes);
}

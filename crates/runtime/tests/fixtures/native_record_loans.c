#include <stdlib.h>
#include <stdint.h>
#include <unistd.h>
static _Thread_local int budget = -1;
static void *controlled_calloc(size_t count, size_t size) {
  if (!budget) return NULL;
  if (budget > 0) --budget;
  return calloc(count, size);
}
static int controlled_aligned(void **pointer, size_t alignment, size_t size) {
  if (!budget) { *pointer = NULL; return 12; }
  if (budget > 0) --budget;
  return posix_memalign(pointer, alignment, size);
}
static _Noreturn void checked_exit(int code);
#define calloc controlled_calloc
#define posix_memalign controlled_aligned
#define _Exit checked_exit
#include "rinzu/native_record.h"
#undef _Exit
#undef calloc
#undef posix_memalign
#include <assert.h>
#include <stdio.h>
#include <string.h>

struct Pair { int64_t x; _Bool flag; };
static const size_t pair_fields[] = {0, 1}, box_fields[] = {2};
static const rinzu_value_type_t types[] = {
    {.kind=RINZU_VALUE_SCALAR,.scalar_kind=RINZU_SCALAR_I64},
    {.kind=RINZU_VALUE_SCALAR,.scalar_kind=RINZU_SCALAR_BOOL},
    {.kind=RINZU_VALUE_STATE,.identity="Pair",.field_count=2,.fields=pair_fields},
    {.kind=RINZU_VALUE_RECORD,.identity="Box",.field_count=1,.fields=box_fields},
    {.kind=RINZU_VALUE_STATE,.identity="Other",.field_count=2,.fields=pair_fields},
};
static const rinzu_native_record_field_t fields[] = {
    {RINZU_SCALAR_I64,offsetof(struct Pair,x)},
    {RINZU_SCALAR_BOOL,offsetof(struct Pair,flag)},
};
static const rinzu_native_record_layout_t layout = {2,sizeof(struct Pair),_Alignof(struct Pair),2,fields};
static const rinzu_native_record_layout_t wrong = {4,sizeof(struct Pair),_Alignof(struct Pair),2,fields};
static rinzu_managed_value_t *verify_unpublished;
static _Noreturn void checked_exit(int code) {
  if (verify_unpublished && verify_unpublished->slots[0].bits != 1)
    _Exit(87);
  _Exit(code);
}
static uint64_t create(rinzu_managed_data_t *data) {
  rinzu_managed_slot_t slots[] = {{1,0},{0,0}};
  uint64_t value=0;
  assert(!rinzu_managed_composite(data,2,-1,slots,2,&value));
  return value;
}
int main(int argc, char **argv) {
  rinzu_value_schema_t schema={0};
  assert(!rinzu_value_schema_init(&schema,types,sizeof(types)/sizeof(*types)));
  rinzu_managed_data_t data={0};
  rinzu_managed_roots_t roots={0};
  assert(!rinzu_managed_configure_values(&data,&schema));
  data.gc.stress=1;
  uint64_t first=create(&data), second=create(&data), box=0;
  rinzu_managed_slot_t child={first,1};
  assert(!rinzu_managed_composite(&data,3,-1,&child,1,&box));
  rinzu_managed_enter(&roots);
  rinzu_managed_publish(&roots,&box,1);
  rinzu_native_record_argument_t arguments[]={{first,&layout},{first,&layout},{second,&layout}};
  rinzu_native_record_field_t altered_fields[] = {fields[0], fields[1]};
  rinzu_native_record_layout_t altered_layout = layout;
  rinzu_native_record_layout_t copied_layout = layout;
  altered_fields[1].offset += 1;
  altered_layout.fields = altered_fields;
  rinzu_native_record_loan_t *loan=NULL;
  if (argc>1 && !strcmp(argv[1],"oom")) budget=0;
  if (argc>1 && !strcmp(argv[1],"wrong-type")) arguments[0].layout=&wrong;
  if (argc>1 && !strcmp(argv[1],"alias-layout")) arguments[1].layout=&altered_layout;
  if (argc>1 && !strcmp(argv[1],"copied-layout")) arguments[1].layout=&copied_layout;
  if (argc>1 && !strcmp(argv[1],"null-alias-layout")) arguments[1].layout=NULL;
  if (argc>1 && !strcmp(argv[1],"wrong-domain")) {
    rinzu_managed_data_t foreign={0};
    assert(!rinzu_managed_configure_values(&foreign,&schema));
    rinzu_native_record_prepare(&foreign,arguments,3,&loan);
    return 90;
  }
  assert(!rinzu_native_record_prepare(&data,arguments,3,&loan));
  assert(loan->link.count==2);
  unsigned char *a=rinzu_native_record_argument_pointer(&data,loan,0);
  unsigned char *b=rinzu_native_record_argument_pointer(&data,loan,2);
  assert(a==rinzu_native_record_argument_pointer(&data,loan,1) && a!=b);
  rinzu_managed_slot_t slot;
  assert(rinzu_managed_project(&data,first,2,-1,0,&slot)==RINZU_VALUE_BUSY);
  assert(!rinzu_managed_project(&data,box,3,-1,0,&slot) && slot.reference && slot.bits==first);
  uint64_t echo=0;
  assert(!rinzu_managed_receive_record(&data,&layout,a,&echo) && echo==first);
  rinzu_native_record_loan_t *inner=NULL;
  budget=0; /* Busy detection cannot allocate or acquire a partial target set. */
  assert(rinzu_native_record_prepare(&data,arguments,3,&inner)==RINZU_VALUE_BUSY && !inner);
  budget=-1;
  uint64_t third=create(&data);
  rinzu_native_record_argument_t late_busy[]={{third,&layout},{first,&layout}};
  size_t retained_roots=data.published_root_count, scratch_allocations=data.native_record_allocations;
  budget=0;
  assert(rinzu_native_record_prepare(&data,late_busy,2,&inner)==RINZU_VALUE_BUSY && !inner);
  assert(data.published_root_count==retained_roots && data.native_record_allocations==scratch_allocations);
  budget=-1;
  rinzu_native_record_argument_t unrelated={third,&layout};
  assert(!rinzu_native_record_prepare(&data,&unrelated,1,&inner));
  rinzu_native_record_release(&data,inner);
  for (int index=0;index<8;++index) {
    rinzu_managed_collect(&data,&roots);
    assert(a==rinzu_native_record_argument_pointer(&data,loan,0));
    assert(rinzu_managed_project(&data,first,2,-1,0,&slot)==RINZU_VALUE_BUSY);
  }
  if (argc>1 && !strcmp(argv[1],"discard")) {
    rinzu_native_record_release(&data,loan);
    assert(!rinzu_managed_project(&data,first,2,-1,0,&slot) && slot.bits==1);
  } else {
    rinzu_native_record_enter(&data,loan);
    int64_t changed=42;
    memcpy(a+offsetof(struct Pair,x),&changed,sizeof(changed));
    a[offsetof(struct Pair,flag)]=1;
    changed=99;
    memcpy(b+offsetof(struct Pair,x),&changed,sizeof(changed));
    if (argc>1 && !strcmp(argv[1],"early-release")) rinzu_native_record_release(&data,loan);
    if (argc>1 && !strcmp(argv[1],"early-commit")) rinzu_native_record_commit(&data,loan);
    if (argc>1 && !strcmp(argv[1],"dispose-entered")) rinzu_managed_data_dispose(&data);
    rinzu_native_record_returned(&data,loan);
    assert(rinzu_managed_project(&data,first,2,-1,0,&slot)==RINZU_VALUE_BUSY);
    if (argc>1 && !strcmp(argv[1],"invalid-bool")) {
      b[offsetof(struct Pair,flag)]=2;
      verify_unpublished=rinzu_managed_find(&data,first);
    }
    budget=0; /* Validation and publication reuse preparation's staging. */
    rinzu_native_record_commit(&data,loan);
    budget=-1;
    assert(!rinzu_managed_project(&data,first,2,-1,0,&slot) && slot.bits==42);
    assert(!rinzu_managed_project(&data,second,2,-1,0,&slot) && slot.bits==99);
    assert(!rinzu_managed_receive_record(&data,&layout,a,&echo) && echo==first);
    struct Pair incoming={7,1};
    uint64_t received=0, again=0;
    assert(!rinzu_managed_receive_record(&data,&layout,(unsigned char *)&incoming,&received));
    assert(!rinzu_managed_receive_record(&data,&layout,(unsigned char *)&incoming,&again));
    assert(received!=again && received!=first);
    rinzu_native_record_release(&data,loan);
    assert(!rinzu_managed_project(&data,echo,2,-1,0,&slot) && slot.bits==42);
  }
  assert(!data.native_record_loans && !data.root_sets && !data.published_root_count);
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data,&roots);
  assert(!data.objects);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_value_schema_dispose(&schema);
  puts("PASS shared native record loans");
  return 0;
}

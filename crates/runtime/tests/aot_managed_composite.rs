use rinzu_runtime::data::{DataError, ManagedData, ManagedHandle, ManagedSlot as Slot};
use rinzu_runtime::failure::{ErrorKind, Failure, FailureCause};
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use rinzu_runtime::scalar::{ScalarKind as K, ScalarValue as S};
use rinzu_runtime::value::{Value, ValueError, ValueSchema, ValueType as T};
use std::process::Command;
use std::sync::Arc;

#[path = "support/value_schema.rs"]
mod value_schema;

fn types() -> Vec<T> {
    vec![
        T::Scalar(K::U64),
        T::Scalar(K::F64),
        T::Unit,
        T::String,
        T::Bytes(3),
        T::Record {
            identity: "fixture::Packet".into(),
            fields: vec![0, 3, 4, 1],
        },
        T::Enum {
            identity: "fixture::Result".into(),
            variants: vec![vec![5], vec![5], vec![]],
        },
        T::List(5),
        T::Map { key: 3, value: 5 },
        T::Other,
        T::Record {
            identity: "fixture::ErrorBox".into(),
            fields: vec![9],
        },
        T::List(0),
        T::Enum {
            identity: "fixture::Key".into(),
            variants: vec![vec![0, 3], vec![]],
        },
        T::Map { key: 12, value: 0 },
    ]
}
fn integer(bits: u64) -> Slot {
    Slot::Scalar(S::from_bits(K::U64, bits).unwrap())
}
fn handle(slot: Slot) -> ManagedHandle {
    if let Slot::Reference(handle) = slot {
        handle
    } else {
        panic!("reference")
    }
}
fn bits(slot: Slot) -> u64 {
    if let Slot::Scalar(value) = slot {
        value.bits()
    } else {
        panic!("scalar")
    }
}
fn packet(
    data: &mut ManagedData,
    heap: &mut DomainHeap,
    number: u64,
    floating: u64,
) -> ManagedHandle {
    let text = data.create_string(heap, "日本語\0tail").unwrap();
    let bytes = data.create_bytes(heap, 3, &[0, 255, 42]).unwrap();
    data.create_composite(
        heap,
        5,
        None,
        &[
            integer(number),
            Slot::Reference(text),
            Slot::Reference(bytes),
            Slot::Scalar(S::from_bits(K::F64, floating).unwrap()),
        ],
    )
    .unwrap()
}
fn status<T>(value: Result<T, DataError>) -> u32 {
    match value {
        Ok(_) => 0,
        Err(DataError::Value(ValueError::InvalidSchema)) => 1,
        Err(DataError::Value(ValueError::NoEquality)) => 3,
        Err(DataError::Value(ValueError::NotKey)) => 4,
        Err(DataError::Value(ValueError::DuplicateKey)) => 5,
        Err(
            DataError::Value(ValueError::TypeMismatch)
            | DataError::UnknownHandle
            | DataError::WrongKind
            | DataError::InvalidEncoding,
        ) => 2,
        Err(error) => panic!("unexpected {error:?}"),
    }
}
fn collect(data: &mut ManagedData, heap: &mut DomainHeap, roots: &[ManagedHandle]) {
    data.retain_roots(heap, roots.iter().copied()).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
}

fn expected(types: Vec<T>, deep_types: Vec<T>) -> String {
    let schema = Arc::new(ValueSchema::new(types).unwrap());
    let mut heap = DomainHeap::new(1);
    let mut data = ManagedData::new(1);
    data.configure_values(&heap, schema.clone()).unwrap();
    let mut lines = Vec::new();
    let a = packet(&mut data, &mut heap, u64::MAX, (-0.0f64).to_bits());
    let b = packet(&mut data, &mut heap, u64::MAX, 0.0f64.to_bits());
    lines.push(format!(
        "record:{}",
        data.values_equal(&heap, 5, Slot::Reference(a), Slot::Reference(b))
            .unwrap() as u8
    ));
    let parent = data
        .create_composite(&mut heap, 6, Some(0), &[Slot::Reference(a)])
        .unwrap();
    collect(&mut data, &mut heap, &[parent]);
    let record = handle(data.project(&mut heap, parent, 6, Some(0), 0).unwrap());
    let number = bits(data.project(&mut heap, record, 5, None, 0).unwrap());
    let text = handle(data.project(&mut heap, record, 5, None, 1).unwrap());
    let bytes = handle(data.project(&mut heap, record, 5, None, 2).unwrap());
    let before = data.snapshot(&heap, 5, Slot::Reference(record)).unwrap();
    for _ in 0..3 {
        collect(&mut data, &mut heap, &[parent, record, text, bytes]);
    }
    lines.push(format!(
        "moving:{}:{number}:{}:{}",
        heap.object_count(),
        data.bytes(&heap, bytes, 3).unwrap().len(),
        data.value_hash(&heap, 3, Slot::Reference(text)).unwrap()
    ));
    assert!(
        schema
            .equal(
                5,
                &before,
                &data.snapshot(&heap, 5, Slot::Reference(record)).unwrap()
            )
            .unwrap()
    );
    let baseline = (data.root_count(), heap.object_count());
    let bad = status(data.project(&mut heap, parent, 6, Some(1), 0));
    lines.push(format!(
        "guard:{bad}:{}",
        (baseline == (data.root_count(), heap.object_count())) as u8
    ));
    collect(&mut data, &mut heap, &[text, bytes]);
    lines.push(format!(
        "children:{}:{}:{}",
        heap.object_count(),
        data.string(&heap, text).unwrap().len(),
        data.bytes(&heap, bytes, 3).unwrap()[1]
    ));
    collect(&mut data, &mut heap, &[]);

    let low = packet(&mut data, &mut heap, 1, 0.0f64.to_bits());
    let high = packet(&mut data, &mut heap, u64::MAX, (-0.0f64).to_bits());
    let same = packet(&mut data, &mut heap, u64::MAX, 0.0f64.to_bits());
    let ka = data.create_string(&mut heap, "key\0日本語").unwrap();
    let kb = data.create_string(&mut heap, "key\0日本語").unwrap();
    let map = data
        .create_composite(
            &mut heap,
            8,
            None,
            &[
                Slot::Reference(ka),
                Slot::Reference(low),
                Slot::Reference(kb),
                Slot::Reference(high),
            ],
        )
        .unwrap();
    let other = data
        .create_composite(
            &mut heap,
            8,
            None,
            &[Slot::Reference(ka), Slot::Reference(same)],
        )
        .unwrap();
    let baseline = (data.root_count(), heap.object_count());
    let bad = status(data.create_composite(
        &mut heap,
        8,
        None,
        &[
            Slot::Reference(ka),
            Slot::Reference(ka),
            Slot::Reference(kb),
            Slot::Reference(high),
        ],
    ));
    lines.push(format!(
        "prepare:{bad}:{}",
        (baseline == (data.root_count(), heap.object_count())) as u8
    ));
    collect(&mut data, &mut heap, &[map, other]);
    let Value::Map(entries) = data.snapshot(&heap, 8, Slot::Reference(map)).unwrap() else {
        panic!("Map")
    };
    let record = handle(data.project(&mut heap, map, 8, None, 1).unwrap());
    let number = bits(data.project(&mut heap, record, 5, None, 0).unwrap());
    lines.push(format!(
        "map:{}:{}:{number}:{}",
        data.values_equal(&heap, 8, Slot::Reference(map), Slot::Reference(other))
            .unwrap() as u8,
        entries.len(),
        heap.object_count()
    ));
    collect(&mut data, &mut heap, &[]);

    let nan = packet(&mut data, &mut heap, u64::MAX, f64::NAN.to_bits());
    let list = data
        .create_composite(&mut heap, 7, None, &[Slot::Reference(nan)])
        .unwrap();
    let key = data.create_string(&mut heap, "nan").unwrap();
    let map = data
        .create_composite(
            &mut heap,
            8,
            None,
            &[Slot::Reference(key), Slot::Reference(nan)],
        )
        .unwrap();
    collect(&mut data, &mut heap, &[nan, list, map]);
    lines.push(format!(
        "nan:{}:{}:{}",
        data.values_equal(&heap, 5, Slot::Reference(nan), Slot::Reference(nan))
            .unwrap() as u8,
        data.values_equal(&heap, 7, Slot::Reference(list), Slot::Reference(list))
            .unwrap() as u8,
        data.values_equal(&heap, 8, Slot::Reference(map), Slot::Reference(map))
            .unwrap() as u8
    ));
    let failure = Failure::new(FailureCause::Error(ErrorKind::InvalidUtf8));
    let error = data.create_error(&mut heap, &failure).unwrap();
    let parent = data
        .create_composite(&mut heap, 10, None, &[Slot::Reference(error)])
        .unwrap();
    lines.push(format!(
        "properties:{}:{}:{}",
        status(data.value_hash(&heap, 5, Slot::Reference(nan))),
        status(data.value_hash(&heap, 4, Slot::Reference(nan))),
        status(data.values_equal(&heap, 10, Slot::Reference(parent), Slot::Reference(parent)))
    ));
    collect(&mut data, &mut heap, &[parent]);
    let error = handle(data.project(&mut heap, parent, 10, None, 0).unwrap());
    collect(&mut data, &mut heap, &[error]);
    lines.push(format!(
        "error:{}:{}",
        (data.error(&heap, error).unwrap() == failure) as u8,
        heap.object_count()
    ));
    collect(&mut data, &mut heap, &[]);

    let text = data.create_string(&mut heap, "日本語\0tail").unwrap();
    let key = data
        .create_composite(
            &mut heap,
            12,
            Some(0),
            &[integer(u64::MAX), Slot::Reference(text)],
        )
        .unwrap();
    let hash = data.value_hash(&heap, 12, Slot::Reference(key)).unwrap();
    let text2 = data.create_string(&mut heap, "日本語\0tail").unwrap();
    let same = data
        .create_composite(
            &mut heap,
            12,
            Some(0),
            &[integer(u64::MAX), Slot::Reference(text2)],
        )
        .unwrap();
    let map = data
        .create_composite(
            &mut heap,
            13,
            None,
            &[
                Slot::Reference(key),
                integer(1),
                Slot::Reference(same),
                integer(u64::MAX),
            ],
        )
        .unwrap();
    collect(&mut data, &mut heap, &[map]);
    let projected = handle(data.project(&mut heap, map, 13, None, 0).unwrap());
    let value = bits(data.project(&mut heap, map, 13, None, 1).unwrap());
    lines.push(format!(
        "key:{hash}:{}:{value}",
        (hash
            == data
                .value_hash(&heap, 12, Slot::Reference(projected))
                .unwrap()) as u8
    ));
    collect(&mut data, &mut heap, &[]);

    let string = data.create_string(&mut heap, "must die").unwrap();
    let raw = string.bits();
    let list = data
        .create_composite(&mut heap, 11, None, &[integer(raw)])
        .unwrap();
    collect(&mut data, &mut heap, &[list]);
    lines.push(format!(
        "tag:{}:{}",
        (bits(data.project(&mut heap, list, 11, None, 0).unwrap()) == raw) as u8,
        heap.object_count()
    ));
    collect(&mut data, &mut heap, &[]);
    lines.push(format!(
        "baseline:{}:{}",
        data.root_count(),
        heap.object_count()
    ));

    let mut heap = DomainHeap::new(2);
    let mut data = ManagedData::new(2);
    data.configure_values(&heap, Arc::new(ValueSchema::new(deep_types).unwrap()))
        .unwrap();
    let mut top = data.create_string(&mut heap, "deep\0").unwrap();
    for ty in 2..514 {
        top = data
            .create_composite(&mut heap, ty, None, &[Slot::Reference(top)])
            .unwrap();
    }
    collect(&mut data, &mut heap, &[top]);
    lines.push(format!(
        "deep:{}:{}",
        data.values_equal(&heap, 513, Slot::Reference(top), Slot::Reference(top))
            .unwrap() as u8,
        heap.object_count()
    ));
    let wide = data
        .create_composite(&mut heap, 514, None, &vec![integer(u64::MAX); 129])
        .unwrap();
    collect(&mut data, &mut heap, &[wide]);
    lines.push(format!(
        "wide:{}:{}",
        data.values_equal(&heap, 514, Slot::Reference(wide), Slot::Reference(wide))
            .unwrap() as u8,
        heap.object_count()
    ));
    collect(&mut data, &mut heap, &[]);
    lines.push(format!(
        "deep_baseline:{}:{}",
        data.root_count(),
        heap.object_count()
    ));
    lines.join("\n") + "\n"
}

#[test]
fn c_managed_composites_match_rust_after_moving_gc_and_settle_all_owned_views() {
    let types = types();
    let mut deep = vec![T::String, T::Scalar(K::U64)];
    for ty in 2..514 {
        deep.push(T::List(if ty == 2 { 0 } else { ty - 1 }));
    }
    deep.push(T::Record {
        identity: "fixture::Wide".into(),
        fields: vec![1; 129],
    });
    let expected = expected(types.clone(), deep.clone());
    let mut program = String::from(
        "#include <assert.h>\n#include <stdio.h>\n#include \"rinzu/managed_composite.h\"\n",
    );
    value_schema::emit_types(&mut program, &types, "types");
    value_schema::emit_types(&mut program, &deep, "deep_types");
    program.push_str(r#"
static rinzu_managed_slot_t ref(uint64_t value) { return (rinzu_managed_slot_t){value,1}; }
static rinzu_managed_slot_t scalar(uint64_t value) { return (rinzu_managed_slot_t){value,0}; }
static uint64_t composite(rinzu_managed_data_t *data,size_t type,int64_t variant,const rinzu_managed_slot_t *slots,size_t count) {
  uint64_t output=91; assert(!rinzu_managed_composite(data,type,variant,slots,count,&output)); return output;
}
static rinzu_managed_slot_t project(rinzu_managed_data_t *data,uint64_t value,size_t type,int64_t variant,size_t field) {
  rinzu_managed_slot_t output={91,0}; assert(!rinzu_managed_project(data,value,type,variant,field,&output)); return output;
}
static int equal(rinzu_managed_data_t *data,size_t type,uint64_t a,uint64_t b) {
  int output=37; assert(!rinzu_managed_values_equal(data,type,ref(a),ref(b),&output)); return output;
}
static uint64_t hash(rinzu_managed_data_t *data,size_t type,uint64_t value) {
  uint64_t output=91; assert(!rinzu_managed_value_hash(data,type,ref(value),&output)); return output;
}
static uint64_t packet(rinzu_managed_data_t *data,uint64_t number,uint64_t floating) {
  static const unsigned char text[]="日本語\0tail"; static const unsigned char bytes[]={0,255,42};
  uint64_t string=rinzu_managed_string(data,text,sizeof(text)-1), blob=91; assert(!rinzu_managed_bytes(data,3,bytes,3,&blob));
  const rinzu_managed_slot_t fields[]={scalar(number),ref(string),ref(blob),scalar(floating)}; return composite(data,5,-1,fields,4);
}
static void collect(rinzu_managed_data_t *data,rinzu_managed_roots_t *roots,const uint64_t *values,size_t count) {
  assert(!data->live_views && !data->view_allocations);
  rinzu_managed_publish(roots,values,(int64_t)count); rinzu_managed_collect(data,roots);
}
static void *foreign(void *input) {
  rinzu_managed_data_t *data=input; uint64_t output=91; rinzu_managed_slot_t field=scalar(1);
  (void)rinzu_managed_composite(data,11,-1,&field,1,&output); return NULL;
}
int main(int argc,char **argv) {
  rinzu_value_schema_t schema={0}; assert(!rinzu_value_schema_init(&schema,types,sizeof(types)/sizeof(*types)));
  rinzu_managed_data_t data={0}; rinzu_managed_roots_t roots={0}; rinzu_managed_enter(&roots);
  assert(!rinzu_managed_configure_values(&data,&schema)); assert(rinzu_managed_configure_values(&data,&schema)==RINZU_VALUE_SCHEMA && data.schema==&schema);
  if (argc>1) {
    uint64_t value=packet(&data,UINT64_MAX,0);
    if (!strcmp(argv[1],"foreign")) { pthread_t thread; assert(!pthread_create(&thread,NULL,foreign,&data)); assert(!pthread_join(thread,NULL)); return 99; }
    rinzu_managed_snapshot_t view={0}; assert(!rinzu_managed_snapshot(&data,5,ref(value),&view)); assert(data.live_views==1 && data.view_allocations>0);
    if (!strcmp(argv[1],"collect")) rinzu_managed_collect(&data,&roots); else rinzu_managed_data_dispose(&data);
    return 99;
  }
  uint64_t a=packet(&data,UINT64_MAX,UINT64_C(0x8000000000000000)), b=packet(&data,UINT64_MAX,0);
  printf("record:%d\n",equal(&data,5,a,b));
  rinzu_managed_slot_t fields[]={ref(a)}; uint64_t parent=composite(&data,6,0,fields,1); collect(&data,&roots,&parent,1);
  uint32_t variant=99; assert(!rinzu_managed_enum_variant(&data,parent,6,&variant) && variant==0);
  uint64_t record=project(&data,parent,6,0,0).bits, number=project(&data,record,5,-1,0).bits;
  uint64_t text=project(&data,record,5,-1,1).bits, blob=project(&data,record,5,-1,2).bits;
  for (int index=0;index<3;++index) {
    void *previous_text=rinzu_managed_find(&data,text)->bytes, *previous_blob=rinzu_managed_find(&data,blob)->bytes, *previous_fields=rinzu_managed_find(&data,record)->slots;
    const uint64_t live[]={parent,record,text,blob}; collect(&data,&roots,live,4);
    assert(previous_text!=rinzu_managed_find(&data,text)->bytes && previous_blob!=rinzu_managed_find(&data,blob)->bytes && previous_fields!=rinzu_managed_find(&data,record)->slots);
  }
  printf("moving:%zu:%llu:%zu:%llu\n",data.objects,(unsigned long long)number,rinzu_managed_find(&data,blob)->length,(unsigned long long)hash(&data,3,text));
  rinzu_managed_slot_t untouched={91,0}; size_t baseline=data.objects;
  int bad=rinzu_managed_project(&data,parent,6,1,0,&untouched); assert(untouched.bits==91 && !untouched.reference);
  printf("guard:%d:%d\n",bad,baseline==data.objects);
  /* Narrow composite resolution must still reject every non-composite parent,
     an incorrect exact type/layout, bounds and a reference from another heap. */
  {
    rinzu_managed_data_t foreign_data={0}; assert(!rinzu_managed_configure_values(&foreign_data,&schema));
    const uint64_t invalid[]={0,text,blob,parent+UINT64_C(1000000)};
    for(size_t i=0;i<sizeof(invalid)/sizeof(*invalid);++i) {
      assert(rinzu_managed_project(&data,invalid[i],6,0,0,&untouched)==RINZU_VALUE_TYPE);
      assert(untouched.bits==91 && !untouched.reference);
    }
    assert(rinzu_managed_project(&data,parent,5,0,0,&untouched)==RINZU_VALUE_TYPE);
    assert(rinzu_managed_project(&data,parent,6,0,1,&untouched)==RINZU_VALUE_TYPE);
    assert(rinzu_managed_project(&data,parent,schema.count,0,0,&untouched)==RINZU_VALUE_SCHEMA);
    assert(rinzu_managed_project(&foreign_data,parent,6,0,0,&untouched)==RINZU_VALUE_TYPE);
    uint32_t unchanged=99;
    assert(rinzu_managed_enum_variant(&data,record,5,&unchanged)==RINZU_VALUE_TYPE && unchanged==99);
    rinzu_managed_value_t *object=rinzu_managed_find(&data,parent);
    rinzu_managed_slot_t saved=object->slots[0]; object->slots[0]=scalar(1);
    assert(rinzu_managed_project(&data,parent,6,0,0,&untouched)==RINZU_VALUE_TYPE);
    assert(untouched.bits==91 && !untouched.reference); object->slots[0]=saved;
    size_t saved_count=object->count; object->count=0;
    assert(rinzu_managed_enum_variant(&data,parent,6,&unchanged)==RINZU_VALUE_TYPE && unchanged==99);
    object->count=saved_count;
    rinzu_managed_data_dispose(&foreign_data);
  }
  { const uint64_t live[]={text,blob}; collect(&data,&roots,live,2); }
  printf("children:%zu:%zu:%u\n",data.objects,rinzu_managed_find(&data,text)->length,rinzu_managed_find(&data,blob)->bytes[1]);
  collect(&data,&roots,NULL,0);
  uint64_t low=packet(&data,1,0), high=packet(&data,UINT64_MAX,UINT64_C(0x8000000000000000)), same=packet(&data,UINT64_MAX,0);
  static const unsigned char key_text[]="key\0日本語";
  uint64_t ka=rinzu_managed_string(&data,key_text,sizeof(key_text)-1), kb=rinzu_managed_string(&data,key_text,sizeof(key_text)-1);
  const rinzu_managed_slot_t pairs[]={ref(ka),ref(low),ref(kb),ref(high)}, other_pair[]={ref(ka),ref(same)};
  uint64_t map=composite(&data,8,-1,pairs,4), other=composite(&data,8,-1,other_pair,2);
  const rinzu_managed_slot_t bad_pair[]={ref(ka),ref(ka),ref(kb),ref(high)}; uint64_t output=91; baseline=data.objects;
  bad=rinzu_managed_composite(&data,8,-1,bad_pair,4,&output); assert(output==91);
  printf("prepare:%d:%d\n",bad,baseline==data.objects);
  { const uint64_t live[]={map,other}; collect(&data,&roots,live,2); }
  record=project(&data,map,8,-1,1).bits; number=project(&data,record,5,-1,0).bits;
  printf("map:%d:%zu:%llu:%zu\n",equal(&data,8,map,other),rinzu_managed_find(&data,map)->count/2,(unsigned long long)number,data.objects);
  /* Corrupt a late field only in this fault fixture: all native view allocations
     must be rolled back without publishing a partial snapshot or result. */
  { rinzu_managed_value_t *object=rinzu_managed_find(&data,record); rinzu_managed_slot_t original=object->slots[1]; object->slots[1]=scalar(1);
    rinzu_managed_snapshot_t view={0}; view.value.kind=77; assert(rinzu_managed_snapshot(&data,5,ref(record),&view)==RINZU_VALUE_TYPE);
    assert(!view.initialized && !view.allocations && !view.owner && view.value.kind==77); int result=37;
    assert(rinzu_managed_values_equal(&data,8,ref(map),ref(other),&result)==RINZU_VALUE_TYPE && result==37);
    assert(!data.live_views && !data.view_allocations); object->slots[1]=original;
  }
  collect(&data,&roots,NULL,0);
  uint64_t nan=packet(&data,UINT64_MAX,UINT64_C(0x7ff8000000000000)); fields[0]=ref(nan); uint64_t list=composite(&data,7,-1,fields,1);
  uint64_t nan_key=rinzu_managed_string(&data,(const unsigned char *)"nan",3); const rinzu_managed_slot_t nan_pair[]={ref(nan_key),ref(nan)}; map=composite(&data,8,-1,nan_pair,2);
  { const uint64_t live[]={nan,list,map}; collect(&data,&roots,live,3); }
  printf("nan:%d:%d:%d\n",equal(&data,5,nan,nan),equal(&data,7,list,list),equal(&data,8,map,map));
  rinzu_failure_t failure={0}; failure.primary=9; uint64_t error=rinzu_managed_error(&data,&failure); fields[0]=ref(error); parent=composite(&data,10,-1,fields,1);
  uint64_t hash_output=91; int equality=37;
  int record_property=rinzu_managed_value_hash(&data,5,ref(nan),&hash_output), bytes_property=rinzu_managed_value_hash(&data,4,ref(nan),&hash_output);
  int error_property=rinzu_managed_values_equal(&data,10,ref(parent),ref(parent),&equality); assert(hash_output==91 && equality==37);
  printf("properties:%d:%d:%d\n",record_property,bytes_property,error_property);
  collect(&data,&roots,&parent,1); error=project(&data,parent,10,-1,0).bits; collect(&data,&roots,&error,1);
  printf("error:%d:%zu\n",rinzu_managed_find(&data,error)->error_primary==9,data.objects); collect(&data,&roots,NULL,0);
  static const unsigned char text_value[]="日本語\0tail";
  text=rinzu_managed_string(&data,text_value,sizeof(text_value)-1); const rinzu_managed_slot_t key_fields[]={scalar(UINT64_MAX),ref(text)};
  uint64_t key=composite(&data,12,0,key_fields,2), original_hash=hash(&data,12,key);
  uint64_t text2=rinzu_managed_string(&data,text_value,sizeof(text_value)-1); const rinzu_managed_slot_t same_fields[]={scalar(UINT64_MAX),ref(text2)};
  same=composite(&data,12,0,same_fields,2); const rinzu_managed_slot_t enum_pairs[]={ref(key),scalar(1),ref(same),scalar(UINT64_MAX)}; map=composite(&data,13,-1,enum_pairs,4);
  collect(&data,&roots,&map,1); key=project(&data,map,13,-1,0).bits; number=project(&data,map,13,-1,1).bits;
  printf("key:%llu:%d:%llu\n",(unsigned long long)original_hash,original_hash==hash(&data,12,key),(unsigned long long)number); collect(&data,&roots,NULL,0);
  text=rinzu_managed_string(&data,(const unsigned char *)"must die",8); uint64_t raw=text; fields[0]=scalar(raw); list=composite(&data,11,-1,fields,1);
  collect(&data,&roots,&list,1); assert(!rinzu_managed_find_optional(&data,text));
  printf("tag:%d:%zu\n",project(&data,list,11,-1,0).bits==raw,data.objects); collect(&data,&roots,NULL,0);
  printf("baseline:%zu:%zu\n",roots.current->count,data.objects);
  /* A registered parked frame is part of the same Main graph. */
  { rinzu_managed_roots_t parked={0}; rinzu_managed_root_set_t entry={0}; rinzu_managed_enter(&parked); rinzu_managed_attach_roots(&data,&entry,&parked);
    record=packet(&data,UINT64_MAX,0); rinzu_managed_publish(&parked,&record,1); collect(&data,&roots,NULL,0); assert(data.objects==3);
    blob=project(&data,record,5,-1,2).bits; rinzu_managed_publish(&roots,&blob,1); rinzu_managed_leave(&parked); rinzu_managed_detach_roots(&data,&entry);
    rinzu_managed_collect(&data,&roots); assert(data.objects==1 && rinzu_managed_find(&data,blob)->bytes[1]==255);
    rinzu_managed_dispose(&parked); collect(&data,&roots,NULL,0);
  }
  rinzu_managed_leave(&roots); rinzu_managed_dispose(&roots); rinzu_managed_data_dispose(&data); rinzu_value_schema_dispose(&schema);
  assert(!data.values && !data.objects && !data.schema);
  rinzu_value_schema_t deep_schema={0}; assert(!rinzu_value_schema_init(&deep_schema,deep_types,sizeof(deep_types)/sizeof(*deep_types)));
  rinzu_managed_data_t deep_data={0}; rinzu_managed_roots_t deep_roots={0}; rinzu_managed_enter(&deep_roots); assert(!rinzu_managed_configure_values(&deep_data,&deep_schema));
  uint64_t top=rinzu_managed_string(&deep_data,(const unsigned char *)"deep\0",5);
  for (size_t type=2;type<514;++type) { fields[0]=ref(top); top=composite(&deep_data,type,-1,fields,1); }
  collect(&deep_data,&deep_roots,&top,1); printf("deep:%d:%zu\n",equal(&deep_data,513,top,top),deep_data.objects);
  rinzu_managed_slot_t wide_fields[129]; for (size_t index=0;index<129;++index) wide_fields[index]=scalar(UINT64_MAX);
  uint64_t wide=composite(&deep_data,514,-1,wide_fields,129); collect(&deep_data,&deep_roots,&wide,1);
  printf("wide:%d:%zu\n",equal(&deep_data,514,wide,wide),deep_data.objects); collect(&deep_data,&deep_roots,NULL,0);
  printf("deep_baseline:%zu:%zu\n",deep_roots.current->count,deep_data.objects);
  rinzu_managed_leave(&deep_roots); rinzu_managed_dispose(&deep_roots); rinzu_managed_data_dispose(&deep_data); rinzu_value_schema_dispose(&deep_schema); return 0;
}
"#);
    let directory =
        std::env::temp_dir().join(format!("rinzu-managed-composite-c-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("test.c"), program).unwrap();
    for (name, flags) in [
        ("optimized", vec!["-O2", "-Wall", "-Wextra", "-Werror"]),
        (
            "sanitized",
            vec!["-O1", "-g", "-fsanitize=address,undefined"],
        ),
    ] {
        let compile = Command::new("clang")
            .arg("-std=c11")
            .args(flags)
            .arg("-I")
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(directory.join("test.c"))
            .arg("-o")
            .arg(directory.join(name))
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(directory.join(name))
            .env("RINZU_GC_STRESS", "1")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{name}: {}\n{}",
            String::from_utf8_lossy(&run.stderr),
            String::from_utf8_lossy(&run.stdout)
        );
        assert_eq!(String::from_utf8(run.stdout).unwrap(), expected, "{name}");
        for mode in ["collect", "dispose", "foreign"] {
            let rejected = Command::new(directory.join(name))
                .env("RINZU_GC_STRESS", "1")
                .arg(mode)
                .output()
                .unwrap();
            assert_eq!(
                rejected.status.code(),
                Some(70),
                "{name} {mode}: {}",
                String::from_utf8_lossy(&rejected.stderr)
            );
            assert!(rejected.stderr.is_empty());
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

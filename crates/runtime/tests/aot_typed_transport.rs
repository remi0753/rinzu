use rinzu_runtime::scalar::{ScalarKind as K, ScalarValue as S};
use rinzu_runtime::transport::{TransportBudget, TransportDirection, typed::Descriptor};
use rinzu_runtime::value::{Value as V, ValueSchema, ValueType as T};
use std::process::Command;
use std::sync::Arc;

#[path = "support/value_schema.rs"]
mod value_schema;

fn scalar(kind: K, bits: u64) -> V {
    V::Scalar(S::from_bits(kind, bits).unwrap())
}
fn bytes(program: &mut String, name: &str, value: &[u8]) {
    program.push_str(&format!(
        "static const unsigned char {name}[]={{{}}};\n",
        if value.is_empty() {
            "0".into()
        } else {
            value
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        }
    ));
}
fn emit_value(program: &mut String, value: &V, next: &mut usize) -> String {
    let name = format!("value_{}", *next);
    *next += 1;
    let initializer = match value {
        V::Scalar(value) => format!(
            ".kind=RINZU_VALUE_SCALAR,.scalar_kind={},.bits=UINT64_C({})",
            value.kind() as u32,
            value.bits()
        ),
        V::Unit => ".kind=RINZU_VALUE_UNIT".into(),
        V::Error(value) => {
            let value = value.encode();
            bytes(program, &format!("{name}_bytes"), &value);
            format!(
                ".kind=RINZU_VALUE_ERROR,.bytes={name}_bytes,.length={}",
                value.len()
            )
        }
        V::String(text) => {
            bytes(program, &format!("{name}_bytes"), text.as_bytes());
            format!(
                ".kind=RINZU_VALUE_STRING,.bytes={name}_bytes,.length={}",
                text.len()
            )
        }
        V::Bytes(value) => {
            bytes(program, &format!("{name}_bytes"), value);
            format!(
                ".kind=RINZU_VALUE_BYTES,.bytes={name}_bytes,.length={}",
                value.len()
            )
        }
        V::Map(entries) => {
            let names = entries
                .iter()
                .map(|(key, value)| {
                    let key = emit_value(program, key, next);
                    let value = emit_value(program, value, next);
                    format!("{{{key},{value}}}")
                })
                .collect::<Vec<_>>();
            if !names.is_empty() {
                program.push_str(&format!(
                    "static const rinzu_value_pair_t {name}_entries[]={{{}}};\n",
                    names.join(",")
                ));
            }
            format!(
                ".kind=RINZU_VALUE_MAP,.count={},.entries={}",
                names.len(),
                if names.is_empty() {
                    "NULL".into()
                } else {
                    format!("{name}_entries")
                }
            )
        }
        V::Record { fields, .. } | V::Enum { fields, .. } | V::List(fields) => {
            let names = fields
                .iter()
                .map(|value| emit_value(program, value, next))
                .collect::<Vec<_>>();
            if !names.is_empty() {
                program.push_str(&format!(
                    "static const rinzu_value_t {name}_fields[]={{{}}};\n",
                    names.join(",")
                ));
            }
            let header = match value {
                V::Record { identity, .. } => {
                    format!(".kind=RINZU_VALUE_RECORD,.identity=\"{identity}\"")
                }
                V::Enum {
                    identity, variant, ..
                } => format!(".kind=RINZU_VALUE_ENUM,.identity=\"{identity}\",.variant={variant}"),
                _ => ".kind=RINZU_VALUE_LIST".into(),
            };
            format!(
                "{header},.count={},.fields={}",
                names.len(),
                if names.is_empty() {
                    "NULL".into()
                } else {
                    format!("{name}_fields")
                }
            )
        }
        V::Other { .. } => panic!("non-transferable fixture"),
    };
    program.push_str(&format!(
        "static const rinzu_value_t {name}={{{initializer}}};\n"
    ));
    name
}

#[test]
fn c_typed_transport_matches_rust_wire_and_frees_every_partial_decode() {
    let mut types = vec![
        T::Scalar(K::U64),
        T::Scalar(K::F64),
        T::String,
        T::Bytes(3),
        T::Record {
            identity: "fixture::Packet".into(),
            fields: vec![0, 1, 2, 3],
        },
        T::Enum {
            identity: "fixture::Key".into(),
            variants: vec![vec![0, 2], vec![]],
        },
        T::List(4),
        T::Map { key: 5, value: 6 },
        T::Unit,
        T::Bytes(0),
        T::Other,
        T::Enum {
            identity: "fixture::Bad".into(),
            variants: vec![vec![], vec![10]],
        },
        T::List(0),
        T::List(8),
    ];
    let packet = |floating| V::Record {
        identity: "fixture::Packet".into(),
        fields: vec![
            scalar(K::U64, u64::MAX),
            scalar(K::F64, floating),
            V::String("日本語\0🙂".into()),
            V::Bytes(vec![0, 255, 42]),
        ],
    };
    let mut values = vec![
        V::Map(vec![
            (
                V::Enum {
                    identity: "fixture::Key".into(),
                    variant: 0,
                    fields: vec![scalar(K::U64, 1 << 63), V::String("a\0b".into())],
                },
                V::List(vec![
                    packet(f64::NAN.to_bits()),
                    packet((-0.0f64).to_bits()),
                ]),
            ),
            (
                V::Enum {
                    identity: "fixture::Key".into(),
                    variant: 1,
                    fields: vec![],
                },
                V::List(vec![]),
            ),
        ]),
        V::Unit,
        V::Bytes(vec![]),
    ];
    let mut roots = vec![7, 8, 9];
    for kind in [
        K::I8,
        K::I16,
        K::I32,
        K::I64,
        K::U8,
        K::U16,
        K::U32,
        K::U64,
        K::F32,
        K::F64,
        K::Bool,
    ] {
        let bits = if kind.signed() {
            u64::MAX
        } else {
            match kind {
                K::Bool => 1,
                K::F32 => 0x7fc0_0123,
                K::F64 => 0x7ff8_0000_0000_0123,
                _ => u64::MAX >> (64 - kind.width()),
            }
        };
        roots.push(types.len());
        types.push(T::Scalar(kind));
        values.push(scalar(kind, bits));
    }
    let mut deep = V::String("deep\0".into());
    let mut top = 2;
    for _ in 0..512 {
        let index = types.len();
        types.push(T::List(top));
        top = index;
        deep = V::List(vec![deep]);
    }
    roots.push(top);
    values.push(deep);
    roots.push(types.len());
    types.push(T::Record {
        identity: "fixture::Wide".into(),
        fields: vec![0; 129],
    });
    values.push(V::Record {
        identity: "fixture::Wide".into(),
        fields: vec![scalar(K::U64, u64::MAX); 129],
    });
    let schema = Arc::new(ValueSchema::new(types.clone()).unwrap());
    let descriptor = Descriptor::new(schema, roots.clone()).unwrap();
    let exact = descriptor.preflight(&values).unwrap();
    let budget = TransportBudget::new(1 << 20, 1 << 20);
    let wire = descriptor
        .encode(
            &values,
            budget.reserve(TransportDirection::Input, exact).unwrap(),
        )
        .unwrap();
    let mut program = String::from(
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
static size_t live;
static void *tracked_calloc(size_t count,size_t width) { void *memory=calloc(count,width); if(memory) live++; return memory; }
static inline void *tracked_malloc(size_t width) { void *memory=malloc(width); if(memory) live++; return memory; }
static void tracked_free(void *memory) { if(memory) { assert(live); live--; free(memory); } }
#define calloc tracked_calloc
#define malloc tracked_malloc
#define free tracked_free
#include "rinzu/typed_transport.h"
#include "rinzu/managed_composite.h"
"#,
    );
    value_schema::emit_types(&mut program, &types, "types");
    let mut next = 0;
    let names = values
        .iter()
        .map(|value| emit_value(&mut program, value, &mut next))
        .collect::<Vec<_>>();
    program.push_str(&format!(
        "static const size_t roots[]={{{}}};\nstatic const rinzu_value_t values[]={{{}}};\n",
        roots
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(","),
        names.join(",")
    ));
    bytes(&mut program, "golden", wire.bytes());
    let mut bad = packet(0);
    let V::Record { fields, .. } = &mut bad else {
        unreachable!()
    };
    fields[3] = V::Bytes(vec![0, 255]);
    let bad = emit_value(&mut program, &bad, &mut next);
    program.push_str(&format!(
        "static const rinzu_value_t bad_values[]={{{bad}}};\n"
    ));
    program.push_str(r#"
static void rejected(rinzu_value_schema_t *schema,const size_t *indices,size_t count,const unsigned char *bytes,size_t size) {
  size_t before=live; rinzu_typed_packet_t packet={0};
  assert(rinzu_typed_decode(schema,indices,count,bytes,size,&packet));
  assert(!packet.values && !packet.count && !packet.allocations && !packet.native_allocations && !packet.native_bytes && live==before);
}
static void heap_copy(rinzu_value_schema_t *schema,const rinzu_value_t *values,unsigned char *wire,size_t size) {
  size_t count=sizeof(roots)/sizeof(*roots),before=live;
  rinzu_managed_data_t data={0},other={0}; rinzu_managed_roots_t frames={0},other_frames={0};
  assert(!rinzu_managed_configure_values(&data,schema) && !rinzu_managed_configure_values(&other,schema));
  rinzu_managed_slot_t *slots=rinzu_failure_allocate(count*sizeof(*slots)),*other_slots=rinzu_failure_allocate(count*sizeof(*slots));
  assert(!rinzu_managed_materialize_values(&data,roots,values,count,slots));
  assert(!rinzu_managed_materialize_values(&other,roots,values,count,other_slots));
  assert(slots[0].reference && other_slots[0].reference && slots[0].bits!=other_slots[0].bits);
  assert(rinzu_managed_slot_valid(&other,roots[0],slots[0])==RINZU_VALUE_TYPE);
  uint64_t *references=rinzu_failure_allocate(count*sizeof(*references)),*other_refs=rinzu_failure_allocate(count*sizeof(*references)); size_t n=0;
  for(size_t index=0;index<count;index++) if(slots[index].reference) { references[n]=slots[index].bits; other_refs[n++]=other_slots[index].bits; }
  rinzu_managed_enter(&frames); rinzu_managed_publish(&frames,references,n);
  rinzu_managed_enter(&other_frames); rinzu_managed_publish(&other_frames,other_refs,n);
  for(unsigned pass=0;pass<4;pass++) {
    rinzu_managed_collect(&data,&frames); assert(data.objects>512);
    rinzu_managed_snapshot_t *views=rinzu_failure_allocate(count*sizeof(*views)); rinzu_value_t *copied=rinzu_failure_allocate(count*sizeof(*copied));
    for(size_t index=0;index<count;index++) { assert(!rinzu_managed_snapshot(&data,roots[index],slots[index],&views[index])); copied[index]=views[index].value; }
    assert(!rinzu_typed_encode(schema,roots,count,copied,wire,size) && !memcmp(wire,golden,size));
    for(size_t index=0;index<count;index++) rinzu_managed_snapshot_dispose(&views[index]); free(copied); free(views);
  }
  size_t objects=data.objects; rinzu_managed_slot_t untouched={91,0}; const size_t record[]={4},bad[]={11};
  rinzu_value_t *bad_arguments=rinzu_failure_allocate(count*sizeof(*bad_arguments)); memcpy(bad_arguments,values,count*sizeof(*bad_arguments));
  bad_arguments[count-1].kind=RINZU_VALUE_BYTES;
  assert(rinzu_managed_materialize_values(&data,roots,bad_arguments,count,other_slots)==RINZU_VALUE_TYPE && data.objects==objects && other_slots[0].bits!=slots[0].bits);
  free(bad_arguments);
  assert(rinzu_managed_materialize_values(&data,record,bad_values,1,&untouched)==RINZU_VALUE_TYPE && untouched.bits==91 && data.objects==objects);
  const rinzu_value_t empty_bad={.kind=RINZU_VALUE_ENUM,.identity="fixture::Bad",.variant=0};
  assert(rinzu_managed_materialize_values(&data,bad,&empty_bad,1,&untouched)==RINZU_VALUE_NO_EQUALITY && untouched.bits==91 && data.objects==objects);
  rinzu_managed_publish(&frames,NULL,0); rinzu_managed_publish(&other_frames,NULL,0);
  rinzu_managed_collect(&data,&frames); rinzu_managed_collect(&other,&other_frames); assert(!data.objects && !other.objects);
  rinzu_managed_leave(&frames); rinzu_managed_leave(&other_frames); rinzu_managed_dispose(&frames); rinzu_managed_dispose(&other_frames);
  rinzu_managed_data_dispose(&data); rinzu_managed_data_dispose(&other);
  free(references); free(other_refs); free(slots); free(other_slots); assert(live==before);
}
int main(void) {
  rinzu_value_schema_t schema={0}; assert(!rinzu_value_schema_init(&schema,types,sizeof(types)/sizeof(*types)));
  size_t count=sizeof(roots)/sizeof(*roots),size=91,baseline=live;
  assert(!rinzu_typed_size(&schema,roots,count,values,&size) && size==sizeof(golden));
  unsigned char *wire=rinzu_failure_allocate(size); memset(wire,0x55,size);
  assert(rinzu_typed_encode(&schema,roots,count,values,wire,size-1)==RINZU_TYPED_LENGTH && wire[0]==0x55);
  assert(!rinzu_typed_encode(&schema,roots,count,values,wire,size) && !memcmp(wire,golden,size));
  rinzu_typed_packet_t packet={0}; assert(!rinzu_typed_decode(&schema,roots,count,wire,size,&packet));
  assert(packet.native_allocations>512 && packet.native_bytes>size && packet.count==count);
  memset(wire,0,size); /* Decoded payloads own their bytes. */
  assert(!rinzu_typed_encode(&schema,roots,count,packet.values,wire,size) && !memcmp(wire,golden,size));
  heap_copy(&schema,packet.values,wire,size);
  rinzu_typed_packet_dispose(&packet); assert(live==baseline+1 && !packet.native_allocations);
  for(size_t end=0;end<size;end++) rejected(&schema,roots,count,wire,end);
  unsigned char *longer=rinzu_failure_allocate(size+1); memcpy(longer,wire,size); longer[size]=0;
  rejected(&schema,roots,count,longer,size+1); free(longer);
  wire[16]^=1; rejected(&schema,roots,count,wire,size); wire[16]^=1;
  for(size_t offset=0;offset+3<size;offset++) if(wire[offset]==0 && wire[offset+1]==255 && wire[offset+2]==42) { wire[offset-8]=2; rejected(&schema,roots,count,wire,size); wire[offset-8]=3; break; }
  for(size_t offset=0;offset+3<size;offset++) if(wire[offset]==0xf0 && wire[offset+1]==0x9f) { wire[offset]=255; rejected(&schema,roots,count,wire,size); wire[offset]=0xf0; break; }
  const size_t record[]={4},bad[]={11},list[]={12},units[]={13},map[]={7}; unsigned char huge[8]; memset(huge,255,8);
  size=91; assert(rinzu_typed_size(&schema,record,1,bad_values,&size)==RINZU_TYPED_VALUE && size==91);
  assert(rinzu_typed_schema(&schema,bad,1)==RINZU_TYPED_SCHEMA);
  rejected(&schema,list,1,huge,8); rejected(&schema,units,1,huge,8); rejected(&schema,map,1,huge,8);
  const size_t boolean[]={24}; const unsigned char invalid_boolean[]={2}; rejected(&schema,boolean,1,invalid_boolean,1);
  rinzu_value_pair_t duplicate_entries[]={values[0].entries[0],values[0].entries[0]};
  rinzu_value_t duplicate={.kind=RINZU_VALUE_MAP,.count=2,.entries=duplicate_entries};
  size=91; assert(rinzu_typed_size(&schema,map,1,&duplicate,&size)==RINZU_TYPED_VALUE && size==91);
  /* Deliberately bypass producer validation to exercise duplicate wire input. */
  rinzu_typed_writer_t measure={0}; assert(!rinzu_typed_walk(&schema,map,1,&duplicate,&measure));
  unsigned char *duplicate_wire=rinzu_failure_allocate(measure.offset);
  rinzu_typed_writer_t writer={duplicate_wire,0,measure.offset}; assert(!rinzu_typed_walk(&schema,map,1,&duplicate,&writer));
  rejected(&schema,map,1,duplicate_wire,measure.offset); free(duplicate_wire);
  assert(!rinzu_typed_decode(&schema,NULL,0,NULL,0,&packet)); rinzu_typed_packet_dispose(&packet);
  free(wire); rinzu_value_schema_dispose(&schema); assert(!live);
  rinzu_value_type_t cycle={.kind=RINZU_VALUE_LIST,.element=0}; const size_t zero[]={0};
  assert(!rinzu_value_schema_init(&schema,&cycle,1)); assert(rinzu_typed_schema(&schema,zero,1)==RINZU_TYPED_SCHEMA);
  rinzu_value_schema_dispose(&schema); assert(!live); puts("typed wire and native baseline: ok");
}
"#);
    let directory =
        std::env::temp_dir().join(format!("rinzu-typed-transport-c-{}", std::process::id()));
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
        let run = Command::new(directory.join(name)).output().unwrap();
        assert!(
            run.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(
            String::from_utf8(run.stdout).unwrap(),
            "typed wire and native baseline: ok\n"
        );
    }
    drop(wire);
    assert_eq!(budget.snapshot().used_bytes, 0);
    std::fs::remove_dir_all(directory).unwrap();
}

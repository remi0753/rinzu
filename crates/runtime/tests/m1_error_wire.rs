use rinzu_runtime::{
    error_value::ErrorValue,
    failure::{Diagnostic, ErrorKind, Failure, FailureCause, SourcePosition},
    transport::{TransportBudget, TransportDirection, typed::Descriptor},
    value::{Value, ValueSchema, ValueType},
};
use std::{process::Command, sync::Arc};
#[test]
fn rust_and_c_error_wire_preserve_sources_causes_and_cleanup_without_equality() {
    let schema = Arc::new(ValueSchema::new(vec![ValueType::Error, ValueType::List(0)]).unwrap());
    let descriptor = Descriptor::new(schema, vec![1]).unwrap();
    let error = ErrorValue::new(1200, "日本語\0tail")
        .with_context("file".into())
        .with_cause(&ErrorValue::new(1201, "cause"))
        .unwrap();
    let failure = Failure {
        managed: Default::default(),
        traces: Default::default(),
        primary: Diagnostic {
            cause: FailureCause::Error(ErrorKind::Conversion),
            source: Some(SourcePosition {
                file: u64::MAX,
                start_byte: 3,
                end_byte: 5,
            }),
        },
        cleanup: vec![
            FailureCause::Error(ErrorKind::Io).into(),
            FailureCause::Error(ErrorKind::NativeRecordBusy).into(),
        ],
    };
    let values = [Value::List(vec![
        Value::Error(error),
        Value::Error(ErrorValue::from_failure(&failure)),
    ])];
    let exact = descriptor.preflight(&values).unwrap();
    let budget = TransportBudget::new(4096, 4096);
    let packet = descriptor
        .encode(
            &values,
            budget.reserve(TransportDirection::Input, exact).unwrap(),
        )
        .unwrap();
    let mut program = String::from(
        "#include <assert.h>\n#include <string.h>\n#include \"rinzu/managed_composite.h\"\n#include \"rinzu/typed_transport.h\"\n",
    );
    program.push_str(&format!(
        "static const unsigned char wire[]={{{}}};\n",
        packet
            .bytes()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",")
    ));
    program.push_str(r#"
int main(void) {
  rinzu_value_type_t types[]={{.kind=RINZU_VALUE_ERROR},{.kind=RINZU_VALUE_LIST,.element=0}};
  rinzu_value_schema_t schema={0};assert(!rinzu_value_schema_init(&schema,types,2));
  assert(!schema.equality[0] && !schema.equality[1] && schema.transferable[1]);
  const size_t roots[]={1};rinzu_typed_packet_t packet={0};
  for(size_t i=0;i<sizeof(wire);i++) {assert(rinzu_typed_decode(&schema,roots,1,wire,i,&packet));rinzu_typed_packet_dispose(&packet);}
  assert(!rinzu_typed_decode(&schema,roots,1,wire,sizeof(wire),&packet));
  rinzu_managed_data_t data={0};assert(!rinzu_managed_configure_values(&data,&schema));
  rinzu_managed_slot_t slot;assert(!rinzu_managed_materialize_values(&data,roots,packet.values,1,&slot));
  rinzu_managed_roots_t frames={0};rinzu_managed_enter(&frames);rinzu_managed_publish(&frames,&slot.bits,1);
  for(int i=0;i<4;i++) {
    rinzu_managed_collect(&data,&frames);rinzu_managed_snapshot_t view={0};assert(!rinzu_managed_snapshot(&data,1,slot,&view));
    unsigned char copy[sizeof(wire)];assert(!rinzu_typed_encode(&schema,roots,1,&view.value,copy,sizeof(copy)));assert(!memcmp(wire,copy,sizeof(wire)));
    rinzu_managed_snapshot_dispose(&view);
  }
  rinzu_managed_publish(&frames,NULL,0);rinzu_managed_collect(&data,&frames);assert(!data.objects && !data.live_views && !data.view_allocations);
  rinzu_managed_leave(&frames);rinzu_managed_dispose(&frames);rinzu_typed_packet_dispose(&packet);rinzu_value_schema_dispose(&schema);
}
"#);
    let dir = std::env::temp_dir().join(format!("rinzu-m1-error-wire-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("test.c"), program).unwrap();
    for (name, flags) in [
        ("plain", vec!["-O2", "-Wall", "-Wextra", "-Werror"]),
        ("san", vec!["-O1", "-g", "-fsanitize=address,undefined"]),
    ] {
        let compiled = Command::new("clang")
            .arg("-std=c11")
            .args(flags)
            .arg("-I")
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(dir.join("test.c"))
            .arg("-o")
            .arg(dir.join(name))
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let run = Command::new(dir.join(name))
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
    drop(packet);
    assert_eq!(budget.snapshot().used_bytes, 0);
}

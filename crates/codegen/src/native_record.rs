//! Bind target-validated SDK layouts to the same closed schema used by Owners.
use anyhow::{Result, anyhow, ensure};
use rinzu_bindgen::source_package::SourceDescriptor;
use rinzu_compiler::{IrModule, Type};
use rinzu_runtime::data::{NativeRecordField, NativeRecordLayout};
use std::collections::BTreeMap;

/// Descriptors must come from the validated source-package loader/build. Every
/// field is then checked against the closed source/Owner schema before binding.
pub fn source_native_record_layouts(
    module: &IrModule,
    descriptors: &[SourceDescriptor],
) -> Result<BTreeMap<String, NativeRecordLayout>> {
    let schema = crate::data_schema::DataSchema::new(module)?;
    let mut layouts = BTreeMap::new();
    for (identity, expected) in &module.source_types {
        if expected.kind != "native-record" {
            continue;
        }
        let record = rinzu_bindgen::source_package::matching_record_layout(
            descriptors
                .iter()
                .flat_map(|descriptor| &descriptor.interface.types),
            identity,
        )
        .map_err(|error| anyhow!(error))?;
        ensure!(
            record.canonical == *identity
                && record.bytes_length.is_none()
                && record.fields.len() == expected.fields.len(),
            "native record identity/field count mismatch: {identity}"
        );
        let mut fields = Vec::new();
        for (actual, (name, ty)) in record.fields.iter().zip(&expected.fields) {
            let scalar = ty
                .scalar()
                .ok_or_else(|| anyhow!("non-scalar native record field: {identity}.{name}"))?;
            ensure!(
                actual.name == *name && actual.c_type == scalar.c_type(),
                "native record field order/type mismatch: {identity}.{name}"
            );
            fields.push(NativeRecordField {
                kind: crate::scalar_backend::kind(scalar),
                offset: actual.offset.parse()?,
            });
        }
        let layout = NativeRecordLayout::new(
            schema.index(&Type::Named(identity.clone()))?,
            record.size.parse()?,
            record.alignment.parse()?,
            fields,
        )
        .map_err(|error| anyhow!("invalid target native record layout {identity}: {error:?}"))?;
        layout.validate_schema(&schema.value).map_err(|error| {
            anyhow!("native record Owner schema mismatch {identity}: {error:?}")
        })?;
        ensure!(
            layouts.insert(identity.clone(), layout).is_none(),
            "duplicate native record identity"
        );
    }
    Ok(layouts)
}

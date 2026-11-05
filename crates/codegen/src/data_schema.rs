//! One closed typed catalog is shared by executable lowering and its Owner heap.
use anyhow::{Result, anyhow, bail};
use rinzu_compiler::{IrModule, Type};
use rinzu_runtime::data::display::{DisplaySchema, DisplayType};
use rinzu_runtime::data::{DataError, ManagedData, ManagedHandle, ManagedSlot};
use rinzu_runtime::heap::DomainHeap;
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::value::{ValueSchema, ValueType};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct DataSchema {
    pub value: Arc<ValueSchema>,
    pub display: Arc<DisplaySchema>,
    types: Vec<Type>,
    indices: HashMap<Type, usize>,
}

pub(crate) fn canonical(ty: &Type) -> Type {
    if let Some(scalar) = ty.scalar() {
        return Type::from_scalar(scalar);
    }
    match ty {
        Type::List(element) => Type::List(Box::new(canonical(element))),
        Type::Map(key, value) => Type::Map(Box::new(canonical(key)), Box::new(canonical(value))),
        Type::Future(value) => Type::Future(Box::new(canonical(value))),
        Type::FunctionReference(signature) => {
            let mut signature = (**signature).clone();
            signature.parameters = signature.parameters.iter().map(canonical).collect();
            signature.result = Box::new(canonical(&signature.result));
            Type::FunctionReference(Box::new(signature))
        }
        Type::StaticHandler(signature) => {
            let mut signature = (**signature).clone();
            signature.parameters = signature.parameters.iter().map(canonical).collect();
            signature.result = Box::new(canonical(&signature.result));
            Type::StaticHandler(Box::new(signature))
        }
        _ => ty.clone(),
    }
}

// Keep the canonical schema identity private while exposing source-level
// field types in markers such as Future<List<{x: Int}>>.
fn display_type_name(module: &IrModule, ty: &Type) -> String {
    match ty {
        Type::Named(name) if name.starts_with("$rinzu_record_") => module
            .source_types
            .get(name)
            .map(|schema| {
                let fields = schema
                    .fields
                    .iter()
                    .map(|(field, ty)| format!("{field}: {}", display_type_name(module, ty)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{fields}}}")
            })
            .unwrap_or_else(|| name.clone()),
        Type::List(element) => format!("List<{}>", display_type_name(module, element)),
        Type::Map(key, value) => format!(
            "Map<{}, {}>",
            display_type_name(module, key),
            display_type_name(module, value)
        ),
        Type::Future(value) => format!("Future<{}>", display_type_name(module, value)),
        Type::FunctionReference(signature) => {
            let parameters = signature
                .parameters
                .iter()
                .map(|ty| display_type_name(module, ty))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{}fn({parameters})->{}",
                if signature.supplant { "supplant " } else { "" },
                display_type_name(module, &signature.result)
            )
        }
        _ => ty.to_string(),
    }
}

impl DataSchema {
    pub fn collection_overflow(
        &self,
        data: &mut ManagedData,
        heap: &mut DomainHeap,
        operation: rinzu_compiler::DataOperation,
        current: i64,
        added: i64,
    ) -> Result<ManagedHandle, DataError> {
        let identity = operation
            .length_overflow()
            .ok_or(DataError::InvalidEncoding)?;
        let ty = *self
            .indices
            .get(&Type::Named(identity.into()))
            .ok_or(DataError::InvalidEncoding)?;
        let ValueType::ErrorRecord { fields, .. } = self.value.ty(ty)? else {
            return Err(DataError::WrongKind);
        };
        let [_, _, source, cause, payload] = fields.as_slice() else {
            return Err(DataError::InvalidEncoding);
        };
        let int = |value: i64| {
            ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, value as u64).unwrap())
        };
        // Structural fields are in canonical name order.
        let payload = data.create_composite(heap, *payload, None, &[int(added), int(current)])?;
        let name = data.create_string(heap, "LengthOverflow")?;
        let message = data.create_string(heap, "collection length exceeds Int")?;
        let source = data.create_composite(heap, *source, Some(0), &[])?;
        let cause = data.create_composite(heap, *cause, Some(0), &[])?;
        let children = [name, message, source, cause, payload];
        let result = data.create_composite(heap, ty, None, &children.map(ManagedSlot::Reference));
        for child in children {
            data.release(heap, child)?;
        }
        result
    }

    pub fn new(module: &IrModule) -> Result<Self> {
        let mut types = Vec::new();
        let mut indices = HashMap::new();
        let intern = |ty: &Type, types: &mut Vec<Type>, indices: &mut HashMap<Type, usize>| {
            let ty = canonical(ty);
            *indices.entry(ty.clone()).or_insert_with(|| {
                let index = types.len();
                types.push(ty);
                index
            })
        };
        intern(&Type::Unit, &mut types, &mut indices);
        for name in module.source_types.keys() {
            intern(&Type::Named(name.clone()), &mut types, &mut indices);
        }
        for function in &module.functions {
            for ty in function
                .parameter_types
                .iter()
                .chain(std::iter::once(&function.result_type))
                .chain(
                    function
                        .blocks
                        .iter()
                        .flat_map(|block| &block.operations)
                        .filter_map(|operation| operation.result.as_ref().map(|result| &result.ty)),
                )
            {
                intern(ty, &mut types, &mut indices);
            }
        }
        let mut descriptors = Vec::new();
        while descriptors.len() < types.len() {
            let ty = types[descriptors.len()].clone();
            let descriptor = if let Some(scalar) = ty.scalar() {
                ValueType::Scalar(crate::scalar_backend::kind(scalar))
            } else {
                match &ty {
                    Type::Unit => ValueType::Unit,
                    Type::String => ValueType::String,
                    Type::FixedBytes(length) => ValueType::Bytes(*length),
                    Type::List(element) => {
                        ValueType::List(intern(element, &mut types, &mut indices))
                    }
                    Type::Map(key, value) => ValueType::Map {
                        key: intern(key, &mut types, &mut indices),
                        value: intern(value, &mut types, &mut indices),
                    },
                    Type::Named(name) => {
                        let schema = module
                            .source_types
                            .get(name)
                            .ok_or_else(|| anyhow!("missing nominal data schema {name}"))?;
                        match schema.kind.as_str() {
                            "record" => ValueType::Record {
                                identity: name.clone(),
                                fields: schema
                                    .fields
                                    .iter()
                                    .map(|(_, ty)| intern(ty, &mut types, &mut indices))
                                    .collect(),
                            },
                            "state" | "native-record" => ValueType::State {
                                identity: name.clone(),
                                fields: schema
                                    .fields
                                    .iter()
                                    .map(|(_, ty)| intern(ty, &mut types, &mut indices))
                                    .collect(),
                            },
                            "error" => ValueType::ErrorRecord {
                                identity: name.clone(),
                                fields: schema
                                    .fields
                                    .iter()
                                    .map(|(_, ty)| intern(ty, &mut types, &mut indices))
                                    .collect(),
                            },
                            "enum" => ValueType::Enum {
                                identity: name.clone(),
                                variants: schema
                                    .variants
                                    .iter()
                                    .map(|(_, fields)| {
                                        fields
                                            .iter()
                                            .map(|(_, ty)| intern(ty, &mut types, &mut indices))
                                            .collect()
                                    })
                                    .collect(),
                            },
                            "opaque" => ValueType::Other,
                            kind => bail!("unknown nominal schema kind {kind}"),
                        }
                    }
                    Type::Bytes => ValueType::DynamicBytes,
                    Type::Error => ValueType::Error,
                    _ => ValueType::Other,
                }
            };
            descriptors.push(descriptor);
        }
        let value = Arc::new(
            ValueSchema::new(descriptors)
                .map_err(|error| anyhow!("invalid closed data schema: {error:?}"))?,
        );
        let display = Arc::new(DisplaySchema(
            types
                .iter()
                .enumerate()
                .map(
                    |(index, ty)| match value.ty(index).expect("closed display type") {
                        ValueType::Scalar(kind) => DisplayType::Scalar(*kind),
                        ValueType::Unit => DisplayType::Unit,
                        ValueType::Error | ValueType::ErrorRecord { .. } => DisplayType::Error,
                        ValueType::String => DisplayType::String,
                        ValueType::Bytes(_) | ValueType::DynamicBytes => DisplayType::Bytes,
                        ValueType::Record { identity, fields }
                        | ValueType::State { identity, fields }
                            if module.source_types[identity].kind != "state" =>
                        {
                            DisplayType::Record {
                                name: identity.clone(),
                                fields: module.source_types[identity]
                                    .fields
                                    .iter()
                                    .zip(fields)
                                    .map(|((name, _), ty)| (name.clone(), *ty))
                                    .collect(),
                            }
                        }
                        ValueType::Enum { identity, variants } => DisplayType::Enum {
                            name: identity.clone(),
                            variants: module.source_types[identity]
                                .variants
                                .iter()
                                .zip(variants)
                                .map(|((name, fields), types)| {
                                    (
                                        name.clone(),
                                        fields
                                            .iter()
                                            .zip(types)
                                            .map(|((name, _), ty)| (name.clone(), *ty))
                                            .collect(),
                                    )
                                })
                                .collect(),
                        },
                        ValueType::List(element) => DisplayType::List(*element),
                        ValueType::Map { key, value } => DisplayType::Map {
                            key: *key,
                            value: *value,
                        },
                        ValueType::Record { .. } => unreachable!("record display schema"),
                        ValueType::State { identity, .. } => {
                            DisplayType::Marker(if identity.starts_with("$rinzu_closure_") {
                                "<function>".into()
                            } else {
                                format!("<class {identity}>")
                            })
                        }
                        ValueType::Other => match ty {
                            Type::Error => DisplayType::Error,
                            Type::FunctionReference(_) | Type::StaticHandler(_) => {
                                DisplayType::Marker("<function>".into())
                            }
                            Type::Native { name, .. } | Type::Named(name) => {
                                DisplayType::Marker(format!("<{name}>"))
                            }
                            _ => {
                                DisplayType::Marker(format!("<{}>", display_type_name(module, ty)))
                            }
                        },
                    },
                )
                .collect(),
        ));
        Ok(Self {
            value,
            display,
            types,
            indices,
        })
    }

    pub fn index(&self, ty: &Type) -> Result<usize> {
        // Already canonical nominal types need no temporary String allocation
        // when a preallocated native result is handed to its source slot.
        if let Some(index) = self.indices.get(ty) {
            return Ok(*index);
        }
        self.indices
            .get(&canonical(ty))
            .copied()
            .ok_or_else(|| anyhow!("type {ty} is absent from the closed data schema"))
    }

    pub fn metadata(&self) -> Vec<crate::baseline::AotDataType> {
        self.types
            .iter()
            .enumerate()
            .map(|(index, ty)| crate::baseline::AotDataType {
                ty: ty.clone(),
                index,
                managed: self.is_reference(index),
            })
            .collect()
    }

    pub fn is_reference(&self, index: usize) -> bool {
        matches!(
            &self.types[index],
            Type::String
                | Type::Bytes
                | Type::FixedBytes(_)
                | Type::Named(_)
                | Type::List(_)
                | Type::Map(_, _)
                | Type::Error
                | Type::Native {
                    call_only: false,
                    ..
                }
        )
    }

    pub fn slot(&self, index: usize, bits: u64) -> Result<ManagedSlot, DataError> {
        let ty = self.value.ty(index)?;
        if self.is_reference(index) {
            return ManagedHandle::from_bits(bits)
                .map(ManagedSlot::Reference)
                .ok_or(DataError::InvalidEncoding);
        }
        match ty {
            ValueType::Scalar(kind) => ScalarValue::from_bits(*kind, bits)
                .map(ManagedSlot::Scalar)
                .map_err(|_| DataError::InvalidEncoding),
            ValueType::Unit if bits == 0 => Ok(ManagedSlot::Unit),
            ValueType::Other => Ok(ManagedSlot::Opaque(bits)),
            _ => Err(DataError::InvalidEncoding),
        }
    }

    pub fn child_types(
        &self,
        index: usize,
        variant: Option<u32>,
        count: usize,
    ) -> Result<Vec<usize>, DataError> {
        let fields = match self.value.ty(index)? {
            ValueType::Record { fields, .. }
            | ValueType::State { fields, .. }
            | ValueType::ErrorRecord { fields, .. }
                if variant.is_none() =>
            {
                fields.clone()
            }
            ValueType::Enum { variants, .. } => variants
                .get(variant.ok_or(DataError::InvalidEncoding)? as usize)
                .cloned()
                .ok_or(DataError::InvalidEncoding)?,
            ValueType::List(element) if variant.is_none() => vec![*element; count],
            ValueType::Map { key, value } if variant.is_none() && count.is_multiple_of(2) => (0
                ..count)
                .map(|field| if field % 2 == 0 { *key } else { *value })
                .collect(),
            _ => return Err(DataError::InvalidEncoding),
        };
        if fields.len() != count {
            return Err(DataError::InvalidEncoding);
        }
        Ok(fields)
    }

    pub fn c_source(&self) -> String {
        let mut source = String::new();
        for op in rinzu_compiler::DataOperation::ALL {
            source.push_str(&format!("enum {{ {} = {} }};\n", op.c_name(), op as u32));
        }
        for (name, op) in [
            ("list", rinzu_compiler::DataOperation::ListAppend),
            ("map", rinzu_compiler::DataOperation::MapInsert),
        ] {
            let ty = Type::Named(op.length_overflow().unwrap().into());
            let index = self
                .indices
                .get(&ty)
                .map_or("SIZE_MAX".into(), usize::to_string);
            source.push_str(&format!(
                "#define rinzu_{name}_length_overflow ((size_t){index})\n"
            ));
        }
        for index in 0..self.types.len() {
            let descriptor = self.value.ty(index).expect("closed type index");
            let groups = match descriptor {
                ValueType::Record { fields, .. }
                | ValueType::State { fields, .. }
                | ValueType::ErrorRecord { fields, .. } => vec![fields],
                ValueType::Enum { variants, .. } => variants.iter().collect(),
                _ => Vec::new(),
            };
            for (group, fields) in groups.into_iter().enumerate() {
                if !fields.is_empty() {
                    source.push_str(&format!(
                        "static const size_t rinzu_data_fields_{index}_{group}[]={{{}}};\n",
                        fields
                            .iter()
                            .map(usize::to_string)
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                }
            }
            if let ValueType::Enum { variants, .. } = descriptor
                && !variants.is_empty()
            {
                source.push_str(&format!(
                    "static const rinzu_key_variant_t rinzu_data_variants_{index}[]={{"
                ));
                for (group, fields) in variants.iter().enumerate() {
                    source.push_str(&format!(
                        "{{{}, {}}},",
                        fields.len(),
                        if fields.is_empty() {
                            "NULL".into()
                        } else {
                            format!("rinzu_data_fields_{index}_{group}")
                        }
                    ));
                }
                source.push_str("};\n");
            }
        }
        source.push_str("static const rinzu_value_type_t rinzu_program_types[]={\n");
        for index in 0..self.types.len() {
            let descriptor = self.value.ty(index).expect("closed type index");
            let entry = match descriptor {
                ValueType::Scalar(kind) => {
                    format!(".kind=RINZU_VALUE_SCALAR,.scalar_kind={}", *kind as u32)
                }
                ValueType::Unit => ".kind=RINZU_VALUE_UNIT".into(),
                ValueType::String => ".kind=RINZU_VALUE_STRING".into(),
                ValueType::Error => ".kind=RINZU_VALUE_ERROR".into(),
                ValueType::DynamicBytes => ".kind=RINZU_VALUE_DYNAMIC_BYTES".into(),
                ValueType::Bytes(length) => {
                    format!(".kind=RINZU_VALUE_BYTES,.bytes_length=UINT64_C({length})")
                }
                ValueType::State { identity, fields }
                | ValueType::ErrorRecord { identity, fields } => format!(
                    ".kind={},.identity={},.field_count={},.fields={}",
                    if matches!(descriptor, ValueType::ErrorRecord { .. }) {
                        "RINZU_VALUE_ERROR_RECORD"
                    } else {
                        "RINZU_VALUE_STATE"
                    },
                    c_string(identity),
                    fields.len(),
                    if fields.is_empty() {
                        "NULL".into()
                    } else {
                        format!("rinzu_data_fields_{index}_0")
                    }
                ),
                ValueType::Record { identity, fields } => format!(
                    ".kind=RINZU_VALUE_RECORD,.identity={},.field_count={},.fields={}",
                    c_string(identity),
                    fields.len(),
                    if fields.is_empty() {
                        "NULL".into()
                    } else {
                        format!("rinzu_data_fields_{index}_0")
                    }
                ),
                ValueType::Enum { identity, variants } => format!(
                    ".kind=RINZU_VALUE_ENUM,.identity={},.variant_count={},.variants={}",
                    c_string(identity),
                    variants.len(),
                    if variants.is_empty() {
                        "NULL".into()
                    } else {
                        format!("rinzu_data_variants_{index}")
                    }
                ),
                ValueType::List(element) => format!(".kind=RINZU_VALUE_LIST,.element={element}"),
                ValueType::Map { key, value } => {
                    format!(".kind=RINZU_VALUE_MAP,.key={key},.value={value}")
                }
                ValueType::Other if matches!(self.types[index], Type::Future(_)) => {
                    ".kind=RINZU_VALUE_OTHER,.scalar_kind=RINZU_VALUE_OTHER_FUTURE".into()
                }
                ValueType::Other => ".kind=RINZU_VALUE_OTHER".into(),
            };
            source.push_str(&format!("{{{entry}}},\n"));
        }
        source.push_str("};\nstatic const unsigned char rinzu_program_slot_references[]={");
        source.push_str(
            &(0..self.types.len())
                .map(|index| u8::from(self.is_reference(index)).to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
        source.push_str("};\n");
        source.push_str(&self.display_c_source());
        source
    }

    pub fn future_types(&self) -> std::collections::BTreeSet<usize> {
        self.types
            .iter()
            .enumerate()
            .filter_map(|(index, ty)| matches!(ty, Type::Future(_)).then_some(index))
            .collect()
    }

    fn display_c_source(&self) -> String {
        let mut source = String::from("#include \"rinzu/display.h\"\n");
        for (index, ty) in self.display.0.iter().enumerate() {
            let groups: Vec<&Vec<(String, usize)>> = match ty {
                DisplayType::Record { fields, .. } => vec![fields],
                DisplayType::Enum { variants, .. } => {
                    variants.iter().map(|(_, fields)| fields).collect()
                }
                _ => vec![],
            };
            for (group, fields) in groups.iter().enumerate() {
                if fields.is_empty() {
                    continue;
                }
                source.push_str(&format!("static const rinzu_display_field_t rinzu_display_fields_{index}_{group}[]={{{}}};\n",
                    fields.iter().map(|(name,ty)| format!("{{{},{ty}}}",c_display_text(name))).collect::<Vec<_>>().join(",")));
            }
            if let DisplayType::Enum { variants, .. } = ty
                && !variants.is_empty()
            {
                source.push_str(&format!("static const rinzu_display_variant_t rinzu_display_variants_{index}[]={{{}}};\n",
                    variants.iter().enumerate().map(|(group,(name,fields))| format!("{{{},{},{}}}",c_display_text(name),
                        if fields.is_empty() {"NULL".into()} else {format!("rinzu_display_fields_{index}_{group}")},fields.len())).collect::<Vec<_>>().join(",")));
            }
        }
        source.push_str("static const rinzu_display_type_t rinzu_program_display[]={\n");
        for (index, ty) in self.display.0.iter().enumerate() {
            let entry = match ty {
                DisplayType::Scalar(kind) => {
                    format!(".kind=RINZU_DISPLAY_SCALAR,.scalar={}", *kind as u32)
                }
                DisplayType::Unit => ".kind=RINZU_DISPLAY_UNIT".into(),
                DisplayType::String => ".kind=RINZU_DISPLAY_STRING".into(),
                DisplayType::Bytes => ".kind=RINZU_DISPLAY_BYTES".into(),
                DisplayType::Record { name, fields } => format!(
                    ".kind=RINZU_DISPLAY_RECORD,.name={},.count={},.fields={}",
                    c_display_text(name),
                    fields.len(),
                    if fields.is_empty() {
                        "NULL".into()
                    } else {
                        format!("rinzu_display_fields_{index}_0")
                    }
                ),
                DisplayType::Enum { name, variants } => format!(
                    ".kind=RINZU_DISPLAY_ENUM,.name={},.count={},.variants={}",
                    c_display_text(name),
                    variants.len(),
                    if variants.is_empty() {
                        "NULL".into()
                    } else {
                        format!("rinzu_display_variants_{index}")
                    }
                ),
                DisplayType::List(element) => {
                    format!(".kind=RINZU_DISPLAY_LIST,.element={element}")
                }
                DisplayType::Map { key, value } => {
                    format!(".kind=RINZU_DISPLAY_MAP,.key={key},.value={value}")
                }
                DisplayType::Marker(text) => {
                    format!(".kind=RINZU_DISPLAY_MARKER,.name={}", c_display_text(text))
                }
                DisplayType::Error => ".kind=RINZU_DISPLAY_ERROR".into(),
            };
            source.push_str(&format!("{{{entry}}},\n"));
        }
        source.push_str("};\n");
        source
    }
}

fn c_display_text(text: &str) -> String {
    format!(
        "{{(const unsigned char *){},{}}}",
        c_string(text),
        text.len()
    )
}

pub(crate) fn c_string(text: &str) -> String {
    format!(
        "\"{}\"",
        text.as_bytes()
            .iter()
            .map(|byte| format!("\\x{byte:02x}"))
            .collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinzu_compiler::{FileId, SourceFile, compile_source};
    #[test]
    fn catalog_closes_nominal_cycles_aliases_lengths_and_unused_variant_properties() {
        let artifact = compile_source(&SourceFile::new(FileId(1), "schema.rinzu", "native {record NativePoint {value:CSize}} class Local { high: CSize } class Packet { const high: CSize, const data: Bytes(3), const text: String } enum Key { item(high: CUInt64, text: String), empty } enum Bad { unused(value: Float), empty } class Box { const value: Error } class Cycle { const values: List<Cycle> } fn main() { let maps: Map<Key, Packet> = {}; let a: Bytes(0) = b\"\"; let b: Bytes(3) = b\"abc\"; }")).unwrap();
        let schema = DataSchema::new(&artifact.ir).unwrap();
        assert_eq!(
            schema.index(&Type::Int).unwrap_err().to_string(),
            "type Int is absent from the closed data schema"
        );
        assert_eq!(
            schema
                .index(&Type::Scalar(rinzu_compiler::AbiScalar::U64))
                .unwrap(),
            schema
                .index(&Type::from_scalar(rinzu_compiler::AbiScalar::U64))
                .unwrap()
        );
        assert_ne!(
            schema.index(&Type::FixedBytes(0)).unwrap(),
            schema.index(&Type::FixedBytes(3)).unwrap()
        );
        for (name, equality, key) in [
            ("Packet", true, true),
            ("NativePoint", false, false),
            ("Key", true, true),
            ("Bad", true, false),
            ("Box", false, false),
            ("Cycle", false, false),
            ("Local", false, false),
        ] {
            let index = schema.index(&Type::Named(name.into())).unwrap();
            assert_eq!(
                schema.value.has_equality(index).unwrap(),
                equality,
                "{name}"
            );
            assert_eq!(schema.value.is_key(index).unwrap(), key, "{name}");
            assert!(schema.is_reference(index));
        }
        let repeat = DataSchema::new(&artifact.ir).unwrap();
        assert_eq!(schema.c_source(), repeat.c_source());
        assert_eq!(schema.types, repeat.types);
        let mut c =
            String::from("#include <assert.h>\n#include <stdio.h>\n#include \"rinzu/value.h\"\n");
        c.push_str(&schema.c_source());
        c.push_str("int main(void) { assert(sizeof rinzu_program_display / sizeof *rinzu_program_display == sizeof rinzu_program_types / sizeof *rinzu_program_types); rinzu_value_schema_t schema={0}; assert(!rinzu_value_schema_init(&schema,rinzu_program_types,sizeof(rinzu_program_types)/sizeof(*rinzu_program_types))); for(size_t index=0;index<schema.count;++index) { assert(rinzu_program_display[index].kind == schema.types[index].kind || schema.types[index].kind == RINZU_VALUE_OTHER || (schema.types[index].kind == RINZU_VALUE_STATE && (rinzu_program_display[index].kind == RINZU_DISPLAY_MARKER || rinzu_program_display[index].kind == RINZU_DISPLAY_RECORD)) || (schema.types[index].kind == RINZU_VALUE_ERROR && rinzu_program_display[index].kind == RINZU_DISPLAY_ERROR) || (schema.types[index].kind == RINZU_VALUE_DYNAMIC_BYTES && rinzu_program_display[index].kind == RINZU_DISPLAY_BYTES)); printf(\"%u:%u:%u:%u\\n\",schema.types[index].kind,schema.equality[index],schema.keys.allowed[index],rinzu_program_slot_references[index]); } rinzu_value_schema_dispose(&schema); return 0; }\n");
        let directory =
            std::env::temp_dir().join(format!("rinzu-typed-catalog-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("test.c"), c).unwrap();
        let compiled = std::process::Command::new("clang")
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(directory.join("test.c"))
            .arg("-o")
            .arg(directory.join("test"))
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let run = std::process::Command::new(directory.join("test"))
            .output()
            .unwrap();
        assert!(run.status.success());
        let expected = (0..schema.types.len())
            .map(|index| {
                let kind = match schema.value.ty(index).unwrap() {
                    ValueType::Scalar(_) => 0,
                    ValueType::Unit => 1,
                    ValueType::String => 2,
                    ValueType::Bytes(_) => 3,
                    ValueType::Record { .. } => 4,
                    ValueType::Enum { .. } => 5,
                    ValueType::List(_) => 6,
                    ValueType::Map { .. } => 7,
                    ValueType::Other => 8,
                    ValueType::DynamicBytes => 9,
                    ValueType::Error => 10,
                    ValueType::State { .. } => 11,
                    ValueType::ErrorRecord { .. } => 12,
                };
                format!(
                    "{kind}:{}:{}:{}\n",
                    u8::from(schema.value.has_equality(index).unwrap()),
                    u8::from(schema.value.is_key(index).unwrap()),
                    u8::from(schema.is_reference(index))
                )
            })
            .collect::<String>();
        assert_eq!(String::from_utf8(run.stdout).unwrap(), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn missing_nominal_and_non_key_map_schema_fail_before_lowering() {
        let mut artifact = compile_source(&SourceFile::new(FileId(1), "schema.rinzu", "enum Key { item(value: Int) } class R { const key: Key } fn main() { let values: Map<Key, R> = {}; }")).unwrap();
        artifact.ir.source_types.get_mut("Key").unwrap().variants[0].1[0].1 = Type::Float;
        assert!(DataSchema::new(&artifact.ir).is_err());
        artifact.ir.source_types.remove("Key");
        assert!(DataSchema::new(&artifact.ir).is_err());
    }
}

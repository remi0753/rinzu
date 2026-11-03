use rinzu_runtime::value::ValueType as T;

pub fn emit_types(program: &mut String, types: &[T], name: &str) {
    for (index, ty) in types.iter().enumerate() {
        let groups = match ty {
            T::Record { fields, .. } | T::State { fields, .. } | T::ErrorRecord { fields, .. } => {
                vec![fields]
            }
            T::Enum { variants, .. } => variants.iter().collect(),
            _ => vec![],
        };
        for (group, fields) in groups.iter().enumerate() {
            if !fields.is_empty() {
                program.push_str(&format!(
                    "static const size_t {name}_{index}_{group}[]={{{}}};\n",
                    fields
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
        }
        if let T::Enum { variants, .. } = ty
            && !variants.is_empty()
        {
            program.push_str(&format!(
                "static const rinzu_key_variant_t {name}_{index}_variants[]={{"
            ));
            for (group, fields) in variants.iter().enumerate() {
                program.push_str(&format!(
                    "{{{}, {}}},",
                    fields.len(),
                    if fields.is_empty() {
                        "NULL".into()
                    } else {
                        format!("{name}_{index}_{group}")
                    }
                ));
            }
            program.push_str("};\n");
        }
    }
    program.push_str(&format!("static const rinzu_value_type_t {name}[]={{"));
    for (index, ty) in types.iter().enumerate() {
        let entry = match ty {
            T::Scalar(kind) => format!(".kind=RINZU_VALUE_SCALAR,.scalar_kind={}", *kind as u32),
            T::Unit => ".kind=RINZU_VALUE_UNIT".into(),
            T::String => ".kind=RINZU_VALUE_STRING".into(),
            T::Bytes(length) => format!(".kind=RINZU_VALUE_BYTES,.bytes_length=UINT64_C({length})"),
            T::State { identity, fields } | T::ErrorRecord { identity, fields } => format!(
                ".kind={},.identity=\"{identity}\",.field_count={},.fields={}",
                if matches!(ty, T::ErrorRecord { .. }) {
                    "RINZU_VALUE_ERROR_RECORD"
                } else {
                    "RINZU_VALUE_STATE"
                },
                fields.len(),
                if fields.is_empty() {
                    "NULL".into()
                } else {
                    format!("{name}_{index}_0")
                }
            ),
            T::Record { identity, fields } => format!(
                ".kind=RINZU_VALUE_RECORD,.identity=\"{identity}\",.field_count={},.fields={}",
                fields.len(),
                if fields.is_empty() {
                    "NULL".into()
                } else {
                    format!("{name}_{index}_0")
                }
            ),
            T::Enum { identity, variants } => format!(
                ".kind=RINZU_VALUE_ENUM,.identity=\"{identity}\",.variant_count={},.variants={}",
                variants.len(),
                if variants.is_empty() {
                    "NULL".into()
                } else {
                    format!("{name}_{index}_variants")
                }
            ),
            T::List(element) => format!(".kind=RINZU_VALUE_LIST,.element={element}"),
            T::Map { key, value } => format!(".kind=RINZU_VALUE_MAP,.key={key},.value={value}"),
            T::Other => ".kind=RINZU_VALUE_OTHER".into(),
            T::DynamicBytes => ".kind=RINZU_VALUE_DYNAMIC_BYTES".into(),
            T::Error => ".kind=RINZU_VALUE_ERROR".into(),
        };
        program.push_str(&format!("{{{entry}}},"));
    }
    program.push_str("};\n");
}

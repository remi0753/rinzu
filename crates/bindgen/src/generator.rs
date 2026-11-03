use std::fmt;

use serde::Serialize;

use crate::{BindingMetadata, CallbackClass, ValidatedMetadata};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedBinding {
    pub file_stem: String,
    pub raw_header: String,
    pub safe_header: String,
    pub trampoline_source: String,
    pub conformance_source: String,
    pub descriptor_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationError {
    pub symbol: String,
    pub message: String,
}

impl fmt::Display for GenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.symbol, self.message)
    }
}

impl std::error::Error for GenerationError {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SafeDescriptor<'a> {
    schema: &'static str,
    version: u32,
    library: &'a str,
    functions: Vec<SafeFunctionDescriptor<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SafeFunctionDescriptor<'a> {
    name: String,
    native_symbol: &'a str,
    callback_count: usize,
    raw_only: bool,
}

/// Generates the v0.2 binding artifact set from an ordinary C header and
/// already validated safety metadata. Macros, variadics and inline function
/// definitions deliberately remain outside this minimal parser.
pub fn generate_binding(
    header_source: &str,
    metadata: &ValidatedMetadata,
) -> Result<GeneratedBinding, Vec<GenerationError>> {
    let metadata = metadata.get();
    let mut errors = Vec::new();
    for function in &metadata.functions {
        let Some(declaration) = find_prototype(header_source, &function.symbol) else {
            errors.push(GenerationError {
                symbol: function.symbol.clone(),
                message: "matching C function prototype was not found".into(),
            });
            continue;
        };
        if declaration.contains("...") {
            errors.push(GenerationError {
                symbol: function.symbol.clone(),
                message: "variadic C functions are RawOnly".into(),
            });
        }
        let symbol_index = declaration
            .find(&function.symbol)
            .expect("find_prototype found the symbol");
        if !declaration[..symbol_index].contains(function.result_c_type.trim()) {
            errors.push(GenerationError {
                symbol: function.symbol.clone(),
                message: format!(
                    "header result does not contain metadata type `{}`",
                    function.result_c_type
                ),
            });
        }
        for parameter in &function.parameters {
            if !declaration.contains(&parameter.name) || !declaration.contains(&parameter.c_type) {
                errors.push(GenerationError {
                    symbol: function.symbol.clone(),
                    message: format!(
                        "header parameter `{}` does not match `{}`",
                        parameter.name, parameter.c_type
                    ),
                });
            }
        }
        if function.callbacks.len() > 1 {
            errors.push(GenerationError {
                symbol: function.symbol.clone(),
                message: "the v0.2 generator supports one callback parameter per function".into(),
            });
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let descriptor = SafeDescriptor {
        schema: "rinzu.generated-safe-binding",
        version: 1,
        library: &metadata.library,
        functions: metadata
            .functions
            .iter()
            .map(|function| SafeFunctionDescriptor {
                name: format!("rinzu.{}.{}", metadata.library, function.name),
                native_symbol: &function.symbol,
                callback_count: function.callbacks.len(),
                raw_only: false,
            })
            .collect(),
    };
    let descriptor_json = serde_json::to_string_pretty(&descriptor).map_err(|error| {
        vec![GenerationError {
            symbol: metadata.library.clone(),
            message: format!("serialize safe descriptor: {error}"),
        }]
    })?;
    Ok(GeneratedBinding {
        file_stem: metadata.library.clone(),
        raw_header: generate_raw_header(metadata),
        safe_header: generate_safe_header(metadata),
        trampoline_source: generate_trampolines(metadata),
        conformance_source: generate_conformance(metadata),
        descriptor_json,
    })
}

fn find_prototype<'a>(header: &'a str, symbol: &str) -> Option<&'a str> {
    header.split(';').find_map(|segment| {
        let declaration = segment.trim();
        let index = declaration.find(symbol)?;
        let before = declaration[..index].chars().last();
        let after = declaration[index + symbol.len()..].chars().next();
        (before.is_none_or(|character| !is_identifier(character))
            && after.is_some_and(|character| character == '(' || character.is_whitespace())
            && declaration.contains('(')
            && declaration.contains(')'))
        .then_some(declaration)
    })
}

fn is_identifier(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
}

fn guard(metadata: &BindingMetadata, suffix: &str) -> String {
    format!(
        "RINZU_GENERATED_{}_{}",
        metadata.library.to_ascii_uppercase(),
        suffix
    )
}

fn generate_raw_header(metadata: &BindingMetadata) -> String {
    let guard = guard(metadata, "RAW_H");
    let mut output = format!(
        "/* generated; unsafe raw C surface */\n#ifndef {guard}\n#define {guard}\n#include \"{}\"\n",
        metadata.header
    );
    for function in &metadata.functions {
        output.push_str(&format!(
            "#define rinzu_raw_{}_{} {}\n",
            metadata.library, function.name, function.symbol
        ));
    }
    output.push_str(&format!("#endif /* {guard} */\n"));
    output
}

fn generate_safe_header(metadata: &BindingMetadata) -> String {
    let guard = guard(metadata, "SAFE_H");
    let mut output = format!(
        "/* generated; validated safe wrapper surface */\n#ifndef {guard}\n#define {guard}\n#include <stddef.h>\n#include <stdint.h>\n#include \"{}_raw.h\"\n\ntypedef struct rinzu_{}_registration {{\n  void *managed_context;\n  uint64_t generation;\n}} rinzu_{}_registration_t;\n\n",
        metadata.library, metadata.library, metadata.library
    );
    for (function_index, function) in metadata.functions.iter().enumerate() {
        let Some(callback) = function.callbacks.first() else {
            continue;
        };
        output.push_str(&format!(
            "{} rinzu_{}_{}_trampoline(void *context",
            callback.result_c_type, metadata.library, function.name
        ));
        for argument in &callback.arguments {
            output.push_str(&format!(", {} {}", argument.c_type, argument.name));
        }
        output.push_str(");\n");
        output.push_str(&format!(
            "static inline {} rinzu_safe_{}_{}(rinzu_{}_registration_t *registration",
            function.result_c_type, metadata.library, function.name, metadata.library
        ));
        for parameter in &function.parameters {
            if parameter.name != callback.context_parameter && parameter.name != callback.parameter
            {
                output.push_str(&format!(", {} {}", parameter.c_type, parameter.name));
            }
        }
        output.push_str(") {\n  ");
        if function.result_c_type.trim() != "void" {
            output.push_str("return ");
        }
        output.push_str(&format!("{}(", function.symbol));
        for (index, parameter) in function.parameters.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            if parameter.name == callback.context_parameter {
                output.push_str("registration");
            } else if parameter.name == callback.parameter {
                output.push_str(&format!(
                    "rinzu_{}_{}_trampoline",
                    metadata.library, function.name
                ));
            } else {
                output.push_str(&parameter.name);
            }
        }
        output.push_str(");\n");
        if function.result_c_type.trim() == "void" {
            output.push_str("  return;\n");
        }
        output.push_str("}\n");
        output.push_str(&format!(
            "#define RINZU_{}_CALLBACK_{}_CLASS {}\n\n",
            metadata.library.to_ascii_uppercase(),
            function_index,
            callback_class_name(callback.class)
        ));
    }
    output.push_str(&format!("#endif /* {guard} */\n"));
    output
}

fn callback_class_name(class: CallbackClass) -> &'static str {
    match class {
        CallbackClass::OwnerSync => "OWNER_SYNC",
        CallbackClass::OwnerAsync => "OWNER_ASYNC",
        CallbackClass::ForeignOneWay => "FOREIGN_ONE_WAY",
        CallbackClass::ForeignSync => unreachable!("ForeignSync is rejected by validation"),
    }
}

fn generate_trampolines(metadata: &BindingMetadata) -> String {
    let mut output = format!(
        "/* generated process-lifetime callback trampolines */\n#include \"{}_safe.h\"\n\nextern int64_t rinzu_generated_callback_dispatch_v1(void *registration, uint32_t callback_index, const void *const *arguments, size_t argument_count);\n\n",
        metadata.library
    );
    for (function_index, function) in metadata.functions.iter().enumerate() {
        let Some(callback) = function.callbacks.first() else {
            continue;
        };
        output.push_str(&format!(
            "{} rinzu_{}_{}_trampoline(void *context",
            callback.result_c_type, metadata.library, function.name
        ));
        for argument in &callback.arguments {
            output.push_str(&format!(", {} {}", argument.c_type, argument.name));
        }
        output.push_str(") {\n");
        if callback.arguments.is_empty() {
            output.push_str("  const void *const *arguments = NULL;\n");
        } else {
            output.push_str("  const void *arguments[] = {");
            for (index, argument) in callback.arguments.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                output.push_str(&format!("&{}", argument.name));
            }
            output.push_str("};\n");
        }
        let call = format!(
            "rinzu_generated_callback_dispatch_v1(context, {function_index}U, arguments, {}U)",
            callback.arguments.len()
        );
        if callback.result_c_type.trim() == "void" {
            output.push_str(&format!("  (void){call};\n"));
        } else {
            output.push_str(&format!("  return ({}){call};\n", callback.result_c_type));
        }
        output.push_str("}\n\n");
    }
    output
}

fn generate_conformance(metadata: &BindingMetadata) -> String {
    format!(
        "/* generated syntax/link conformance entry */\n#include \"{}_safe.h\"\n\nint64_t rinzu_generated_callback_dispatch_v1(void *registration, uint32_t callback_index, const void *const *arguments, size_t argument_count) {{\n  (void)registration; (void)callback_index; (void)arguments; (void)argument_count; return 0;\n}}\n\nint rinzu_{}_binding_conformance(void) {{\n  rinzu_{}_registration_t registration = {{0}};\n  (void)registration;\n  return 0;\n}}\n",
        metadata.library, metadata.library, metadata.library
    )
}

//! Nominal method identity and syntax normalization. Execution remains an ordinary call.
use crate::{Diagnostic, DiagnosticCode, SourceRange, ast::*};
use std::collections::{HashMap, HashSet};
type OwnerMembers<'a> = Vec<(&'a str, SourceRange)>;

/// Ignore dots in a package namespace; only the local declaration can be a method.
pub(crate) fn parts(name: &str) -> Option<(&str, &str)> {
    let local = name.rsplit_once("::").map_or(name, |(_, local)| local);
    (local.split('.').count() == 2)
        .then(|| name.rsplit_once('.'))
        .flatten()
}
pub(crate) fn module_name(name: &str) -> &str {
    name.rsplit_once("::").map_or("", |(module, _)| module)
}
pub(crate) fn export_name(path: &[String], target: &str) -> String {
    if parts(target).is_some() && path.len() >= 2 {
        path[path.len() - 2..].join(".")
    } else {
        path.last().expect("nonempty export path").clone()
    }
}

pub(crate) fn method_export_error(name: &str, range: SourceRange) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::InvalidModulePath,
        range,
        format!("method `{name}` cannot be exported individually"),
        "export the owner type; its non-private methods are exported automatically",
    )
}

/// Qualified identities of the non-private methods belonging to each nominal type.
pub(crate) fn public_methods(module: &Module) -> HashMap<String, Vec<String>> {
    let owners: HashSet<_> = module
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Record(record) => Some(record.name.clone()),
            Declaration::Enum(enumeration) => Some(enumeration.name.clone()),
            _ => None,
        })
        .collect();
    let mut methods: HashMap<String, Vec<String>> = HashMap::new();
    for declaration in &module.declarations {
        if let Declaration::Function(function) = declaration
            && !function.is_private
            && let Some((owner, _)) = parts(&function.name)
            && owners.contains(owner)
        {
            methods
                .entry(owner.to_owned())
                .or_default()
                .push(function.name.clone());
        }
    }
    for members in methods.values_mut() {
        members.sort();
    }
    methods
}

/// The standalone frontend and package linker share the same effective export surface.
pub(crate) fn expand_exports(module: &Module) -> Result<Module, Vec<Diagnostic>> {
    let functions: HashSet<_> = module
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Function(function) if parts(&function.name).is_some() => {
                Some(function.name.as_str())
            }
            _ => None,
        })
        .collect();
    for export in &module.exports {
        let name = export.path.join(".");
        if !export.implicit && functions.contains(name.as_str()) {
            return Err(vec![method_export_error(&name, export.range)]);
        }
    }
    let methods = public_methods(module);
    let mut output = module.clone();
    let mut seen: HashSet<_> = output
        .exports
        .iter()
        .map(|export| (export.path.join("."), export.range.file_id))
        .collect();
    for export in &module.exports {
        if let Some(members) = methods.get(&export.path.join(".")) {
            for name in members {
                if seen.insert((name.clone(), export.range.file_id)) {
                    output.exports.push(Export {
                        path: vec![name.clone()],
                        implicit: true,
                        range: export.range,
                    });
                }
            }
        }
    }
    Ok(output)
}

/// Validate before generic expansion can erase a generic owner/template declaration.
pub(crate) fn validate(module: &Module) -> Result<(), Vec<Diagnostic>> {
    let mut owners: HashMap<&str, (bool, OwnerMembers<'_>, Option<SourceRange>)> = HashMap::new();
    for declaration in &module.declarations {
        match declaration {
            Declaration::Record(r) => {
                owners.insert(
                    &r.name,
                    (
                        !r.type_parameters.is_empty(),
                        r.fields
                            .iter()
                            .map(|f| (f.name.as_str(), f.range))
                            .collect(),
                        Some(r.range),
                    ),
                );
            }
            Declaration::Enum(e) => {
                owners.insert(
                    &e.name,
                    (
                        !e.type_parameters.is_empty(),
                        e.variants
                            .iter()
                            .map(|v| (v.name.as_str(), v.range))
                            .collect(),
                        None,
                    ),
                );
            }
            _ => {}
        }
    }
    let mut native_owners: HashSet<String> = module
        .native_blocks
        .iter()
        .flat_map(|b| &b.members)
        .filter_map(|member| match member {
            NativeMember::Opaque { name, .. } => Some(name.clone()),
            NativeMember::Record(record) => Some(record.name.clone()),
            NativeMember::Function(_) => None,
        })
        .collect();
    // Alias expansion has resolved every non-generic alias to its final target.
    // A transparent name cannot acquire members that its native target forbids.
    let aliases = module
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::TypeAlias(alias) = declaration else {
                return None;
            };
            let TypeRefKind::Named(path) = &alias.ty.kind else {
                return None;
            };
            native_owners
                .contains(&path.join("."))
                .then_some(alias.name.clone())
        })
        .collect::<Vec<_>>();
    native_owners.extend(aliases);
    let mut errors = Vec::new();
    for declaration in &module.declarations {
        let Declaration::Function(f) = declaration else {
            continue;
        };
        let Some((owner, member)) = parts(&f.name) else {
            if f.is_private {
                errors.push(Diagnostic::error(
                    DiagnosticCode::UnexpectedToken,
                    f.range,
                    "private can only modify a nominal type's method",
                    "declare the private function as a method of its owner type",
                ));
            }
            continue;
        };
        if native_owners.contains(owner) {
            errors.push(Diagnostic::error(
                DiagnosticCode::UnsupportedSafeFfi,
                f.range,
                format!("native type `{owner}` cannot own methods"),
                "hold the native reference in a class and declare methods there, or use a free function",
            ));
            continue;
        }
        let Some((generic, fields, class_range)) = owners.get(owner) else {
            errors.push(Diagnostic::error(DiagnosticCode::UnknownType,f.range,
                format!("method owner `{owner}` must be a nominal type defined in the same module"),
                "define the method in its owner module; use a free function for foreign or builtin types"));
            continue;
        };
        if let Some(range) = class_range
            && (range.file_id != f.range.file_id
                || f.range.start_byte < range.start_byte
                || f.range.end_byte > range.end_byte)
        {
            errors.push(Diagnostic::error(
                DiagnosticCode::UnexpectedToken,
                f.range,
                format!(
                    "class method `{}` must be declared inside class `{owner}`",
                    f.name
                ),
                "move the method into the class and use an unqualified method name",
            ));
        }
        if *generic || !f.type_parameters.is_empty() {
            errors.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                f.range,
                "generic owners and generic methods are not supported by this method surface",
                "use a generic free function with an explicit receiver argument",
            ));
        }
        if let Some((_, range)) = fields.iter().find(|(name, _)| *name == member) {
            errors.push(
                Diagnostic::error(
                    DiagnosticCode::DuplicateName,
                    f.range,
                    format!("method `{}` conflicts with a field or enum variant", f.name),
                    "give the operation and stored field/variant different names",
                )
                .with_secondary(*range, "the field or variant is declared here"),
            );
        }
        for (index, parameter) in f
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, p)| p.name == "self")
        {
            if index != 0
                || !matches!(&parameter.ty.kind,TypeRefKind::Named(path) if path.join(".")==owner)
            {
                errors.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    parameter.range,
                    format!(
                        "instance method `{}` requires self:{owner} as its first parameter",
                        f.name
                    ),
                    "put self with the exact owner type first, or omit self for a static method",
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub(crate) fn normalize(module: &mut Module, replacements: &HashMap<SourceRange, ExpressionKind>) {
    struct Normalizer<'a>(&'a HashMap<SourceRange, ExpressionKind>);
    impl crate::ast_walk::Visitor for Normalizer<'_> {
        fn expression(&mut self, expression: &mut Expression) {
            if let Some(kind) = self.0.get(&expression.range) {
                // Keep already-normalized nested arguments from the post-order walk.
                match (&mut expression.kind, kind) {
                    (
                        ExpressionKind::Call {
                            target,
                            arguments: Arguments::Positional(values),
                        },
                        ExpressionKind::Call {
                            target: new_target,
                            arguments: Arguments::Positional(new_values),
                        },
                    ) => {
                        *target = new_target.clone();
                        if new_values.len() == values.len() + 1 {
                            values.insert(0, new_values[0].clone());
                        }
                    }
                    (
                        ExpressionKind::Spawn { target, .. },
                        ExpressionKind::Spawn {
                            target: new_target, ..
                        },
                    ) => {
                        *target = new_target.clone();
                    }
                    _ => unreachable!("checked method normalization shape"),
                }
            }
        }
    }
    crate::ast_walk::module(module, &mut Normalizer(replacements));
}

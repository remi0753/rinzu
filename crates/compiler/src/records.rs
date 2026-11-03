//! Transparent aliases and canonical structural aggregate identities.
use crate::{Diagnostic, DiagnosticCode, SourceRange, Type, ast::*};
use std::collections::{HashMap, HashSet};

pub(crate) fn identity(fields: &[(String, Type)]) -> String {
    let mut fields = fields
        .iter()
        .map(|(name, ty)| (name, ty.to_string()))
        .collect::<Vec<_>>();
    fields.sort_by(|a, b| a.0.cmp(b.0));
    // Length-delimited JSON avoids ambiguities between field names/types. The
    // interner additionally checks the complete shape before reusing an id.
    let bytes = serde_json::to_vec(&fields).expect("structural field spelling");
    let mut a = 0xcbf29ce484222325_u64;
    let mut b = 0x84222325cbf29ce4_u64;
    for byte in bytes {
        a = (a ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        b = (b ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("$rinzu_record_{a:016x}{b:016x}")
}

pub(crate) fn expand_aliases(module: &Module) -> Result<Module, Vec<Diagnostic>> {
    let aliases = module
        .declarations
        .iter()
        .filter_map(|d| {
            if let Declaration::TypeAlias(a) = d {
                Some((a.name.clone(), a.clone()))
            } else {
                None
            }
        })
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    for d in &module.declarations {
        if !seen.insert(d.name()) {
            return Err(vec![Diagnostic::error(
                DiagnosticCode::DuplicateName,
                d.range(),
                format!("duplicate declaration `{}`", d.name()),
                "give declarations distinct names",
            )]);
        }
    }
    for alias in aliases.values() {
        let mut names = HashSet::new();
        for p in &alias.type_parameters {
            if !names.insert(&p.name) {
                return Err(vec![error(p.range, "duplicate alias type parameter")]);
            }
            if !p.constraints.is_empty() {
                return Err(vec![error(
                    p.range,
                    "transparent aliases do not declare marker constraints; constrain the consuming function or class",
                )]);
            }
        }
        let mut ty = alias.ty.clone();
        expand_type(
            &mut ty,
            &aliases,
            &alias
                .type_parameters
                .iter()
                .map(|p| {
                    (
                        p.name.clone(),
                        TypeRef {
                            range: p.range,
                            kind: TypeRefKind::Named(vec![format!("$alias_parameter_{}", p.name)]),
                        },
                    )
                })
                .collect(),
            &mut vec![alias.name.clone()],
        )
        .map_err(|e| vec![e])?;
    }
    struct Expander<'a> {
        aliases: &'a HashMap<String, TypeAliasDecl>,
        errors: Vec<Diagnostic>,
    }
    impl crate::ast_walk::Visitor for Expander<'_> {
        fn ty(&mut self, ty: &mut TypeRef) {
            if let Err(e) = expand_type(ty, self.aliases, &HashMap::new(), &mut Vec::new()) {
                self.errors.push(e);
            }
        }
    }
    let mut output = module.clone();
    output
        .declarations
        .retain(|d| !matches!(d, Declaration::TypeAlias(a) if !a.type_parameters.is_empty()));
    let mut expander = Expander {
        aliases: &aliases,
        errors: Vec::new(),
    };
    for declaration in &mut output.declarations {
        let parameters = match declaration {
            Declaration::Function(f) => &f.type_parameters,
            Declaration::Record(r) => &r.type_parameters,
            Declaration::Enum(e) => &e.type_parameters,
            Declaration::Error(error) => &error.type_parameters,
            _ => &Vec::new(),
        };
        let mut visible = aliases.clone();
        for p in parameters {
            visible.remove(&p.name);
        }
        let mut scoped = Expander {
            aliases: &visible,
            errors: Vec::new(),
        };
        crate::ast_walk::declaration(declaration, &mut scoped);
        expander.errors.extend(scoped.errors);
    }
    for member in output.native_blocks.iter_mut().flat_map(|b| &mut b.members) {
        match member {
            NativeMember::Record(r) => {
                for field in &mut r.fields {
                    crate::ast_walk::ty(&mut field.ty, &mut expander);
                }
            }
            NativeMember::Function(f) => {
                for p in &mut f.parameters {
                    crate::ast_walk::ty(&mut p.ty, &mut expander);
                }
                crate::ast_walk::ty(&mut f.result, &mut expander);
            }
            _ => {}
        }
    }
    if expander.errors.is_empty() {
        Ok(output)
    } else {
        Err(expander.errors)
    }
}

fn expand_type(
    ty: &mut TypeRef,
    aliases: &HashMap<String, TypeAliasDecl>,
    substitution: &HashMap<String, TypeRef>,
    stack: &mut Vec<String>,
) -> Result<(), Diagnostic> {
    if let TypeRefKind::Named(path) = &ty.kind
        && path.len() == 1
        && let Some(value) = substitution.get(&path[0])
    {
        ty.kind = value.kind.clone();
        return Ok(());
    }
    let application = match &ty.kind {
        TypeRefKind::Named(path) => aliases
            .get(&path.join("."))
            .map(|a| (a.clone(), Vec::new())),
        TypeRefKind::Generic { path, arguments } => aliases
            .get(&path.join("."))
            .map(|a| (a.clone(), arguments.clone())),
        _ => None,
    };
    if let Some((alias, mut arguments)) = application {
        if arguments.len() != alias.type_parameters.len() {
            return Err(error(
                ty.range,
                format!(
                    "alias `{}` requires {} explicit type arguments",
                    alias.name,
                    alias.type_parameters.len()
                ),
            ));
        }
        for a in &mut arguments {
            expand_type(a, aliases, substitution, stack)?;
        }
        if stack.contains(&alias.name) || stack.len() >= 64 {
            return Err(error(
                ty.range,
                "recursive or excessively deep type alias expansion",
            ));
        }
        let parameters = alias
            .type_parameters
            .iter()
            .zip(arguments)
            .map(|(p, a)| (p.name.clone(), a))
            .collect();
        let mut result = alias.ty.clone();
        stack.push(alias.name);
        expand_type(&mut result, aliases, &parameters, stack)?;
        stack.pop();
        ty.kind = result.kind;
        return Ok(());
    }
    match &mut ty.kind {
        TypeRefKind::Record(fields) => {
            let mut names = HashSet::new();
            for f in fields {
                if !names.insert(&f.name) {
                    return Err(error(
                        f.range,
                        format!("duplicate field `{}` in record type", f.name),
                    ));
                }
                expand_type(&mut f.ty, aliases, substitution, stack)?;
            }
        }
        TypeRefKind::Generic { arguments, .. } => {
            for a in arguments {
                expand_type(a, aliases, substitution, stack)?;
            }
        }
        TypeRefKind::List(t) | TypeRefKind::Future(t) => {
            expand_type(t, aliases, substitution, stack)?
        }
        TypeRefKind::Map(k, v) => {
            expand_type(k, aliases, substitution, stack)?;
            expand_type(v, aliases, substitution, stack)?;
        }
        TypeRefKind::Function {
            parameters, result, ..
        } => {
            for p in parameters {
                expand_type(p, aliases, substitution, stack)?;
            }
            expand_type(result, aliases, substitution, stack)?;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn normalize(
    module: &mut Module,
    literals: &HashMap<SourceRange, String>,
    resolve: impl Fn(&TypeRef) -> Option<Type>,
) {
    struct Normalizer<'a, F> {
        literals: &'a HashMap<SourceRange, String>,
        resolve: F,
        native: bool,
    }
    impl<F: Fn(&TypeRef) -> Option<Type>> crate::ast_walk::Visitor for Normalizer<'_, F> {
        fn ty(&mut self, t: &mut TypeRef) {
            if !self.native
                && matches!(t.kind, TypeRefKind::Bytes(_))
                && let Some(Type::FixedBytes(n)) = (self.resolve)(t)
            {
                t.kind = TypeRefKind::ConcreteBytes(n);
            }
            if matches!(t.kind, TypeRefKind::Record(_))
                && let Some(Type::Named(name)) = (self.resolve)(t)
            {
                t.kind = TypeRefKind::Named(vec![name]);
            }
        }
        fn expression(&mut self, e: &mut Expression) {
            if let ExpressionKind::Record(fields) = &e.kind
                && let Some(name) = self.literals.get(&e.range)
            {
                e.kind = ExpressionKind::Call {
                    target: vec![name.clone()],
                    arguments: Arguments::Named(
                        fields
                            .iter()
                            .map(|f| (f.name.clone(), f.value.clone(), f.range))
                            .collect(),
                    ),
                };
            }
        }
    }
    let mut normalizer = Normalizer {
        literals,
        resolve,
        native: false,
    };
    crate::ast_walk::module(module, &mut normalizer);
    // A context's structural identity belongs to its source fields, before
    // closure conversion changes their physical representation. Native callback
    // function types and dependent byte lengths retain their original ABI form.
    normalizer.native = true;
    for member in module
        .native_blocks
        .iter_mut()
        .flat_map(|block| &mut block.members)
    {
        if let NativeMember::Function(function) = member {
            for parameter in &mut function.parameters {
                crate::ast_walk::ty(&mut parameter.ty, &mut normalizer);
            }
            crate::ast_walk::ty(&mut function.result, &mut normalizer);
        }
    }
}

fn error(range: SourceRange, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::TypeMismatch,
        range,
        message,
        "use a finite transparent alias with explicit type arguments",
    )
}

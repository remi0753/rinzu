//! Nominal, declaration-local interface conformance and static dispatch.
//!
//! A constrained call selects a private specialization of the ordinary method
//! with the interface's public Error bound. It executes the same body (including
//! the same async scheduling), without boxing, vtables, or an additional task.
use crate::{Diagnostic, DiagnosticCode, SourceRange, ast::*};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(crate) fn spelling(ty: &TypeRef) -> String {
    crate::generics::spelling(ty)
}

pub(crate) fn substitute(ty: &mut TypeRef, values: &HashMap<String, TypeRef>) {
    struct Replace<'a>(&'a HashMap<String, TypeRef>);
    impl crate::ast_walk::Visitor for Replace<'_> {
        fn ty(&mut self, ty: &mut TypeRef) {
            if let TypeRefKind::Named(path) = &ty.kind
                && path.len() == 1
                && let Some(value) = self.0.get(&path[0])
            {
                ty.kind = value.kind.clone();
            }
        }
    }
    crate::ast_walk::ty(ty, &mut Replace(values));
}

fn error(range: SourceRange, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::InvalidInterface,
        range,
        message,
        "declare conformance on the class or enum and provide matching public instance methods",
    )
}

fn application(ty: &TypeRef) -> Option<(String, Vec<TypeRef>)> {
    match &ty.kind {
        TypeRefKind::Named(path) => Some((path.join("."), Vec::new())),
        TypeRefKind::Generic { path, arguments } => Some((path.join("."), arguments.clone())),
        _ => None,
    }
}

fn validate_declarations(
    module: &Module,
    known: &HashMap<String, Declaration>,
) -> Result<(), Diagnostic> {
    fn ty(
        source: &TypeRef,
        parameters: &BTreeSet<String>,
        known: &HashMap<String, Declaration>,
        native: &BTreeSet<String>,
    ) -> Result<(), Diagnostic> {
        match &source.kind {
            TypeRefKind::Named(path) | TypeRefKind::Generic { path, .. } => {
                let name = path.join(".");
                let args = match &source.kind {
                    TypeRefKind::Generic { arguments, .. } => arguments.as_slice(),
                    _ => &[],
                };
                if matches!(known.get(&name), Some(Declaration::Interface(_))) {
                    return Err(error(
                        source.range,
                        "an interface is a constraint, not a value type",
                    ));
                }
                let count = match known.get(&name) {
                    Some(Declaration::Record(r)) => r.type_parameters.len(),
                    Some(Declaration::Enum(e)) => e.type_parameters.len(),
                    Some(Declaration::Error(e)) => e.type_parameters.len(),
                    _ if name == "Option" => 1,
                    _ if parameters.contains(&name)
                        || native.contains(&name)
                        || crate::AbiScalar::from_name(&name).is_some()
                        || matches!(
                            name.as_str(),
                            "String"
                                | "Unit"
                                | "Never"
                                | "Error"
                                | "ByteArray"
                                | "SourceLocation"
                                | "$rinzu_SourceLocation"
                        ) =>
                    {
                        0
                    }
                    _ => {
                        return Err(error(
                            source.range,
                            format!("unknown interface signature type `{name}`"),
                        ));
                    }
                };
                if args.len() != count {
                    return Err(error(
                        source.range,
                        "interface signature type argument count mismatch",
                    ));
                }
                for a in args {
                    ty(a, parameters, known, native)?;
                }
            }
            TypeRefKind::List(t) | TypeRefKind::Future(t) => ty(t, parameters, known, native)?,
            TypeRefKind::Map(k, v) => {
                ty(k, parameters, known, native)?;
                ty(v, parameters, known, native)?;
            }
            TypeRefKind::Record(fields) => {
                for f in fields {
                    ty(&f.ty, parameters, known, native)?;
                }
            }
            TypeRefKind::Function {
                parameters: args,
                result,
                ..
            } => {
                for a in args {
                    ty(a, parameters, known, native)?;
                }
                ty(result, parameters, known, native)?;
            }
            TypeRefKind::Bytes(_) | TypeRefKind::ConcreteBytes(_) => {}
        }
        Ok(())
    }
    let native: BTreeSet<_> = module
        .native_blocks
        .iter()
        .flat_map(|b| &b.members)
        .filter_map(|m| match m {
            NativeMember::Opaque { name, .. } => Some(name.clone()),
            NativeMember::Record(r) => Some(r.name.clone()),
            _ => None,
        })
        .collect();
    for d in &module.declarations {
        let Declaration::Interface(i) = d else {
            continue;
        };
        let mut parameters = BTreeSet::from(["Self".into()]);
        for p in &i.type_parameters {
            if !parameters.insert(p.name.clone()) {
                return Err(error(
                    p.range,
                    "duplicate or reserved interface type parameter",
                ));
            }
            if p.constraints
                .iter()
                .any(|m| !matches!(m.as_str(), "Transferable" | "Equality" | "Key"))
            {
                return Err(error(p.range, "unknown sealed marker constraint"));
            }
        }
        for m in &i.methods {
            let mut names = BTreeSet::new();
            for p in &m.parameters {
                if !names.insert(&p.name) {
                    return Err(error(p.range, "duplicate interface method parameter"));
                }
                ty(&p.ty, &parameters, known, &native)?;
            }
            ty(&m.result, &parameters, known, &native)?;
            if m.is_async && matches!(m.result.kind, TypeRefKind::Future(_)) {
                return Err(error(
                    m.result.range,
                    "an async interface method declares its result, not Future<Result>",
                ));
            }
            if let Some(c) = &m.error_contract {
                let mut seen = BTreeSet::new();
                for t in &c.types {
                    ty(t, &parameters, known, &native)?;
                    let Some((name, _)) = application(t) else {
                        return Err(error(t.range, "throws requires an Error type"));
                    };
                    if name != "Error" && !matches!(known.get(&name), Some(Declaration::Error(_))) {
                        return Err(error(t.range, "throws requires a nominal Error type"));
                    }
                    if !seen.insert(spelling(t)) {
                        return Err(error(t.range, "duplicate interface Error bound"));
                    }
                }
                if seen.contains("Error") && seen.len() > 1 {
                    return Err(error(
                        c.range,
                        "Error already includes every ordinary Error",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn method(
    interface: &InterfaceDecl,
    args: &[TypeRef],
    owner: &TypeRef,
    member: &str,
) -> Option<FunctionDecl> {
    let mut method = interface.methods.iter().find(|m| m.name == member)?.clone();
    let mut values: HashMap<_, _> = interface
        .type_parameters
        .iter()
        .zip(args)
        .map(|(p, t)| (p.name.clone(), t.clone()))
        .collect();
    values.insert("Self".into(), owner.clone());
    for p in &mut method.parameters {
        substitute(&mut p.ty, &values);
    }
    substitute(&mut method.result, &values);
    if let Some(c) = &mut method.error_contract {
        for t in &mut c.types {
            substitute(t, &values);
        }
    }
    Some(method)
}

fn same_signature(a: &FunctionDecl, b: &FunctionDecl) -> bool {
    a.is_async == b.is_async
        && a.parameters.len() == b.parameters.len()
        && a.parameters
            .iter()
            .zip(&b.parameters)
            .all(|(a, b)| spelling(&a.ty) == spelling(&b.ty))
        && spelling(&a.result) == spelling(&b.result)
}

fn bound(contract: &ErrorContract) -> BTreeSet<String> {
    contract.types.iter().map(spelling).collect()
}
fn within(actual: &ErrorContract, expected: &ErrorContract) -> bool {
    let expected = bound(expected);
    expected.contains("Error") || bound(actual).is_subset(&expected)
}

fn intersect(left: &mut ErrorContract, right: &ErrorContract) {
    let other = bound(right);
    if other.contains("Error") {
        return;
    }
    if bound(left).contains("Error") {
        left.types = right.types.clone();
    } else {
        left.types.retain(|ty| other.contains(&spelling(ty)));
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Dispatch {
    interfaces: Vec<String>,
    member: String,
}
impl Dispatch {
    fn member(&self) -> String {
        let identity = format!("{}::{}", self.interfaces.join("+"), self.member);
        format!(
            "$interface_{:016x}_{}",
            crate::fnv1a64(identity.as_bytes()),
            self.member
        )
    }
}

pub(crate) fn prepare(module: &Module) -> Result<Module, Vec<Diagnostic>> {
    prepare_inner(module).map_err(|e| vec![e])
}

fn prepare_inner(module: &Module) -> Result<Module, Diagnostic> {
    let interfaces: BTreeMap<_, _> = module
        .declarations
        .iter()
        .filter_map(|d| match d {
            Declaration::Interface(i) => Some((i.name.clone(), i.clone())),
            _ => None,
        })
        .collect();
    let known: HashMap<_, _> = module
        .declarations
        .iter()
        .map(|d| (d.name().to_owned(), d.clone()))
        .collect();
    validate_declarations(module, &known)?;
    for i in interfaces.values() {
        let mut names = BTreeSet::new();
        for m in &i.methods {
            if !names.insert(&m.name) {
                return Err(error(m.range, "duplicate interface method"));
            }
            if m.name.contains('.')
                || !m
                    .parameters
                    .first()
                    .is_some_and(|p| p.name == "self" && spelling(&p.ty) == "Self")
            {
                return Err(error(
                    m.range,
                    "interface requirements must be instance methods with self: Self",
                ));
            }
            if m.error_contract.is_none() || m.is_private || !m.type_parameters.is_empty() {
                return Err(error(
                    m.range,
                    "interface methods require an explicit Error contract and cannot be private or generic",
                ));
            }
            for ty in &m.error_contract.as_ref().unwrap().types {
                if i.type_parameters.iter().any(|p| spelling(ty) == p.name) {
                    return Err(error(
                        ty.range,
                        "a bare type parameter is not an Error-effect variable",
                    ));
                }
            }
        }
    }
    let mut dispatches = BTreeSet::new();
    for i in interfaces.values() {
        for m in &i.methods {
            dispatches.insert(Dispatch {
                interfaces: vec![i.name.clone()],
                member: m.name.clone(),
            });
        }
    }
    let mut output = module.clone();
    for d in &mut output.declarations {
        if let Declaration::Function(f) = d {
            if f.type_parameters.iter().all(|p| p.interfaces.is_empty()) {
                continue;
            }
            let mut resolver = Resolver {
                interfaces: &interfaces,
                known: &known,
                dispatches: &mut dispatches,
                constraints: f
                    .type_parameters
                    .iter()
                    .map(|p| (p.name.clone(), p.interfaces.clone()))
                    .collect(),
                locals: f
                    .parameters
                    .iter()
                    .map(|p| (p.name.clone(), p.ty.clone()))
                    .collect(),
            };
            resolver.block(&mut f.body)?;
        }
    }
    let mut generated = Vec::new();
    let implementations: HashMap<_, _> = output
        .declarations
        .iter()
        .map(|d| (d.name().to_owned(), d))
        .collect();
    for d in &module.declarations {
        let (parameters, declared) = match d {
            Declaration::Record(r) => (&r.type_parameters, &r.interfaces),
            Declaration::Enum(e) => (&e.type_parameters, &e.interfaces),
            _ => continue,
        };
        let owner = TypeRef {
            range: d.range(),
            kind: if parameters.is_empty() {
                TypeRefKind::Named(vec![d.name().into()])
            } else {
                TypeRefKind::Generic {
                    path: vec![d.name().into()],
                    arguments: parameters
                        .iter()
                        .map(|p| TypeRef {
                            range: p.range,
                            kind: TypeRefKind::Named(vec![p.name.clone()]),
                        })
                        .collect(),
                }
            },
        };
        let mut applications = BTreeMap::new();
        for source in declared {
            let (name, args) = application(source)
                .ok_or_else(|| error(source.range, "an interface name is required"))?;
            let interface = interfaces
                .get(&name)
                .ok_or_else(|| error(source.range, format!("`{name}` is not an interface")))?;
            if args.len() != interface.type_parameters.len() {
                return Err(error(
                    source.range,
                    "interface type argument count does not match its declaration",
                ));
            }
            if applications.insert(name, args).is_some() {
                return Err(error(
                    source.range,
                    "an interface can be declared only once on an owner",
                ));
            }
        }
        for dispatch in &dispatches {
            if !dispatch
                .interfaces
                .iter()
                .all(|i| applications.contains_key(i))
            {
                continue;
            }
            let Some(Declaration::Function(actual)) = implementations
                .get(&format!("{}.{}", d.name(), dispatch.member))
                .copied()
            else {
                return Err(error(
                    d.range(),
                    format!(
                        "`{}` is missing required method `{}`",
                        d.name(),
                        dispatch.member
                    ),
                ));
            };
            if actual.is_private
                || !actual.parameters.first().is_some_and(|p| p.name == "self")
                || actual.type_parameters.len() != parameters.len()
            {
                return Err(error(
                    actual.range,
                    "an interface implementation must be a public, non-generic instance method",
                ));
            }
            let mut expected: Option<FunctionDecl> = None;
            for name in &dispatch.interfaces {
                let request = method(
                    &interfaces[name],
                    &applications[name],
                    &owner,
                    &dispatch.member,
                )
                .unwrap();
                if !same_signature(actual, &request) {
                    return Err(error(
                        actual.range,
                        format!("method `{}` does not match interface `{name}`", actual.name),
                    )
                    .with_secondary(request.range, "the required signature is declared here"));
                }
                if let Some(contract) = &actual.error_contract
                    && !within(contract, request.error_contract.as_ref().unwrap())
                {
                    return Err(error(
                        contract.range,
                        "implementation Error contract exceeds the interface's upper bound",
                    )
                    .with_secondary(
                        request.range,
                        "the interface's Error contract is declared here",
                    ));
                }
                if let Some(previous) = &mut expected {
                    if !same_signature(previous, &request) {
                        return Err(error(
                            d.range(),
                            "incompatible same-name interface requirements",
                        ));
                    }
                    intersect(
                        previous.error_contract.as_mut().unwrap(),
                        request.error_contract.as_ref().unwrap(),
                    );
                } else {
                    expected = Some(request);
                }
            }
            let mut specialized = actual.clone();
            specialized.name = format!("{}.{}", d.name(), dispatch.member());
            specialized.is_private = true;
            specialized.error_contract = expected.unwrap().error_contract;
            generated.push(Declaration::Function(specialized));
        }
    }
    output.declarations.extend(generated);
    // Validate bound names even on templates which have no concrete uses yet.
    for d in &output.declarations {
        let parameters = match d {
            Declaration::Function(f) => &f.type_parameters,
            Declaration::Record(r) => &r.type_parameters,
            Declaration::Enum(e) => &e.type_parameters,
            Declaration::Interface(i) => &i.type_parameters,
            Declaration::Error(e) => &e.type_parameters,
            Declaration::TypeAlias(a) => &a.type_parameters,
            Declaration::Constant(_) => continue,
        };
        for p in parameters {
            let mut seen = BTreeSet::new();
            let mut requirements = BTreeMap::<String, FunctionDecl>::new();
            for source in &p.interfaces {
                let Some((name, args)) = application(source) else {
                    return Err(error(
                        source.range,
                        "an interface constraint requires an interface name",
                    ));
                };
                let Some(i) = interfaces.get(&name) else {
                    return Err(error(source.range, format!("`{name}` is not an interface")));
                };
                if !seen.insert(name) {
                    return Err(error(source.range, "duplicate interface constraint"));
                }
                if args.len() != i.type_parameters.len() {
                    return Err(error(
                        source.range,
                        "interface type argument count mismatch",
                    ));
                }
                let owner = TypeRef {
                    range: p.range,
                    kind: TypeRefKind::Named(vec![p.name.clone()]),
                };
                for m in &i.methods {
                    let request = method(i, &args, &owner, &m.name).unwrap();
                    if let Some(previous) = requirements.insert(m.name.clone(), request.clone())
                        && !same_signature(&previous, &request)
                    {
                        return Err(error(
                            source.range,
                            "conflicting same-name methods in interface constraints",
                        )
                        .with_secondary(previous.range, "the other requirement is declared here"));
                    }
                }
            }
        }
    }
    Ok(output)
}

/// Verify nominal conformance after concrete type arguments have been selected.
pub(crate) fn satisfies(owner: &Declaration, args: &[TypeRef], expected: &TypeRef) -> bool {
    let (parameters, declared) = match owner {
        Declaration::Record(r) => (&r.type_parameters, &r.interfaces),
        Declaration::Enum(e) => (&e.type_parameters, &e.interfaces),
        _ => return false,
    };
    let values = parameters
        .iter()
        .zip(args)
        .map(|(p, t)| (p.name.clone(), t.clone()))
        .collect();
    declared.iter().any(|source| {
        let mut source = source.clone();
        substitute(&mut source, &values);
        spelling(&source) == spelling(expected)
    })
}

struct Resolver<'a> {
    interfaces: &'a BTreeMap<String, InterfaceDecl>,
    known: &'a HashMap<String, Declaration>,
    dispatches: &'a mut BTreeSet<Dispatch>,
    constraints: HashMap<String, Vec<TypeRef>>,
    locals: HashMap<String, TypeRef>,
}

impl Resolver<'_> {
    fn requirements(&self, ty: &TypeRef, member: &str) -> Vec<(String, FunctionDecl)> {
        let Some((parameter, args)) = application(ty) else {
            return Vec::new();
        };
        if !args.is_empty() {
            return Vec::new();
        }
        self.constraints
            .get(&parameter)
            .into_iter()
            .flatten()
            .filter_map(|source| {
                let (name, args) = application(source)?;
                let i = self.interfaces.get(&name)?;
                Some((name, method(i, &args, ty, member)?))
            })
            .collect()
    }
    fn field(&self, ty: TypeRef, name: &str) -> Option<TypeRef> {
        if let TypeRefKind::Record(fields) = &ty.kind {
            return fields.iter().find(|f| f.name == name).map(|f| f.ty.clone());
        }
        let (owner, args) = application(&ty)?;
        let Declaration::Record(r) = self.known.get(&owner)? else {
            return None;
        };
        let mut ty = r.fields.iter().find(|f| f.name == name)?.ty.clone();
        let values = r
            .type_parameters
            .iter()
            .zip(args)
            .map(|(p, t)| (p.name.clone(), t))
            .collect();
        substitute(&mut ty, &values);
        Some(ty)
    }
    fn path(&self, path: &[String]) -> Option<TypeRef> {
        let (first, fields) = path.split_first()?;
        let mut ty = self.locals.get(first)?.clone();
        for field in fields {
            ty = self.field(ty, field)?;
        }
        Some(ty)
    }
    fn infer(&self, e: &Expression) -> Option<TypeRef> {
        let named = |name: &str| TypeRef {
            range: e.range,
            kind: TypeRefKind::Named(vec![name.into()]),
        };
        match &e.kind {
            ExpressionKind::Name(path) => self.path(path).or_else(|| {
                if path.len() == 2 && self.constraints.contains_key(&path[0]) {
                    let owner = TypeRef {
                        range: e.range,
                        kind: TypeRefKind::Named(vec![path[0].clone()]),
                    };
                    if let Some((_, request)) = self.requirements(&owner, &path[1]).first() {
                        return Some(function_type(request, e.range));
                    }
                }
                let Declaration::Function(f) = self.known.get(&path.join("."))? else {
                    return None;
                };
                Some(function_type(f, e.range))
            }),
            ExpressionKind::Literal(Literal::String(_)) | ExpressionKind::FormatString(_) => {
                Some(named("String"))
            }
            ExpressionKind::Literal(Literal::Bool(_)) => Some(named("Bool")),
            ExpressionKind::Literal(
                Literal::Int(_) | Literal::Unsigned(_) | Literal::DecimalInteger(_),
            ) => Some(named("Int")),
            ExpressionKind::Unit => Some(named("Unit")),
            ExpressionKind::Await(inner) => match self.infer(inner)?.kind {
                TypeRefKind::Future(t) => Some(*t),
                _ => None,
            },
            ExpressionKind::List(values) => Some(TypeRef {
                range: e.range,
                kind: TypeRefKind::List(Box::new(self.infer(values.first()?)?)),
            }),
            ExpressionKind::Map(values) => {
                let (k, v) = values.first()?;
                Some(TypeRef {
                    range: e.range,
                    kind: TypeRefKind::Map(Box::new(self.infer(k)?), Box::new(self.infer(v)?)),
                })
            }
            ExpressionKind::Record(values) => Some(TypeRef {
                range: e.range,
                kind: TypeRefKind::Record(
                    values
                        .iter()
                        .map(|f| {
                            Some(FieldDecl {
                                name: f.name.clone(),
                                ty: f.annotation.clone().or_else(|| self.infer(&f.value))?,
                                is_const: false,
                                is_private: false,
                                range: f.range,
                            })
                        })
                        .collect::<Option<_>>()?,
                ),
            }),
            ExpressionKind::RecordUpdate { base, .. } => self.infer(base),
            ExpressionKind::Function(f) => Some(function_type(f, e.range)),
            ExpressionKind::GenericReference {
                target,
                type_arguments,
                member,
                member_type_arguments,
            } => {
                let name = member.as_ref().map_or_else(
                    || target.join("."),
                    |m| format!("{}.{}", target.join("."), m),
                );
                let Declaration::Function(f) = self.known.get(&name)? else {
                    return None;
                };
                let values = f
                    .type_parameters
                    .iter()
                    .zip(type_arguments.iter().chain(member_type_arguments))
                    .map(|(p, t)| (p.name.clone(), t.clone()))
                    .collect();
                let mut result = function_type(f, e.range);
                substitute(&mut result, &values);
                Some(result)
            }
            ExpressionKind::Spawn { target, arguments } => self.call_type(e, target, arguments),
            ExpressionKind::Call { target, arguments }
            | ExpressionKind::GenericCall {
                target, arguments, ..
            } => self.call_type(e, target, arguments),
            _ => None,
        }
    }
    fn call_type(
        &self,
        e: &Expression,
        target: &[String],
        arguments: &Arguments,
    ) -> Option<TypeRef> {
        let spawned = matches!(
            e.kind,
            ExpressionKind::Spawn { .. } | ExpressionKind::GenericCall { spawn: true, .. }
        );
        if let Some(TypeRef {
            kind: TypeRefKind::Function { result, .. },
            ..
        }) = self.path(target)
        {
            return Some(if spawned {
                TypeRef {
                    range: e.range,
                    kind: TypeRefKind::Future(result),
                }
            } else {
                *result
            });
        }
        // Only collection operations can return a constrained element without a
        // source function signature. Other intrinsics cannot manufacture R.
        if let Arguments::Positional(args) = arguments
            && let Some(operation) = crate::core::data_operation(target)
            && let Some(input) = args.first().and_then(|a| self.infer(a))
        {
            let option = |t| TypeRef {
                range: e.range,
                kind: TypeRefKind::Generic {
                    path: vec!["Option".into()],
                    arguments: vec![t],
                },
            };
            use crate::DataOperation::*;
            match (operation, &input.kind) {
                (ListGet, TypeRefKind::List(t)) | (MapGet, TypeRefKind::Map(_, t)) => {
                    return Some(option(*t.clone()));
                }
                (ListAppend | MapInsert | MapRemove, _) => return Some(input),
                (ListSet | ListRemove, _) => return Some(option(input)),
                (MapKeys, TypeRefKind::Map(k, _)) => {
                    return Some(TypeRef {
                        range: e.range,
                        kind: TypeRefKind::List(k.clone()),
                    });
                }
                _ => {}
            }
        }
        let (enum_name, variant, explicit) = match &e.kind {
            ExpressionKind::GenericCall {
                target,
                variant: Some(v),
                type_arguments,
                ..
            } => (target.join("."), v.as_str(), type_arguments.as_slice()),
            _ if target.len() > 1 && !self.locals.contains_key(&target[0]) => (
                target[..target.len() - 1].join("."),
                target.last()?.as_str(),
                &[][..],
            ),
            _ => (String::new(), "", &[][..]),
        };
        if enum_name == "Option" && !self.known.contains_key("Option") {
            let arg = explicit.first().cloned().or_else(|| match arguments {
                Arguments::Named(a) => self.infer(&a.first()?.1),
                _ => None,
            })?;
            return Some(TypeRef {
                range: e.range,
                kind: TypeRefKind::Generic {
                    path: vec![enum_name],
                    arguments: vec![arg],
                },
            });
        }
        if let Some(Declaration::Enum(enumeration)) = self.known.get(&enum_name) {
            let variant = enumeration.variants.iter().find(|v| v.name == variant)?;
            let mut values: HashMap<_, _> = enumeration
                .type_parameters
                .iter()
                .zip(explicit)
                .map(|(p, t)| (p.name.clone(), t.clone()))
                .collect();
            if let Arguments::Named(args) = arguments {
                for (name, a, _) in args {
                    if let (Some(f), Some(t)) = (
                        variant.fields.iter().find(|f| f.name == *name),
                        self.infer(a),
                    ) {
                        unify(&f.ty, &t, &mut values);
                    }
                }
            }
            let args = enumeration
                .type_parameters
                .iter()
                .map(|p| values.get(&p.name).cloned())
                .collect::<Option<Vec<_>>>()?;
            return Some(TypeRef {
                range: e.range,
                kind: if args.is_empty() {
                    TypeRefKind::Named(vec![enum_name])
                } else {
                    TypeRefKind::Generic {
                        path: vec![enum_name],
                        arguments: args,
                    }
                },
            });
        }
        let mut fixed = Vec::new();
        let name = if target.len() > 1 && self.locals.contains_key(&target[0]) {
            let receiver = self.path(&target[..target.len() - 1])?;
            if let Some((_, request)) = self.requirements(&receiver, target.last()?).first() {
                return Some(if request.is_async || spawned {
                    TypeRef {
                        range: e.range,
                        kind: TypeRefKind::Future(Box::new(request.result.clone())),
                    }
                } else {
                    request.result.clone()
                });
            }
            let (owner, args) = application(&receiver)?;
            fixed = args;
            format!("{owner}.{}", target.last()?)
        } else {
            target.join(".")
        };
        match self.known.get(&name)? {
            Declaration::Function(f) => {
                let mut values: HashMap<_, _> = f
                    .type_parameters
                    .iter()
                    .zip(fixed)
                    .map(|(p, t)| (p.name.clone(), t))
                    .collect();
                if let ExpressionKind::GenericCall { type_arguments, .. } = &e.kind {
                    for (p, t) in f
                        .type_parameters
                        .iter()
                        .skip(values.len())
                        .zip(type_arguments)
                    {
                        values.insert(p.name.clone(), t.clone());
                    }
                }
                if let Arguments::Positional(args) = arguments {
                    let offset =
                        usize::from(target.len() > 1 && self.locals.contains_key(&target[0]));
                    for (p, a) in f.parameters.iter().skip(offset).zip(args) {
                        if let Some(t) = self.infer(a) {
                            unify(&p.ty, &t, &mut values);
                        }
                    }
                }
                let mut result = f.result.clone();
                substitute(&mut result, &values);
                if f.is_async || spawned {
                    result = TypeRef {
                        range: e.range,
                        kind: TypeRefKind::Future(Box::new(result)),
                    };
                }
                Some(result)
            }
            Declaration::Record(r) => {
                let mut values = HashMap::new();
                if let ExpressionKind::GenericCall { type_arguments, .. } = &e.kind {
                    for (p, t) in r.type_parameters.iter().zip(type_arguments) {
                        values.insert(p.name.clone(), t.clone());
                    }
                }
                if let Arguments::Named(args) = arguments {
                    for (name, a, _) in args {
                        if let (Some(f), Some(t)) =
                            (r.fields.iter().find(|f| f.name == *name), self.infer(a))
                        {
                            unify(&f.ty, &t, &mut values);
                        }
                    }
                }
                let args = r
                    .type_parameters
                    .iter()
                    .map(|p| values.get(&p.name).cloned())
                    .collect::<Option<Vec<_>>>()?;
                Some(TypeRef {
                    range: e.range,
                    kind: if args.is_empty() {
                        TypeRefKind::Named(vec![r.name.clone()])
                    } else {
                        TypeRefKind::Generic {
                            path: vec![r.name.clone()],
                            arguments: args,
                        }
                    },
                })
            }
            _ => None,
        }
    }
    fn expression(&mut self, e: &mut Expression) -> Result<(), Diagnostic> {
        // Resolve from the symbolic receiver, before monomorphization erases R.
        if let ExpressionKind::Call { target, .. }
        | ExpressionKind::GenericCall { target, .. }
        | ExpressionKind::Spawn { target, .. }
        | ExpressionKind::Name(target) = &mut e.kind
            && target.len() > 1
            && let Some(ty) = self.path(&target[..target.len() - 1]).or_else(|| {
                (target.len() == 2 && self.constraints.contains_key(&target[0])).then(|| TypeRef {
                    range: e.range,
                    kind: TypeRefKind::Named(vec![target[0].clone()]),
                })
            })
        {
            let member = target.last().unwrap().clone();
            let requests = self.requirements(&ty, &member);
            if !requests.is_empty() {
                let first = &requests[0].1;
                if requests.iter().any(|(_, m)| !same_signature(first, m)) {
                    return Err(error(e.range, "conflicting interface method signatures"));
                }
                let mut names = requests.into_iter().map(|(n, _)| n).collect::<Vec<_>>();
                names.sort();
                names.dedup();
                let dispatch = Dispatch {
                    interfaces: names,
                    member,
                };
                *target.last_mut().unwrap() = dispatch.member();
                self.dispatches.insert(dispatch);
            }
        }
        // Ordinary AST recursion also handles captures and shadowed parameters.
        match &mut e.kind {
            ExpressionKind::Function(f) => {
                let saved = self.locals.clone();
                for p in &f.parameters {
                    self.locals.insert(p.name.clone(), p.ty.clone());
                }
                self.block(&mut f.body)?;
                self.locals = saved;
            }
            ExpressionKind::Call { arguments, .. }
            | ExpressionKind::GenericCall { arguments, .. }
            | ExpressionKind::Spawn { arguments, .. }
            | ExpressionKind::Bind { arguments, .. } => match arguments {
                Arguments::Positional(v) => {
                    for a in v {
                        self.expression(a)?;
                    }
                }
                Arguments::Named(v) => {
                    for (_, a, _) in v {
                        self.expression(a)?;
                    }
                }
            },
            ExpressionKind::Await(a)
            | ExpressionKind::Unary { operand: a, .. }
            | ExpressionKind::Is { value: a, .. } => self.expression(a)?,
            ExpressionKind::Binary { left, right, .. } => {
                self.expression(left)?;
                self.expression(right)?;
            }
            ExpressionKind::List(v) | ExpressionKind::FormatString(v) => {
                for a in v {
                    self.expression(a)?;
                }
            }
            ExpressionKind::Map(v) => {
                for (a, b) in v {
                    self.expression(a)?;
                    self.expression(b)?;
                }
            }
            ExpressionKind::Record(v) => {
                for f in v {
                    self.expression(&mut f.value)?;
                }
            }
            ExpressionKind::RecordUpdate { base, fields } => {
                self.expression(base)?;
                for (_, a, _) in fields {
                    self.expression(a)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn block(&mut self, block: &mut Block) -> Result<(), Diagnostic> {
        let saved = self.locals.clone();
        for s in &mut block.statements {
            self.statement(s)?;
        }
        self.locals = saved;
        Ok(())
    }
    fn statement(&mut self, s: &mut Statement) -> Result<(), Diagnostic> {
        match &mut s.kind {
            StatementKind::Local {
                name,
                annotation,
                initializer,
                ..
            } => {
                let ty = annotation.clone().or_else(|| self.infer(initializer));
                self.expression(initializer)?;
                if let Some(ty) = ty {
                    self.locals.insert(name.clone(), ty);
                } else {
                    self.locals.remove(name);
                }
            }
            StatementKind::Assign { name, value } => {
                self.expression(value)?;
                let _ = name;
            }
            StatementKind::Expression(e) | StatementKind::Throw(e) => self.expression(e)?,
            StatementKind::Return(e) => {
                if let Some(e) = e {
                    self.expression(e)?;
                }
            }
            StatementKind::FieldAssign { base, value, .. } => {
                self.expression(base)?;
                self.expression(value)?;
            }
            StatementKind::Block(b) => self.block(b)?,
            StatementKind::If {
                condition,
                then_block,
                else_branch,
            } => {
                self.expression(condition)?;
                self.block(then_block)?;
                if let Some(s) = else_branch {
                    self.statement(s)?;
                }
            }
            StatementKind::While { condition, body } => {
                self.expression(condition)?;
                self.block(body)?;
            }
            StatementKind::For {
                binding,
                iterable,
                body,
                ..
            } => {
                let ty = self.infer(iterable);
                self.expression(iterable)?;
                let saved = self.locals.clone();
                if let Some(TypeRef {
                    kind: TypeRefKind::List(t),
                    ..
                }) = ty
                {
                    self.locals.insert(binding.clone(), *t);
                }
                self.block(body)?;
                self.locals = saved;
            }
            StatementKind::Switch { value, cases } => {
                let ty = self.infer(value);
                self.expression(value)?;
                for case in cases {
                    let saved = self.locals.clone();
                    if let Some(ty) = &ty {
                        self.case_bindings(ty, case);
                    }
                    self.block(&mut case.body)?;
                    self.locals = saved;
                }
            }
            StatementKind::TryErrors {
                body,
                handlers,
                cleanup,
                ..
            } => {
                self.block(body)?;
                for h in handlers {
                    let saved = self.locals.clone();
                    for b in h.pattern.bindings() {
                        self.locals.remove(&b.name);
                    }
                    self.block(&mut h.body)?;
                    self.locals = saved;
                }
                if let Some(b) = cleanup {
                    self.block(b)?;
                }
            }
            StatementKind::TryAll {
                body,
                handler,
                cleanup,
            } => {
                self.block(body)?;
                if let Some(h) = handler {
                    self.block(&mut h.body)?;
                }
                if let Some(b) = cleanup {
                    self.block(b)?;
                }
            }
            StatementKind::TryCatch {
                body,
                handler,
                cleanup,
                ..
            } => {
                self.block(body)?;
                self.block(handler)?;
                if let Some(b) = cleanup {
                    self.block(b)?;
                }
            }
            StatementKind::TryFinally { body, cleanup }
            | StatementKind::TaskScope { body, cleanup } => {
                self.block(body)?;
                self.block(cleanup)?;
            }
            StatementKind::Break | StatementKind::Continue => {}
        }
        Ok(())
    }
    fn case_bindings(&mut self, ty: &TypeRef, case: &SwitchCase) {
        let Some((name, args)) = application(ty) else {
            return;
        };
        if name == "Option" && !self.known.contains_key("Option") {
            if case.path.last().is_some_and(|v| v == "Some")
                && let (Some(binding), Some(arg)) = (case.bindings.first(), args.first())
            {
                self.locals.insert(binding.clone(), arg.clone());
            }
            return;
        }
        let Some(Declaration::Enum(e)) = self.known.get(&name) else {
            return;
        };
        let Some(v) = e
            .variants
            .iter()
            .find(|v| case.path.last() == Some(&v.name))
        else {
            return;
        };
        let values = e
            .type_parameters
            .iter()
            .zip(args)
            .map(|(p, t)| (p.name.clone(), t))
            .collect();
        for (binding, field) in case.bindings.iter().zip(&v.fields) {
            let mut ty = field.ty.clone();
            substitute(&mut ty, &values);
            self.locals.insert(binding.clone(), ty);
        }
    }
}

fn function_type(f: &FunctionDecl, range: SourceRange) -> TypeRef {
    TypeRef {
        range,
        kind: TypeRefKind::Function {
            supplant: false,
            parameters: f.parameters.iter().map(|p| p.ty.clone()).collect(),
            result: Box::new(if f.is_async {
                TypeRef {
                    range,
                    kind: TypeRefKind::Future(Box::new(f.result.clone())),
                }
            } else {
                f.result.clone()
            }),
        },
    }
}

fn unify(formal: &TypeRef, actual: &TypeRef, values: &mut HashMap<String, TypeRef>) {
    match (&formal.kind, &actual.kind) {
        (TypeRefKind::Named(p), _) if p.len() == 1 => {
            values.entry(p[0].clone()).or_insert_with(|| actual.clone());
        }
        (
            TypeRefKind::Generic {
                path: a,
                arguments: aa,
            },
            TypeRefKind::Generic {
                path: b,
                arguments: bb,
            },
        ) if a == b => {
            for (a, b) in aa.iter().zip(bb) {
                unify(a, b, values);
            }
        }
        (TypeRefKind::List(a), TypeRefKind::List(b))
        | (TypeRefKind::Future(a), TypeRefKind::Future(b)) => unify(a, b, values),
        _ => {}
    }
}

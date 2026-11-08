//! Explicit closure conversion: typed state/enum environments and closed dispatch.
//! Executable addresses never become managed values or capture descriptors.
use crate::ast::*;
use crate::ast_walk::Visitor;
use crate::{
    CheckedModule, Diagnostic, DiagnosticCode, FileId, FunctionReferenceType, SourceRange, Type,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
struct Variant {
    target: String,
    captures: Vec<(String, Type)>,
}
#[derive(Clone)]
struct Shape {
    name: String,
    environment: String,
    dispatch: String,
    signature: FunctionReferenceType,
    variants: Vec<Variant>,
}
fn collect(ty: &Type, out: &mut HashSet<FunctionReferenceType>) {
    match ty {
        Type::FunctionReference(s) => {
            if out.insert(s.runtime_shape()) {
                for p in &s.parameters {
                    collect(p, out);
                }
                collect(&s.result, out);
            }
        }
        Type::List(t) | Type::Future(t) => collect(t, out),
        Type::Map(k, v) => {
            collect(k, out);
            collect(v, out);
        }
        _ => {}
    }
}
pub(crate) fn convert(
    checked: &CheckedModule,
) -> Result<Option<(Module, FileId)>, Vec<Diagnostic>> {
    let _timing = crate::timing::span("frontend.closures");
    let mut selected = HashSet::new();
    for schema in checked.source_types.values() {
        for (_, ty) in schema
            .fields
            .iter()
            .chain(schema.variants.iter().flat_map(|(_, fields)| fields))
        {
            collect(ty, &mut selected);
        }
    }
    for f in &checked.functions {
        if matches!(f.result_type, Type::FunctionReference(_)) {
            collect(&f.result_type, &mut selected);
        }
    }
    // Container elements are managed values: lower both code and captures into
    // the existing typed environment graph instead of storing an untraced code
    // index in a List/Map/Future slot.
    for ty in checked
        .expression_types
        .values()
        .chain(checked.functions.iter().flat_map(|f| {
            f.parameter_types
                .iter()
                .chain(std::iter::once(&f.result_type))
        }))
    {
        match ty {
            Type::List(t) | Type::Future(t) => collect(t, &mut selected),
            Type::Map(k, v) => {
                collect(k, &mut selected);
                collect(v, &mut selected);
            }
            _ => {}
        }
    }
    for b in checked.bindings.values() {
        collect(
            &Type::FunctionReference(Box::new(b.signature.clone())),
            &mut selected,
        );
        for (_, ty) in &b.captures {
            collect(ty, &mut selected);
        }
    }
    let annotated = checked
        .type_references
        .values()
        .chain(checked.expression_types.values())
        .any(|ty| matches!(ty, Type::FunctionReference(s) if s.error_bound.is_some()));
    if selected.is_empty() && !annotated {
        return Ok(None);
    }
    let mut signatures = selected.into_iter().collect::<Vec<_>>();
    signatures.sort_by_key(|s| format!("{s:?}"));
    let file = FileId(
        checked
            .syntax
            .declarations
            .iter()
            .map(|d| d.range().file_id.0)
            .chain(checked.expression_types.keys().map(|r| r.file_id.0))
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .expect("closure source ID"),
    );
    let mut shapes = HashMap::new();
    let mut identities = HashSet::new();
    for signature in signatures {
        if signature.supplant {
            return Err(vec![Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                checked
                    .type_references
                    .iter()
                    .filter(|(_, ty)| **ty == Type::FunctionReference(Box::new(signature.clone())))
                    .map(|(range, _)| *range)
                    .min_by_key(|r| (r.file_id.0, r.start_byte, r.instance))
                    .unwrap_or(checked.syntax.range),
                "supplant references cannot be stored as managed closures",
                "keep supplant references in locals and await them directly",
            )]);
        }
        let mut variants = Vec::new();
        for function in &checked.functions {
            let result = if function.facts.is_async {
                Type::Future(Box::new(function.result_type.clone()))
            } else {
                function.result_type.clone()
            };
            if function.parameter_types == signature.parameters && result == *signature.result {
                variants.push(Variant {
                    target: function.declaration.name.clone(),
                    captures: Vec::new(),
                });
            }
        }
        for native in &checked.source_native_signatures {
            let result = crate::sema::source_abi_type(&native.result);
            let result = if native.is_async {
                Type::Future(Box::new(result))
            } else {
                result
            };
            if !native.is_supplant
                && native
                    .parameters
                    .iter()
                    .map(crate::sema::source_abi_type)
                    .collect::<Vec<_>>()
                    == signature.parameters
                && result == *signature.result
            {
                variants.push(Variant {
                    target: native.name.clone(),
                    captures: Vec::new(),
                });
            }
        }
        for binding in checked
            .bindings
            .values()
            .filter(|b| b.signature.same_signature(&signature))
        {
            variants.push(Variant {
                target: binding.target.clone(),
                captures: binding.captures.clone(),
            });
        }
        for (range, target) in &checked.static_references {
            if matches!(checked.expression_types.get(range), Some(Type::FunctionReference(s)) if s.same_signature(&signature))
                && target.starts_with("rinzu.core::")
            {
                variants.push(Variant {
                    target: target.clone(),
                    captures: Vec::new(),
                });
            }
        }
        variants.sort_by_key(|v| (v.target.clone(), v.captures.len()));
        variants.dedup_by(|a, b| a.target == b.target && a.captures == b.captures);
        let name = format!(
            "$rinzu_closure_{:016x}",
            crate::fnv1a64(format!("{signature:?}").as_bytes())
        );
        if !identities.insert(name.clone()) {
            return Err(vec![Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                checked.syntax.range,
                "closure signature identity collision",
                "report the concrete function signatures",
            )]);
        }
        shapes.insert(
            signature.clone(),
            Shape {
                environment: format!("{name}_env"),
                dispatch: format!("{name}_invoke"),
                name,
                signature,
                variants,
            },
        );
    }
    let mut converter = Converter {
        checked,
        shapes,
        file,
        next: 0,
        diagnostics: Vec::new(),
    };
    let mut module = checked.syntax.clone();
    crate::ast_walk::module(&mut module, &mut converter);
    if !converter.diagnostics.is_empty() {
        return Err(converter.diagnostics);
    }
    let mut shapes = converter.shapes.values().cloned().collect::<Vec<_>>();
    shapes.sort_by(|a, b| a.name.cmp(&b.name));
    for shape in shapes {
        module.declarations.extend(converter.declarations(&shape));
    }
    Ok(Some((module, file)))
}
struct Converter<'a> {
    checked: &'a CheckedModule,
    shapes: HashMap<FunctionReferenceType, Shape>,
    file: FileId,
    next: usize,
    diagnostics: Vec<Diagnostic>,
}
impl Converter<'_> {
    fn range(&mut self) -> SourceRange {
        let n = self.next;
        self.next += 2;
        SourceRange::new(self.file, n, n + 1)
    }
    fn expression(&mut self, kind: ExpressionKind) -> Expression {
        Expression {
            kind,
            range: self.range(),
        }
    }
    fn name(&mut self, path: Vec<String>) -> Expression {
        self.expression(ExpressionKind::Name(path))
    }
    fn type_ref(&mut self, ty: &Type) -> TypeRef {
        let kind = match ty {
            Type::FunctionReference(signature)
                if self.shapes.contains_key(&signature.runtime_shape()) =>
            {
                TypeRefKind::Named(vec![self.shapes[&signature.runtime_shape()].name.clone()])
            }
            Type::FunctionReference(s) => TypeRefKind::Function {
                supplant: s.supplant,
                parameters: s.parameters.iter().map(|t| self.type_ref(t)).collect(),
                result: Box::new(self.type_ref(&s.result)),
                error_contract: None,
            },
            Type::List(t) => TypeRefKind::List(Box::new(self.type_ref(t))),
            Type::Map(k, v) => {
                TypeRefKind::Map(Box::new(self.type_ref(k)), Box::new(self.type_ref(v)))
            }
            Type::Future(t) => TypeRefKind::Future(Box::new(self.type_ref(t))),
            Type::FixedBytes(n) => TypeRefKind::ConcreteBytes(*n),
            Type::Native { name, .. } | Type::Named(name) => TypeRefKind::Named(vec![name.clone()]),
            _ => TypeRefKind::Named(vec![ty.to_string()]),
        };
        TypeRef {
            kind,
            range: self.range(),
        }
    }
    fn field(&mut self, name: String, ty: &Type) -> FieldDecl {
        FieldDecl {
            is_const: false,
            is_private: false,
            name,
            ty: self.type_ref(ty),
            range: self.range(),
        }
    }
    fn parameter(&mut self, name: String, ty: &Type) -> Parameter {
        Parameter {
            name,
            ty: self.type_ref(ty),
            range: self.range(),
        }
    }
    fn call_target(target: &str) -> Vec<String> {
        if target == "rinzu.core::utf8ByteLength" {
            vec!["utf8ByteLength".into()]
        } else if target.starts_with("rinzu.core::byteLength:") {
            vec!["byteLength".into()]
        } else {
            vec![target.to_owned()]
        }
    }
    fn value(
        &mut self,
        shape: &Shape,
        target: &str,
        captures: Vec<Expression>,
        range: SourceRange,
    ) -> Expression {
        let index = shape
            .variants
            .iter()
            .position(|v| v.target == target && v.captures.len() == captures.len())
            .expect("closed capture descriptor");
        let variant = &shape.variants[index];
        let fields = variant
            .captures
            .iter()
            .zip(captures)
            .map(|((name, _), e)| (name.clone(), e, self.range()))
            .collect();
        let environment = self.expression(ExpressionKind::Call {
            target: vec![shape.environment.clone(), format!("V{index}")],
            arguments: if variant.captures.is_empty() {
                Arguments::Positional(Vec::new())
            } else {
                Arguments::Named(fields)
            },
        });
        Expression {
            range,
            kind: ExpressionKind::Call {
                target: vec![shape.name.clone()],
                arguments: Arguments::Named(vec![(
                    "environment".into(),
                    environment,
                    self.range(),
                )]),
            },
        }
    }
    fn declarations(&mut self, shape: &Shape) -> Vec<Declaration> {
        let environment = Declaration::Enum(EnumDecl {
            interfaces: Vec::new(),
            name: shape.environment.clone(),
            type_parameters: Vec::new(),
            variants: shape
                .variants
                .iter()
                .enumerate()
                .map(|(i, v)| EnumVariant {
                    name: format!("V{i}"),
                    fields: v
                        .captures
                        .iter()
                        .map(|(n, t)| self.field(n.clone(), t))
                        .collect(),
                    range: self.range(),
                })
                .collect(),
            range: self.range(),
        });
        let function = Declaration::Record(RecordDecl {
            interfaces: Vec::new(),
            mutable: true,
            name: shape.name.clone(),
            type_parameters: Vec::new(),
            fields: vec![self.field(
                "environment".into(),
                &Type::Named(shape.environment.clone()),
            )],
            range: self.range(),
        });
        let function_start = self.next;
        let mut parameters =
            vec![self.parameter("$function".into(), &Type::Named(shape.name.clone()))];
        parameters.extend(
            shape
                .signature
                .parameters
                .iter()
                .enumerate()
                .map(|(i, t)| self.parameter(format!("$arg{i}"), t)),
        );
        let body_start = self.next;
        let value = self.name(vec!["$function".into(), "environment".into()]);
        let cases = shape
            .variants
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let case_start = self.next;
                let bindings = v
                    .captures
                    .iter()
                    .enumerate()
                    .map(|(j, _)| format!("$capture{i}_{j}"))
                    .collect::<Vec<_>>();
                let mut arguments = bindings
                    .iter()
                    .map(|n| self.name(vec![n.clone()]))
                    .collect::<Vec<_>>();
                arguments.extend(
                    shape
                        .signature
                        .parameters
                        .iter()
                        .enumerate()
                        .map(|(j, _)| self.name(vec![format!("$arg{j}")])),
                );
                let call = self.expression(ExpressionKind::Call {
                    target: Self::call_target(&v.target),
                    arguments: Arguments::Positional(arguments),
                });
                SwitchCase {
                    path: vec![shape.environment.clone(), format!("V{i}")],
                    bindings,
                    body: Block {
                        statements: vec![Statement {
                            kind: StatementKind::Return(Some(call)),
                            range: self.range(),
                        }],
                        range: SourceRange::new(self.file, case_start, self.next),
                    },
                    range: self.range(),
                }
            })
            .collect();
        let dispatch = Declaration::Function(FunctionDecl {
            error_contract: None,
            name: shape.dispatch.clone(),
            is_private: false,
            lexical_owner: None,
            owner_type_parameter_count: 0,
            type_parameters: Vec::new(),
            is_async: false,
            parameters,
            result: self.type_ref(&shape.signature.result),
            body: Block {
                statements: vec![Statement {
                    kind: StatementKind::Switch { value, cases },
                    range: self.range(),
                }],
                range: SourceRange::new(self.file, body_start, self.next),
            },
            fallback: None,
            range: SourceRange::new(self.file, function_start, self.next),
        });
        let mut declarations = vec![environment, function, dispatch];
        if *shape.signature.result == Type::Unit || shape.signature.result.scalar().is_some() {
            for (index, variant) in shape.variants.iter().enumerate() {
                let start = self.next;
                let mut parameters =
                    vec![self.parameter("$function".into(), &Type::Named(shape.name.clone()))];
                parameters.extend(
                    shape
                        .signature
                        .parameters
                        .iter()
                        .enumerate()
                        .map(|(i, t)| self.parameter(format!("$arg{i}"), t)),
                );
                let body_start = self.next;
                let mut arguments = vec![self.name(vec!["$function".into()])];
                arguments.extend(
                    shape
                        .signature
                        .parameters
                        .iter()
                        .enumerate()
                        .map(|(i, _)| self.name(vec![format!("$arg{i}")])),
                );
                let call = self.expression(ExpressionKind::Call {
                    target: vec![shape.dispatch.clone()],
                    arguments: Arguments::Positional(arguments),
                });
                let statement = Statement {
                    kind: StatementKind::Return(Some(call)),
                    range: self.range(),
                };
                let fallback = self
                    .checked
                    .functions
                    .iter()
                    .find(|f| f.declaration.name == variant.target)
                    .and_then(|f| f.declaration.fallback.clone());
                declarations.push(Declaration::Function(FunctionDecl {
                    error_contract: None,
                    name: format!("{}_callback_{index}", shape.name),
                    is_private: false,
                    lexical_owner: None,
                    owner_type_parameter_count: 0,
                    type_parameters: Vec::new(),
                    is_async: false,
                    parameters,
                    result: self.type_ref(&shape.signature.result),
                    body: Block {
                        statements: vec![statement],
                        range: SourceRange::new(self.file, body_start, self.next),
                    },
                    fallback,
                    range: SourceRange::new(self.file, start, self.next),
                }));
            }
        }
        declarations
    }
}
impl Visitor for Converter<'_> {
    fn ty(&mut self, ty: &mut TypeRef) {
        if let Some(
            resolved @ (Type::FunctionReference(_)
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Future(_)),
        ) = self.checked.type_references.get(&ty.range)
        {
            // An expanded alias can reuse the same nested source ranges for
            // several type arguments. Lower the complete resolved type at
            // this use site so another instantiation cannot change its shape.
            ty.kind = self.type_ref(resolved).kind;
        } else if let TypeRefKind::Function { error_contract, .. } = &mut ty.kind {
            *error_contract = None;
        }
    }
    fn expression(&mut self, e: &mut Expression) {
        if let ExpressionKind::Bind { arguments, .. } = &e.kind {
            let binding = &self.checked.bindings[&e.range];
            let shape = self.shapes[&binding.signature.runtime_shape()].clone();
            let Arguments::Positional(captures) = arguments else {
                unreachable!()
            };
            *e = self.value(&shape, &binding.target, captures.clone(), e.range);
            return;
        }
        if matches!(e.kind, ExpressionKind::Name(_))
            && let Some(target) = self.checked.static_references.get(&e.range)
            && let Some(Type::FunctionReference(signature)) =
                self.checked.expression_types.get(&e.range)
            && let Some(shape) = self.shapes.get(&signature.runtime_shape()).cloned()
        {
            *e = self.value(&shape, target, Vec::new(), e.range);
            return;
        }
        if let ExpressionKind::Call {
            target,
            arguments: Arguments::Positional(arguments),
        } = &mut e.kind
            && let Some(call) = self.checked.indirect_signatures.get(&e.range)
            && let Some(shape) = self.shapes.get(&call.signature.runtime_shape()).cloned()
        {
            let callee = self.name(target.clone());
            arguments.insert(0, callee);
            *target = vec![shape.dispatch];
        }
        if let ExpressionKind::Call {
            arguments: Arguments::Positional(arguments),
            ..
        } = &mut e.kind
            && let Some(native) = self.checked.source_native_calls.get(&e.range).or_else(|| {
                self.checked.supplant_calls.get(&e.range).and_then(|call| {
                    self.checked
                        .source_native_signatures
                        .iter()
                        .find(|s| call.targets.contains(&s.name))
                })
            })
        {
            for (abi, argument) in native.parameters.iter().zip(arguments) {
                let crate::SourceAbiType::Callback { .. } = abi else {
                    continue;
                };
                let Type::FunctionReference(signature) = crate::sema::source_abi_type(abi) else {
                    unreachable!()
                };
                if !self.shapes.contains_key(signature.as_ref()) {
                    continue;
                }
                if let Some(target) = self.checked.static_references.get(&argument.range) {
                    argument.kind = ExpressionKind::Name(Self::call_target(target));
                } else if *signature.result != Type::Unit && !signature.result.scalar().is_some() {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::CallOnlyEscape, argument.range,
                        "captured native callback result must be Scalar or Unit",
                        "return a copied scalar; static buffer callbacks keep their existing contract",
                    ));
                }
            }
        }
    }
}

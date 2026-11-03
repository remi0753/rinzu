//! Pure module values become rooted locals, never cross-Domain managed globals.
use crate::ast::*;
use crate::{AbiScalar, Diagnostic, DiagnosticCode, SourceRange};
use std::collections::{BTreeMap, HashMap, HashSet};

pub(crate) fn rich_names(module: &Module) -> HashSet<String> {
    let constants = module
        .declarations
        .iter()
        .filter_map(|d| {
            if let Declaration::Constant(c) = d {
                Some(c)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let mut names = HashSet::new();
    for c in &constants {
        let annotation = c.annotation.as_ref().is_some_and(|t| match &t.kind {
            TypeRefKind::Named(path) => {
                path.len() != 1
                    || AbiScalar::from_name(&path[0]).is_none_or(|s| s == AbiScalar::Bool)
            }
            _ => true,
        });
        if annotation
            || matches!(
                c.initializer.kind,
                ExpressionKind::Literal(Literal::Bool(_) | Literal::String(_) | Literal::Bytes(_))
                    | ExpressionKind::List(_)
                    | ExpressionKind::Record(_)
                    | ExpressionKind::Function(_)
                    | ExpressionKind::Map(_)
                    | ExpressionKind::Unit
                    | ExpressionKind::GenericCall { .. }
            )
        {
            names.insert(c.name.clone());
        }
    }
    loop {
        let old = names.len();
        for c in &constants {
            if let ExpressionKind::Name(path) = &c.initializer.kind
                && names.contains(&path.join("."))
            {
                names.insert(c.name.clone());
            }
        }
        if old == names.len() {
            break;
        }
    }
    names
}

pub(crate) fn expand(module: &Module) -> Result<Module, Vec<Diagnostic>> {
    // This also diagnoses duplicate names and every invalid integer initializer.
    crate::evaluate_module_constants(module)?;
    let rich = rich_names(module);
    if rich.is_empty() {
        return Ok(module.clone());
    }
    let constants = module
        .declarations
        .iter()
        .filter_map(|d| {
            if let Declaration::Constant(c) = d {
                Some((c.name.clone(), c.clone()))
            } else {
                None
            }
        })
        .collect::<HashMap<_, _>>();
    let mut validator = Validator {
        constants: &constants,
        rich: &rich,
        module,
        active: HashSet::new(),
        done: HashSet::new(),
    };
    let mut sorted = rich.iter().cloned().collect::<Vec<_>>();
    sorted.sort();
    for name in &sorted {
        validator.constant(name).map_err(|e| vec![e])?;
    }
    let mut types = BTreeMap::new();
    for name in &sorted {
        types.insert(
            name.clone(),
            constant_type(name, &constants, &mut HashSet::new()).map_err(|e| vec![e])?,
        );
    }
    let mut output = module.clone();
    output
        .declarations
        .retain(|d| !matches!(d,Declaration::Constant(c) if rich.contains(&c.name)));
    for name in &sorted {
        let c = &constants[name];
        output
            .declarations
            .push(Declaration::Function(FunctionDecl {
                name: accessor(name),
                is_private: false,
                lexical_owner: None,
                owner_type_parameter_count: 0,
                type_parameters: Vec::new(),
                is_async: false,
                parameters: Vec::new(),
                result: types[name].clone(),
                body: Block {
                    range: c.initializer.range,
                    statements: vec![Statement {
                        range: c.range,
                        kind: StatementKind::Return(Some(c.initializer.clone())),
                    }],
                },
                fallback: None,
                range: c.range,
            }));
    }
    for declaration in &mut output.declarations {
        if let Declaration::Function(f) = declaration {
            let mut used = BTreeMap::new();
            let locals = f.parameters.iter().map(|p| p.name.clone()).collect();
            rewrite_block(&mut f.body, &locals, &rich, &mut used);
            let mut bindings = Vec::new();
            for (name, local) in used {
                let range = SourceRange::empty(
                    constants[&name].range.file_id,
                    constants[&name].range.start_byte as usize,
                );
                bindings.push(Statement {
                    range,
                    kind: StatementKind::Local {
                        mutable: false,
                        name: local,
                        annotation: Some(types[&name].clone()),
                        initializer: Expression {
                            range,
                            kind: ExpressionKind::Call {
                                target: vec![accessor(&name)],
                                arguments: Arguments::Positional(Vec::new()),
                            },
                        },
                    },
                });
            }
            bindings.append(&mut f.body.statements);
            f.body.statements = bindings;
        }
    }
    Ok(output)
}
fn accessor(name: &str) -> String {
    format!("rinzu.constant::{name}")
}
fn invalid(range: SourceRange, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::InvalidConstant,
        range,
        message,
        "use an annotated acyclic literal/immutable constructor and module constants",
    )
}
fn constant_type(
    name: &str,
    constants: &HashMap<String, ConstantDecl>,
    active: &mut HashSet<String>,
) -> Result<TypeRef, Diagnostic> {
    let c = &constants[name];
    if let Some(t) = &c.annotation {
        return Ok(t.clone());
    }
    if !active.insert(name.into()) {
        return Err(invalid(c.range, "value constant dependency cycle"));
    }
    let result = match &c.initializer.kind {
        ExpressionKind::Function(function) => Ok(TypeRef {
            range: c.range,
            kind: TypeRefKind::Function {
                supplant: false,
                parameters: function.parameters.iter().map(|p| p.ty.clone()).collect(),
                result: Box::new(if function.is_async {
                    TypeRef {
                        range: function.result.range,
                        kind: TypeRefKind::Future(Box::new(function.result.clone())),
                    }
                } else {
                    function.result.clone()
                }),
            },
        }),
        ExpressionKind::Record(fields) => {
            let mut types = Vec::new();
            for field in fields {
                let ty = if let Some(t) = &field.annotation {
                    t.clone()
                } else {
                    match &field.value.kind {
                        ExpressionKind::Literal(Literal::Bool(_)) => TypeRef {
                            range: field.value.range,
                            kind: TypeRefKind::Named(vec!["Bool".into()]),
                        },
                        ExpressionKind::Literal(Literal::String(_)) => TypeRef {
                            range: field.value.range,
                            kind: TypeRefKind::Named(vec!["String".into()]),
                        },
                        ExpressionKind::Name(path) if constants.contains_key(&path.join(".")) => {
                            constant_type(&path.join("."), constants, active)?
                        }
                        _ => {
                            return Err(invalid(
                                field.range,
                                "module record field needs an explicit type when its value type is not known",
                            ));
                        }
                    }
                };
                types.push(FieldDecl {
                    name: field.name.clone(),
                    is_const: true,
                    is_private: false,
                    ty,
                    range: field.range,
                });
            }
            Ok(TypeRef {
                range: c.range,
                kind: TypeRefKind::Record(types),
            })
        }
        ExpressionKind::Literal(Literal::Bool(_)) => Ok(TypeRef {
            range: c.range,
            kind: TypeRefKind::Named(vec!["Bool".into()]),
        }),
        ExpressionKind::Literal(Literal::String(_)) => Ok(TypeRef {
            range: c.range,
            kind: TypeRefKind::Named(vec!["String".into()]),
        }),
        ExpressionKind::Name(path) if constants.contains_key(&path.join(".")) => {
            constant_type(&path.join("."), constants, active)
        }
        _ => Err(invalid(
            c.range,
            "aggregate value constants require an explicit type annotation",
        )),
    };
    active.remove(name);
    result
}
struct Validator<'a> {
    constants: &'a HashMap<String, ConstantDecl>,
    rich: &'a HashSet<String>,
    module: &'a Module,
    active: HashSet<String>,
    done: HashSet<String>,
}
impl Validator<'_> {
    fn constant(&mut self, name: &str) -> Result<(), Diagnostic> {
        if !self.rich.contains(name) || self.done.contains(name) {
            return Ok(());
        }
        let c = &self.constants[name];
        if !self.active.insert(name.into()) {
            return Err(invalid(
                c.range,
                format!("value constant dependency cycle at `{name}`"),
            ));
        }
        self.expression(&c.initializer)?;
        self.active.remove(name);
        self.done.insert(name.into());
        Ok(())
    }
    fn constructor(&self, target: &[String], variant: Option<&str>) -> bool {
        if let Some(v) = variant {
            return self.module.declarations.iter().any(|d|matches!(d,Declaration::Enum(e) if e.name==target.join(".") && e.variants.iter().any(|item|item.name==v))) || matches!(target.join(".").as_str(),"Option");
        }
        let name = target.join(".");
        self.module.declarations.iter().any(|d| match d {
            Declaration::Record(r) => {
                r.name == name && (!r.mutable || r.fields.iter().all(|f| f.is_const))
            }
            Declaration::Enum(e) => target.last().is_some_and(|v| {
                e.name == target[..target.len() - 1].join(".")
                    && e.variants.iter().any(|item| &item.name == v)
            }),
            _ => false,
        })
    }
    fn expression(&mut self, e: &Expression) -> Result<(), Diagnostic> {
        match &e.kind {
            ExpressionKind::Function(function) => {
                for (name, _) in
                    crate::lambdas::captures(function, &self.constants.keys().cloned().collect())
                {
                    self.constant(&name)?;
                }
                Ok(())
            }
            ExpressionKind::Record(fields) => {
                for field in fields {
                    self.expression(&field.value)?;
                }
                Ok(())
            }
            ExpressionKind::Literal(
                Literal::Bool(_)
                | Literal::String(_)
                | Literal::Bytes(_)
                | Literal::Int(_)
                | Literal::Unsigned(_)
                | Literal::DecimalInteger(_),
            )
            | ExpressionKind::Unit => Ok(()),
            ExpressionKind::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } if matches!(
                operand.kind,
                ExpressionKind::Literal(
                    Literal::Int(_) | Literal::Unsigned(_) | Literal::DecimalInteger(_)
                )
            ) =>
            {
                Ok(())
            }
            ExpressionKind::Name(path) => {
                // Qualified constants are flattened to one component by package linking.
                if let Some(name) = path
                    .first()
                    .filter(|name| self.constants.contains_key(*name))
                {
                    self.constant(name)
                } else {
                    Err(invalid(
                        e.range,
                        "value constant references a runtime-dependent name",
                    ))
                }
            }
            ExpressionKind::List(values) => {
                for v in values {
                    self.expression(v)?;
                }
                Ok(())
            }
            ExpressionKind::Map(values) => {
                for (k, v) in values {
                    self.expression(k)?;
                    self.expression(v)?;
                }
                Ok(())
            }
            ExpressionKind::Call { target, arguments } if self.constructor(target, None) => {
                self.arguments(arguments)
            }
            ExpressionKind::GenericCall {
                target,
                variant,
                arguments,
                spawn: false,
                ..
            } if self.constructor(target, variant.as_deref()) => self.arguments(arguments),
            _ => Err(invalid(
                e.range,
                "value constant only permits literals, module constants and immutable constructors",
            )),
        }
    }
    fn arguments(&mut self, args: &Arguments) -> Result<(), Diagnostic> {
        match args {
            Arguments::Positional(values) => {
                for v in values {
                    self.expression(v)?;
                }
            }
            Arguments::Named(values) => {
                for (_, v, _) in values {
                    self.expression(v)?;
                }
            }
        }
        Ok(())
    }
}
fn rewrite_block(
    block: &mut Block,
    outer: &HashSet<String>,
    rich: &HashSet<String>,
    used: &mut BTreeMap<String, String>,
) {
    let mut locals = outer.clone();
    for statement in &mut block.statements {
        rewrite_statement(statement, &mut locals, rich, used);
    }
}
fn rewrite_statement(
    s: &mut Statement,
    locals: &mut HashSet<String>,
    rich: &HashSet<String>,
    used: &mut BTreeMap<String, String>,
) {
    use StatementKind::*;
    match &mut s.kind {
        Block(b) => rewrite_block(b, locals, rich, used),
        Local {
            name, initializer, ..
        } => {
            rewrite_expression(initializer, locals, rich, used);
            locals.insert(name.clone());
        }
        FieldAssign { base, value, .. } => {
            rewrite_expression(base, locals, rich, used);
            rewrite_expression(value, locals, rich, used);
        }
        Assign { value, .. } | Expression(value) | Throw(value) => {
            rewrite_expression(value, locals, rich, used)
        }
        Return(v) => {
            if let Some(v) = v {
                rewrite_expression(v, locals, rich, used);
            }
        }
        If {
            condition,
            then_block,
            else_branch,
        } => {
            rewrite_expression(condition, locals, rich, used);
            rewrite_block(then_block, locals, rich, used);
            if let Some(branch) = else_branch {
                rewrite_statement(branch, &mut locals.clone(), rich, used);
            }
        }
        While { condition, body } => {
            rewrite_expression(condition, locals, rich, used);
            rewrite_block(body, locals, rich, used);
        }
        For {
            binding,
            iterable,
            body,
            ..
        } => {
            rewrite_expression(iterable, locals, rich, used);
            let mut scope = locals.clone();
            scope.insert(binding.clone());
            rewrite_block(body, &scope, rich, used);
        }
        Switch { value, cases } => {
            rewrite_expression(value, locals, rich, used);
            for c in cases {
                let mut scope = locals.clone();
                scope.extend(c.bindings.iter().cloned());
                rewrite_block(&mut c.body, &scope, rich, used);
            }
        }
        TryErrors {
            body,
            handlers,
            cleanup,
            ..
        } => {
            rewrite_block(body, locals, rich, used);
            for handler in handlers {
                let mut names = locals.clone();
                names.extend(
                    handler
                        .pattern
                        .bindings()
                        .map(|binding| binding.name.clone()),
                );
                rewrite_block(&mut handler.body, &names, rich, used);
            }
            if let Some(cleanup) = cleanup {
                rewrite_block(cleanup, locals, rich, used);
            }
        }
        TryAll {
            body,
            handler,
            cleanup,
        } => {
            rewrite_block(body, locals, rich, used);
            if let Some(h) = handler {
                let mut names = locals.clone();
                names.insert(h.binding.clone());
                rewrite_block(&mut h.body, &names, rich, used);
            }
            if let Some(c) = cleanup {
                rewrite_block(c, locals, rich, used);
            }
        }
        TryCatch {
            body,
            binding,
            handler,
            cleanup,
            ..
        } => {
            rewrite_block(body, locals, rich, used);
            let mut scope = locals.clone();
            scope.insert(binding.clone());
            rewrite_block(handler, &scope, rich, used);
            if let Some(c) = cleanup {
                rewrite_block(c, locals, rich, used);
            }
        }
        TryFinally { body, cleanup } | TaskScope { body, cleanup } => {
            rewrite_block(body, locals, rich, used);
            rewrite_block(cleanup, locals, rich, used);
        }
        Break | Continue => {}
    }
}
fn rewrite_expression(
    e: &mut Expression,
    locals: &HashSet<String>,
    rich: &HashSet<String>,
    used: &mut BTreeMap<String, String>,
) {
    match &mut e.kind {
        ExpressionKind::Function(f) => {
            let mut local = locals.clone();
            for p in &f.parameters {
                local.insert(p.name.clone());
            }
            rewrite_block(&mut f.body, &local, rich, used);
            if let Some(e) = &mut f.fallback {
                rewrite_expression(e, &local, rich, used);
            }
        }
        ExpressionKind::Record(fields) => {
            for f in fields {
                rewrite_expression(&mut f.value, locals, rich, used);
            }
        }
        ExpressionKind::Name(path) => {
            if let Some(name) = path
                .first()
                .filter(|name| rich.contains(*name) && !locals.contains(*name))
                .cloned()
            {
                let local = format!("$rinzu_const_{}", used.len());
                path[0] = used.entry(name).or_insert(local).clone();
            }
        }
        ExpressionKind::Call { target, arguments }
        | ExpressionKind::Bind {
            target, arguments, ..
        }
        | ExpressionKind::GenericCall {
            target, arguments, ..
        }
        | ExpressionKind::Spawn { target, arguments } => {
            if let Some(name) = target
                .first()
                .filter(|name| rich.contains(*name) && !locals.contains(*name))
                .cloned()
            {
                let local = format!("$rinzu_const_{}", used.len());
                target[0] = used.entry(name).or_insert(local).clone();
            }
            match arguments {
                Arguments::Positional(values) => {
                    for v in values {
                        rewrite_expression(v, locals, rich, used);
                    }
                }
                Arguments::Named(values) => {
                    for (_, v, _) in values {
                        rewrite_expression(v, locals, rich, used);
                    }
                }
            }
        }
        ExpressionKind::List(values) | ExpressionKind::FormatString(values) => {
            for v in values {
                rewrite_expression(v, locals, rich, used);
            }
        }
        ExpressionKind::Map(values) => {
            for (k, v) in values {
                rewrite_expression(k, locals, rich, used);
                rewrite_expression(v, locals, rich, used);
            }
        }
        ExpressionKind::Unary { operand, .. } | ExpressionKind::Await(operand) => {
            rewrite_expression(operand, locals, rich, used)
        }
        ExpressionKind::Is { value, .. } => rewrite_expression(value, locals, rich, used),
        ExpressionKind::Binary { left, right, .. } => {
            rewrite_expression(left, locals, rich, used);
            rewrite_expression(right, locals, rich, used);
        }
        ExpressionKind::Literal(_)
        | ExpressionKind::Unit
        | ExpressionKind::GenericReference { .. } => {}
        ExpressionKind::RecordUpdate { base, fields } => {
            rewrite_expression(base, locals, rich, used);
            for (_, value, _) in fields {
                rewrite_expression(value, locals, rich, used);
            }
        }
    }
}

//! Concrete frontend specialization. Templates never enter verified IR.
use crate::ast::*;
use crate::{AbiScalar, Diagnostic, DiagnosticCode, SourceRange};
use std::collections::{HashMap, HashSet};
mod inference;
mod methods;

#[derive(Clone, Debug)]
pub(crate) struct Requirement {
    pub ty: TypeRef,
    pub marker: String,
    pub range: SourceRange,
    pub instance: String,
}

pub(crate) fn expand(
    module: &Module,
    _is_library: bool,
) -> Result<(Module, Vec<Requirement>), Vec<Diagnostic>> {
    let _timing = crate::timing::span("frontend.generics");
    Expander::new(module).run().map_err(|error| vec![error])
}

fn parameters(declaration: &Declaration) -> &[TypeParameter] {
    match declaration {
        Declaration::Function(value) => &value.type_parameters,
        Declaration::Record(value) => &value.type_parameters,
        Declaration::Enum(value) => &value.type_parameters,
        Declaration::Error(value) => &value.type_parameters,
        Declaration::Constant(_) | Declaration::TypeAlias(_) => &[],
    }
}

struct Expander {
    module: Module,
    templates: HashMap<String, Declaration>,
    instances: HashMap<String, String>,
    output: Vec<Declaration>,
    requirements: Vec<Requirement>,
    depth: usize,
    next_instance: u32,
    constants: std::collections::BTreeMap<String, crate::IntegerConstant>,
    module_constants: std::collections::BTreeMap<String, crate::IntegerConstant>,
    locals: HashSet<String>,
    known: HashMap<String, Declaration>,
    type_locals: HashMap<String, TypeRef>,
    expected_result: Option<TypeRef>,
    lexical_owner: Option<String>,
    inference_active: bool,
    instance_types: HashMap<String, (String, Vec<TypeRef>)>,
}

impl Expander {
    fn new(module: &Module) -> Self {
        let constants = crate::evaluate_module_constants(module).unwrap_or_default();
        let mut module = module.clone();
        // Prelude templates use ordinary enum specialization and can be shadowed by
        // module declarations. Only Option is provided by the language.
        for (name, params, variants) in [(
            "Option",
            vec!["T"],
            vec![("None", None), ("Some", Some(("value", "T")))],
        )] {
            if module.declarations.iter().any(|d| d.name() == name) {
                continue;
            }
            let range = module.range;
            module.declarations.push(Declaration::Enum(EnumDecl {
                name: name.to_owned(),
                type_parameters: params
                    .into_iter()
                    .map(|name| TypeParameter {
                        name: name.to_owned(),
                        constraints: Vec::new(),
                        range,
                    })
                    .collect(),
                variants: variants
                    .into_iter()
                    .map(|(name, field)| EnumVariant {
                        name: name.to_owned(),
                        range,
                        fields: field
                            .into_iter()
                            .map(|(name, ty)| FieldDecl {
                                is_const: true,
                                is_private: false,
                                name: name.to_owned(),
                                ty: TypeRef {
                                    kind: TypeRefKind::Named(vec![ty.to_owned()]),
                                    range,
                                },
                                range,
                            })
                            .collect(),
                    })
                    .collect(),
                range,
            }));
        }
        let known = module
            .declarations
            .iter()
            .map(|d| (d.name().to_owned(), d.clone()))
            .collect();
        Self {
            module,
            templates: HashMap::new(),
            instances: HashMap::new(),
            output: Vec::new(),
            requirements: Vec::new(),
            depth: 0,
            next_instance: 1,
            constants: constants.clone(),
            module_constants: constants,
            locals: HashSet::new(),
            known,
            type_locals: HashMap::new(),
            expected_result: None,
            lexical_owner: None,
            inference_active: false,
            instance_types: HashMap::new(),
        }
    }

    fn run(mut self) -> Result<(Module, Vec<Requirement>), Diagnostic> {
        let declarations = std::mem::take(&mut self.module.declarations);
        let mut names = HashSet::new();
        for declaration in &declarations {
            if !names.insert(declaration.name()) {
                return Err(Diagnostic::error(
                    DiagnosticCode::DuplicateName,
                    declaration.range(),
                    "duplicate declaration before generic expansion",
                    "use a unique declaration name",
                ));
            }
            let mut seen = HashSet::new();
            for parameter in parameters(declaration) {
                if !seen.insert(&parameter.name) {
                    return Err(error(parameter.range, "duplicate type parameter"));
                }
                for marker in &parameter.constraints {
                    if !matches!(marker.as_str(), "Transferable" | "Equality" | "Key") {
                        return Err(error(
                            parameter.range,
                            &format!("unknown sealed constraint `{marker}`"),
                        ));
                    }
                }
            }
            if !parameters(declaration).is_empty() {
                self.templates
                    .insert(declaration.name().to_owned(), declaration.clone());
            }
        }
        self.module
            .exports
            .retain(|export| !self.templates.contains_key(&export.path.join(".")));
        let mut native = std::mem::take(&mut self.module.native_blocks);
        for block in &mut native {
            for member in &mut block.members {
                match member {
                    NativeMember::Record(record) => {
                        if !record.type_parameters.is_empty() {
                            return Err(error(
                                record.range,
                                "native record cannot declare type parameters",
                            ));
                        }
                        for field in &mut record.fields {
                            self.ty(&mut field.ty, &HashMap::new(), 0)?;
                        }
                    }
                    NativeMember::Function(function) => {
                        for parameter in &mut function.parameters {
                            self.ty(&mut parameter.ty, &HashMap::new(), 0)?;
                        }
                        self.ty(&mut function.result, &HashMap::new(), 0)?;
                    }
                    _ => {}
                }
            }
        }
        self.module.native_blocks = native;
        for mut declaration in declarations {
            if parameters(&declaration).is_empty() {
                self.declaration(&mut declaration, &HashMap::new(), 0)?;
                self.output.push(declaration);
            }
        }
        self.module.declarations = self.output;
        Ok((self.module, self.requirements))
    }

    fn instantiate(
        &mut self,
        name: &str,
        args: &[TypeRef],
        range: SourceRange,
    ) -> Result<String, Diagnostic> {
        let Some(mut declaration) = self.templates.get(name).cloned() else {
            return Err(error(
                range,
                &format!("`{name}` is not a generic declaration"),
            ));
        };
        let parameters = parameters(&declaration).to_vec();
        if args.len() != parameters.len() {
            return Err(error(
                range,
                &format!(
                    "`{name}` expects {} type arguments, got {}",
                    parameters.len(),
                    args.len()
                ),
            ));
        }
        let identity = if let Declaration::Function(f) = &declaration
            && let Some((owner, member)) = crate::methods::parts(name)
        {
            let count = f.owner_type_parameter_count;
            let owner = if count > 0 {
                format!(
                    "{owner}<{}>",
                    args[..count]
                        .iter()
                        .map(spelling)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            } else {
                owner.to_owned()
            };
            let member = if count < args.len() {
                format!(
                    "{member}<{}>",
                    args[count..]
                        .iter()
                        .map(spelling)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            } else {
                member.to_owned()
            };
            format!("{owner}.{member}")
        } else {
            format!(
                "{name}<{}>",
                args.iter().map(spelling).collect::<Vec<_>>().join(",")
            )
        };
        if let Some(instance) = self.instances.get(&identity) {
            return Ok(instance.clone());
        }
        if self.instances.len() >= 256 || self.depth >= 32 {
            return Err(error(
                range,
                "generic specialization limit exceeded (256 instances / depth 32)",
            ));
        }
        self.instances.insert(identity.clone(), identity.clone());
        self.instance_types
            .insert(identity.clone(), (name.to_owned(), args.to_vec()));
        if let Declaration::Function(f) = &declaration
            && f.owner_type_parameter_count > 0
            && let Some((owner, _)) = crate::methods::parts(name)
        {
            self.instantiate(owner, &args[..f.owner_type_parameter_count], range)?;
        }
        let substitution = parameters
            .iter()
            .zip(args)
            .map(|(p, ty)| (p.name.clone(), ty.clone()))
            .collect();
        let instance = self.next_instance;
        self.next_instance += 1;
        for (parameter, ty) in parameters.iter().zip(args) {
            for marker in &parameter.constraints {
                self.requirements.push(Requirement {
                    ty: ty.clone(),
                    marker: marker.clone(),
                    range,
                    instance: identity.clone(),
                });
            }
        }
        match &mut declaration {
            Declaration::Function(value) => {
                value.name = identity.clone();
                value.type_parameters.clear();
                value.owner_type_parameter_count = 0;
            }
            Declaration::Record(value) => {
                value.name = identity.clone();
                value.type_parameters.clear();
            }
            Declaration::Enum(value) => {
                value.name = identity.clone();
                value.type_parameters.clear();
            }
            Declaration::Error(value) => {
                value.name = identity.clone();
                value.type_parameters.clear();
            }
            Declaration::Constant(_) | Declaration::TypeAlias(_) => {
                unreachable!("aliases expanded before generics")
            }
        }
        self.depth += 1;
        let result = self.declaration(&mut declaration, &substitution, instance);
        self.depth -= 1;
        result?;
        self.output.push(declaration);
        Ok(identity)
    }

    fn ty(
        &mut self,
        ty: &mut TypeRef,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
    ) -> Result<(), Diagnostic> {
        ty.range.instance = instance;
        match &mut ty.kind {
            TypeRefKind::Record(fields) => {
                for field in fields {
                    self.ty(&mut field.ty, substitution, instance)?;
                }
            }
            TypeRefKind::Named(path) => {
                if path.len() == 1
                    && let Some(concrete) = substitution.get(&path[0])
                {
                    ty.kind = concrete.kind.clone();
                } else if self.templates.contains_key(&path.join(".")) {
                    return Err(error(
                        ty.range,
                        "generic type requires explicit type arguments",
                    ));
                } else if path.len() == 1
                    && let Some(scalar) = AbiScalar::from_name(&path[0])
                {
                    path[0] = match scalar {
                        AbiScalar::I64 => "Int",
                        AbiScalar::F64 => "Float",
                        AbiScalar::Bool => "Bool",
                        _ => scalar.name(),
                    }
                    .to_owned();
                }
            }
            TypeRefKind::Generic { path, arguments } => {
                for argument in arguments.iter_mut() {
                    self.ty(argument, substitution, instance)?;
                    self.canonical_argument(argument)?;
                }
                let name = self.instantiate(&path.join("."), arguments, ty.range)?;
                ty.kind = TypeRefKind::Named(vec![name]);
            }
            TypeRefKind::List(child) | TypeRefKind::Future(child) => {
                self.ty(child, substitution, instance)?
            }
            TypeRefKind::Map(key, value) => {
                self.ty(key, substitution, instance)?;
                self.ty(value, substitution, instance)?;
            }
            TypeRefKind::Function {
                parameters, result, ..
            } => {
                for parameter in parameters {
                    self.ty(parameter, substitution, instance)?;
                }
                self.ty(result, substitution, instance)?;
            }
            TypeRefKind::Bytes(length) => {
                self.expression(length, substitution, instance)?;
            }
            TypeRefKind::ConcreteBytes(_) => {}
        }
        Ok(())
    }

    fn canonical_argument(&self, ty: &mut TypeRef) -> Result<(), Diagnostic> {
        match &mut ty.kind {
            TypeRefKind::Record(fields) => {
                for field in fields.iter_mut() {
                    self.canonical_argument(&mut field.ty)?;
                }
                fields.sort_by(|a, b| a.name.cmp(&b.name));
            }
            TypeRefKind::Bytes(length) => {
                let value =
                    crate::constants::evaluate_integer_expression(length, None, &self.constants)
                        .or_else(|_| {
                            crate::constants::evaluate_integer_expression(
                                length,
                                Some(AbiScalar::U64),
                                &self.constants,
                            )
                        })?;
                if !(0..=u64::MAX as i128).contains(&value.value) {
                    return Err(error(
                        ty.range,
                        "generic fixed Bytes length is outside CSize range",
                    ));
                }
                ty.kind = TypeRefKind::ConcreteBytes(value.value as u64);
            }
            TypeRefKind::List(inner) | TypeRefKind::Future(inner) => {
                self.canonical_argument(inner)?
            }
            TypeRefKind::Map(key, value) => {
                self.canonical_argument(key)?;
                self.canonical_argument(value)?;
            }
            TypeRefKind::Function {
                parameters, result, ..
            } => {
                for parameter in parameters {
                    self.canonical_argument(parameter)?;
                }
                self.canonical_argument(result)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn declaration(
        &mut self,
        declaration: &mut Declaration,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
    ) -> Result<(), Diagnostic> {
        let saved_constants = std::mem::replace(&mut self.constants, self.module_constants.clone());
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_types = std::mem::take(&mut self.type_locals);
        let saved_result = self.expected_result.take();
        let saved_owner = self.lexical_owner.take();
        let saved_active = self.inference_active;
        match declaration {
            Declaration::TypeAlias(alias) => self.ty(&mut alias.ty, substitution, instance)?,
            Declaration::Error(error) => {
                error.range.instance = instance;
                if let Some(payload) = &mut error.payload {
                    payload.range.instance = instance;
                    self.ty(&mut payload.ty, substitution, instance)?;
                }
            }
            Declaration::Function(value) => {
                self.inference_active = self.needs_inference(&value.body);
                self.lexical_owner = value.lexical_owner.clone();
                for parameter in &value.parameters {
                    self.constants.remove(&parameter.name);
                    self.locals.insert(parameter.name.clone());
                }
                value.range.instance = instance;
                for parameter in &mut value.parameters {
                    parameter.range.instance = instance;
                    self.ty(&mut parameter.ty, substitution, instance)?;
                    self.type_locals
                        .insert(parameter.name.clone(), parameter.ty.clone());
                }
                self.ty(&mut value.result, substitution, instance)?;
                self.expected_result = Some(value.result.clone());
                self.known
                    .insert(value.name.clone(), Declaration::Function(value.clone()));
                self.block(&mut value.body, substitution, instance)?;
                if let Some(fallback) = &mut value.fallback {
                    self.expression(fallback, substitution, instance)?;
                }
            }
            Declaration::Record(value) => {
                value.range.instance = instance;
                for field in &mut value.fields {
                    field.range.instance = instance;
                    self.ty(&mut field.ty, substitution, instance)?;
                }
            }
            Declaration::Enum(value) => {
                value.range.instance = instance;
                for variant in &mut value.variants {
                    variant.range.instance = instance;
                    for field in &mut variant.fields {
                        field.range.instance = instance;
                        self.ty(&mut field.ty, substitution, instance)?;
                    }
                }
            }
            Declaration::Constant(value) => {
                if let Some(ty) = &mut value.annotation {
                    self.ty(ty, substitution, instance)?;
                }
                self.expression(&mut value.initializer, substitution, instance)?;
            }
        }
        self.constants = saved_constants;
        self.locals = saved_locals;
        self.type_locals = saved_types;
        self.expected_result = saved_result;
        self.lexical_owner = saved_owner;
        self.inference_active = saved_active;
        Ok(())
    }

    fn block(
        &mut self,
        block: &mut Block,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
    ) -> Result<(), Diagnostic> {
        let saved_constants = self.constants.clone();
        let saved_locals = self.locals.clone();
        let saved_types = self.type_locals.clone();
        block.range.instance = instance;
        for statement in &mut block.statements {
            self.statement(statement, substitution, instance)?;
        }
        self.constants = saved_constants;
        self.locals = saved_locals;
        self.type_locals = saved_types;
        Ok(())
    }

    fn statement(
        &mut self,
        statement: &mut Statement,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
    ) -> Result<(), Diagnostic> {
        statement.range.instance = instance;
        match &mut statement.kind {
            StatementKind::Block(block) => self.block(block, substitution, instance)?,
            StatementKind::Local {
                annotation,
                initializer,
                name,
                mutable,
            } => {
                if let Some(ty) = annotation {
                    self.ty(ty, substitution, instance)?;
                }
                self.expression_expected(initializer, substitution, instance, annotation.as_ref())?;
                if self.inference_active
                    && let Some(ty) = annotation
                        .clone()
                        .or_else(|| self.probe_type(initializer, None))
                {
                    self.type_locals.insert(name.clone(), ty);
                }
                let expected = annotation.as_ref().and_then(|ty| match &ty.kind {
                    TypeRefKind::Named(path) if path.len() == 1 => AbiScalar::from_name(&path[0])
                        .filter(|scalar| scalar.integer_bounds().is_some()),
                    _ => None,
                });
                let value = if !*mutable && (annotation.is_none() || expected.is_some()) {
                    crate::constants::evaluate_integer_expression(
                        initializer,
                        expected,
                        &self.constants,
                    )
                    .ok()
                } else {
                    None
                };
                self.constants.remove(name);
                if let Some(value) = value {
                    self.constants.insert(name.clone(), value);
                }
                self.locals.insert(name.clone());
            }
            StatementKind::FieldAssign { base, value, .. } => {
                self.expression(base, substitution, instance)?;
                self.expression(value, substitution, instance)?;
            }
            StatementKind::Assign { name, value } => {
                let expected = self.type_locals.get(name).cloned();
                self.expression_expected(value, substitution, instance, expected.as_ref())?;
            }
            StatementKind::Expression(value) | StatementKind::Throw(value) => {
                self.expression(value, substitution, instance)?
            }
            StatementKind::Return(value) => {
                if let Some(value) = value {
                    let expected = self.expected_result.clone();
                    self.expression_expected(value, substitution, instance, expected.as_ref())?;
                }
            }
            StatementKind::If {
                condition,
                then_block,
                else_branch,
            } => {
                self.expression(condition, substitution, instance)?;
                self.block(then_block, substitution, instance)?;
                if let Some(branch) = else_branch {
                    self.statement(branch, substitution, instance)?;
                }
            }
            StatementKind::While { condition, body } => {
                self.expression(condition, substitution, instance)?;
                self.block(body, substitution, instance)?;
            }
            StatementKind::For {
                binding,
                iterable,
                binding_range,
                body,
                ..
            } => {
                binding_range.instance = instance;
                self.expression(iterable, substitution, instance)?;
                let saved_constants = self.constants.clone();
                let saved_locals = self.locals.clone();
                self.constants.remove(binding);
                self.locals.insert(binding.clone());
                let saved_types = self.type_locals.clone();
                if self.inference_active
                    && let Some(TypeRef {
                        kind: TypeRefKind::List(element),
                        ..
                    }) = self.probe_type(iterable, None)
                {
                    self.type_locals.insert(binding.clone(), *element);
                }
                self.block(body, substitution, instance)?;
                self.type_locals = saved_types;
                self.constants = saved_constants;
                self.locals = saved_locals;
            }
            StatementKind::Switch { value, cases } => {
                self.expression(value, substitution, instance)?;
                for case in cases {
                    let saved_constants = self.constants.clone();
                    let saved_locals = self.locals.clone();
                    for binding in &case.bindings {
                        self.constants.remove(binding);
                        self.locals.insert(binding.clone());
                    }
                    case.range.instance = instance;
                    self.block(&mut case.body, substitution, instance)?;
                    self.constants = saved_constants;
                    self.locals = saved_locals;
                }
            }
            StatementKind::TryErrors {
                body,
                handlers,
                cleanup,
                ..
            } => {
                self.block(body, substitution, instance)?;
                for handler in handlers {
                    handler.range.instance = instance;
                    match &mut handler.pattern {
                        CatchPattern::Binding(binding) => binding.range.instance = instance,
                        CatchPattern::List {
                            elements,
                            rest,
                            range,
                        } => {
                            range.instance = instance;
                            for element in elements {
                                element.range.instance = instance;
                                if let Some(binding) = &mut element.binding {
                                    binding.range.instance = instance;
                                }
                                self.ty(&mut element.ty, substitution, instance)?;
                            }
                            if let Some(rest) = rest {
                                rest.range.instance = instance;
                            }
                        }
                    }
                    let saved = self.constants.clone();
                    let saved_locals = self.locals.clone();
                    for binding in handler.pattern.bindings() {
                        self.constants.remove(&binding.name);
                        self.locals.insert(binding.name.clone());
                    }
                    self.block(&mut handler.body, substitution, instance)?;
                    self.constants = saved;
                    self.locals = saved_locals;
                }
                if let Some(cleanup) = cleanup {
                    self.block(cleanup, substitution, instance)?;
                }
            }
            StatementKind::TryAll {
                body,
                handler,
                cleanup,
            } => {
                self.block(body, substitution, instance)?;
                if let Some(h) = handler {
                    h.binding_range.instance = instance;
                    let saved = self.constants.clone();
                    let saved_locals = self.locals.clone();
                    self.constants.remove(&h.binding);
                    self.locals.insert(h.binding.clone());
                    self.block(&mut h.body, substitution, instance)?;
                    self.constants = saved;
                    self.locals = saved_locals;
                }
                if let Some(c) = cleanup {
                    self.block(c, substitution, instance)?;
                }
            }
            StatementKind::TryCatch {
                body,
                binding_range,
                binding,
                handler,
                cleanup,
                ..
            } => {
                binding_range.instance = instance;
                self.block(body, substitution, instance)?;
                let saved_constants = self.constants.clone();
                let saved_locals = self.locals.clone();
                self.constants.remove(binding);
                self.locals.insert(binding.clone());
                self.block(handler, substitution, instance)?;
                self.constants = saved_constants;
                self.locals = saved_locals;
                if let Some(cleanup) = cleanup {
                    self.block(cleanup, substitution, instance)?;
                }
            }
            StatementKind::TryFinally { body, cleanup }
            | StatementKind::TaskScope { body, cleanup } => {
                self.block(body, substitution, instance)?;
                self.block(cleanup, substitution, instance)?;
            }
            StatementKind::Break | StatementKind::Continue => {}
        }
        Ok(())
    }

    fn arguments(
        &mut self,
        arguments: &mut Arguments,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
    ) -> Result<(), Diagnostic> {
        match arguments {
            Arguments::Positional(values) => {
                for value in values {
                    self.expression(value, substitution, instance)?;
                }
            }
            Arguments::Named(values) => {
                for (_, value, range) in values {
                    range.instance = instance;
                    self.expression(value, substitution, instance)?;
                }
            }
        }
        Ok(())
    }

    fn expression(
        &mut self,
        expression: &mut Expression,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
    ) -> Result<(), Diagnostic> {
        self.expression_expected(expression, substitution, instance, None)
    }

    fn expression_expected(
        &mut self,
        expression: &mut Expression,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
        expected: Option<&TypeRef>,
    ) -> Result<(), Diagnostic> {
        expression.range.instance = instance;
        if self.method_expression(expression, substitution, instance, expected)? {
            return Ok(());
        }
        if let ExpressionKind::Call { target, arguments }
        | ExpressionKind::Spawn { target, arguments } = &mut expression.kind
            && !self.local_target(target)
        {
            let template = if self.templates.contains_key(&target.join(".")) {
                Some((target.join("."), None))
            } else if target.len() > 1
                && self
                    .templates
                    .contains_key(&target[..target.len() - 1].join("."))
            {
                Some((target[..target.len() - 1].join("."), target.last().cloned()))
            } else {
                None
            };
            if let Some((name, variant)) = template {
                let type_arguments = self.inferred_arguments(
                    &name,
                    variant.as_deref(),
                    arguments,
                    expected,
                    substitution,
                    instance,
                    expression.range,
                    &[],
                )?;
                let arguments = arguments.clone();
                let spawn = matches!(expression.kind, ExpressionKind::Spawn { .. });
                expression.kind = ExpressionKind::GenericCall {
                    target: vec![name],
                    type_arguments,
                    variant,
                    member_type_arguments: Vec::new(),
                    arguments,
                    spawn,
                };
            }
        }
        match &mut expression.kind {
            ExpressionKind::Function(f) => {
                let saved_constants = self.constants.clone();
                let saved_locals = self.locals.clone();
                let saved_types = self.type_locals.clone();
                let saved_result = self.expected_result.clone();
                for parameter in &f.parameters {
                    self.constants.remove(&parameter.name);
                    self.locals.insert(parameter.name.clone());
                }
                for p in &mut f.parameters {
                    self.ty(&mut p.ty, substitution, instance)?;
                    self.type_locals.insert(p.name.clone(), p.ty.clone());
                }
                self.ty(&mut f.result, substitution, instance)?;
                self.expected_result = Some(f.result.clone());
                self.block(&mut f.body, substitution, instance)?;
                if let Some(e) = &mut f.fallback {
                    self.expression(e, substitution, instance)?;
                }
                self.constants = saved_constants;
                self.locals = saved_locals;
                self.type_locals = saved_types;
                self.expected_result = saved_result;
                f.range.instance = instance;
            }
            ExpressionKind::Record(fields) => {
                for field in fields {
                    field.range.instance = instance;
                    if let Some(t) = &mut field.annotation {
                        self.ty(t, substitution, instance)?;
                    }
                    let contextual = expected.and_then(|t| match &t.kind {
                        TypeRefKind::Record(fields) => {
                            fields.iter().find(|f| f.name == field.name).map(|f| &f.ty)
                        }
                        _ => None,
                    });
                    self.expression_expected(
                        &mut field.value,
                        substitution,
                        instance,
                        field.annotation.as_ref().or(contextual),
                    )?;
                }
            }
            ExpressionKind::Bind {
                target,
                type_arguments,
                arguments,
            } => {
                if !type_arguments.is_empty() {
                    self.require_template_visible(target, expression.range)?;
                    for argument in type_arguments.iter_mut() {
                        self.ty(argument, substitution, instance)?;
                        self.canonical_argument(argument)?;
                    }
                    let name =
                        self.instantiate(&target.join("."), type_arguments, expression.range)?;
                    *target = vec![name];
                    type_arguments.clear();
                } else if !self.local_target(target)
                    && self.templates.contains_key(&target.join("."))
                {
                    return Err(error(
                        expression.range,
                        "generic bind requires explicit type arguments",
                    ));
                }
                self.arguments(arguments, substitution, instance)?;
            }
            ExpressionKind::GenericCall {
                target,
                type_arguments,
                variant,
                member_type_arguments,
                arguments,
                spawn,
            } => {
                if !member_type_arguments.is_empty() {
                    return Err(error(
                        expression.range,
                        "this generic invocation has no generic member",
                    ));
                }
                self.require_template_visible(target, expression.range)?;
                for argument in type_arguments.iter_mut() {
                    self.ty(argument, substitution, instance)?;
                    self.canonical_argument(argument)?;
                }
                let name = self.instantiate(&target.join("."), type_arguments, expression.range)?;
                let mut target = vec![name];
                if let Some(variant) = variant {
                    target.push(variant.clone());
                }
                self.arguments_expected(&target, arguments, substitution, instance)?;
                expression.kind = if *spawn {
                    ExpressionKind::Spawn {
                        target,
                        arguments: arguments.clone(),
                    }
                } else {
                    ExpressionKind::Call {
                        target,
                        arguments: arguments.clone(),
                    }
                };
            }
            ExpressionKind::GenericReference {
                target,
                type_arguments,
                member,
                ..
            } => {
                if self.local_target(target) || member.is_some() {
                    return Err(error(
                        expression.range,
                        "use a concrete unbound function or method reference",
                    ));
                }
                for t in type_arguments.iter_mut() {
                    self.ty(t, substitution, instance)?;
                    self.canonical_argument(t)?;
                }
                let name = self.instantiate(&target.join("."), type_arguments, expression.range)?;
                expression.kind = ExpressionKind::Name(vec![name]);
            }
            ExpressionKind::Call { target, arguments }
            | ExpressionKind::Spawn { target, arguments } => {
                if !self.local_target(target)
                    && (self.templates.contains_key(&target.join("."))
                        || (target.len() > 1
                            && self
                                .templates
                                .contains_key(&target[..target.len() - 1].join("."))))
                {
                    return Err(error(
                        expression.range,
                        "generic invocation requires explicit type arguments",
                    ));
                }
                self.arguments_expected(target, arguments, substitution, instance)?;
            }
            ExpressionKind::Name(path)
                if !self.local_target(path) && self.templates.contains_key(&path.join(".")) =>
            {
                let name = path.join(".");
                let args =
                    self.inferred_reference(&name, &[], expected, instance, expression.range)?;
                *path = vec![self.instantiate(&name, &args, expression.range)?];
            }
            ExpressionKind::List(values) => {
                let element = expected.and_then(|t| {
                    if let TypeRefKind::List(element) = &t.kind {
                        Some(element.as_ref())
                    } else {
                        None
                    }
                });
                for value in values {
                    self.expression_expected(value, substitution, instance, element)?;
                }
            }
            ExpressionKind::FormatString(values) => {
                for value in values {
                    self.expression(value, substitution, instance)?;
                }
            }
            ExpressionKind::RecordUpdate { base, fields } => {
                self.expression(base, substitution, instance)?;
                let base_type = if self.inference_active {
                    self.probe_type(base, None)
                } else {
                    None
                };
                for (name, value, range) in fields {
                    range.instance = instance;
                    let expected = base_type.as_ref().and_then(|t| match &t.kind {
                        TypeRefKind::Record(fields) => {
                            fields.iter().find(|f| &f.name == name).map(|f| &f.ty)
                        }
                        _ => None,
                    });
                    self.expression_expected(value, substitution, instance, expected)?;
                }
            }
            ExpressionKind::Map(entries) => {
                let (key_type, value_type) = expected.map_or((None, None), |t| {
                    if let TypeRefKind::Map(k, v) = &t.kind {
                        (Some(k.as_ref()), Some(v.as_ref()))
                    } else {
                        (None, None)
                    }
                });
                for (key, value) in entries {
                    self.expression_expected(key, substitution, instance, key_type)?;
                    self.expression_expected(value, substitution, instance, value_type)?;
                }
            }
            ExpressionKind::Await(operand) => {
                let expected = expected.map(|t| TypeRef {
                    range: t.range,
                    kind: TypeRefKind::Future(Box::new(t.clone())),
                });
                self.expression_expected(operand, substitution, instance, expected.as_ref())?;
            }
            ExpressionKind::Unary { operand, .. } => {
                self.expression_expected(operand, substitution, instance, expected)?
            }
            ExpressionKind::Binary { left, right, .. } => {
                self.expression(left, substitution, instance)?;
                self.expression(right, substitution, instance)?;
            }
            ExpressionKind::Is { value, ty } => {
                self.expression(value, substitution, instance)?;
                self.ty(ty, substitution, instance)?;
            }
            ExpressionKind::Literal(_) | ExpressionKind::Name(_) | ExpressionKind::Unit => {}
        }
        Ok(())
    }

    fn local_target(&self, path: &[String]) -> bool {
        path.first().is_some_and(|name| self.locals.contains(name))
    }

    fn require_template_visible(
        &self,
        path: &[String],
        range: SourceRange,
    ) -> Result<(), Diagnostic> {
        if self.local_target(path) {
            Err(error(
                range,
                "local values do not accept explicit type arguments",
            ))
        } else {
            Ok(())
        }
    }
}

fn spelling(ty: &TypeRef) -> String {
    match &ty.kind {
        TypeRefKind::Record(fields) => {
            let mut fields = fields
                .iter()
                .map(|f| format!("{}:{}", f.name, spelling(&f.ty)))
                .collect::<Vec<_>>();
            fields.sort();
            format!("{{{}}}", fields.join(","))
        }
        TypeRefKind::Named(path) => path.join("."),
        TypeRefKind::List(inner) => format!("List<{}>", spelling(inner)),
        TypeRefKind::Future(inner) => format!("Future<{}>", spelling(inner)),
        TypeRefKind::Map(key, value) => format!("Map<{},{}>", spelling(key), spelling(value)),
        TypeRefKind::Bytes(length) => match &length.kind {
            ExpressionKind::Literal(Literal::Int(value)) => format!("Bytes({value})"),
            ExpressionKind::Literal(Literal::Unsigned(value)) => format!("Bytes({value})"),
            _ => format!("Bytes({:?})", length.kind),
        },
        TypeRefKind::ConcreteBytes(length) => format!("Bytes({length})"),
        TypeRefKind::Function {
            supplant,
            parameters,
            result,
        } => format!(
            "{}fn({})->{}",
            if *supplant { "supplant " } else { "" },
            parameters
                .iter()
                .map(spelling)
                .collect::<Vec<_>>()
                .join(","),
            spelling(result)
        ),
        TypeRefKind::Generic { .. } => unreachable!("type arguments expanded first"),
    }
}

fn error(range: SourceRange, message: &str) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::TypeMismatch,
        range,
        message,
        "use explicit concrete type arguments within the supported generic contract",
    )
}

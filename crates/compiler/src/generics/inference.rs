//! Local constraints select concrete instances. Expression types come from
//! sema's ordinary checker rather than a second set of expression typing rules.
use super::*;
use crate::ast_walk::{self, Visitor};

impl Expander<'_> {
    fn weak_argument(&self, value: &Expression) -> bool {
        if numeric_only(value) {
            return true;
        }
        if let ExpressionKind::List(elements) = &value.kind {
            return elements.iter().all(|element| self.weak_argument(element));
        }
        let ExpressionKind::Call { target, arguments } = &value.kind else {
            return false;
        };
        let name = target.join(".");
        let template = self.templates.get(&name).or_else(|| {
            target
                .len()
                .checked_sub(1)
                .and_then(|n| self.templates.get(&target[..n].join(".")))
        });
        let Some(template) = template else {
            return false;
        };
        if let Declaration::Function(f) = template {
            let variables = f.type_parameters.iter().map(|p| p.name.clone()).collect();
            if !contains_variable(&f.result, &variables) {
                return false;
            }
        }
        match arguments {
            Arguments::Positional(values) => values.iter().all(|v| self.weak_argument(v)),
            Arguments::Named(values) => values.iter().all(|(_, v, _)| self.weak_argument(v)),
        }
    }
    pub(super) fn declaration_named(&self, name: &str) -> Option<&Declaration> {
        self.output
            .iter()
            .find(|d| d.name() == name)
            .or_else(|| self.known.get(name))
    }

    fn argument_types(&self, target: &[String]) -> Vec<(String, TypeRef)> {
        if target.len() == 1
            && self.local_target(target)
            && let Some(TypeRef {
                kind: TypeRefKind::Function { parameters, .. },
                ..
            }) = self.type_locals.get(&target[0])
        {
            return parameters
                .iter()
                .map(|ty| (String::new(), ty.clone()))
                .collect();
        }
        let name = target.join(".");
        match self.declaration_named(&name) {
            Some(Declaration::Function(f)) => {
                return f
                    .parameters
                    .iter()
                    .map(|p| (p.name.clone(), p.ty.clone()))
                    .collect();
            }
            Some(Declaration::Record(r)) => {
                return r
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.ty.clone()))
                    .collect();
            }
            Some(Declaration::Error(e)) => {
                return e
                    .payload
                    .iter()
                    .map(|p| ("payload".into(), p.ty.clone()))
                    .collect();
            }
            _ => {}
        }
        if target.len() > 1 {
            let owner = target[..target.len() - 1].join(".");
            if let Some(Declaration::Enum(e)) = self.declaration_named(&owner)
                && let Some(v) = e.variants.iter().find(|v| Some(&v.name) == target.last())
            {
                return v
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.ty.clone()))
                    .collect();
            }
        }
        Vec::new()
    }

    pub(super) fn arguments_expected(
        &mut self,
        target: &[String],
        arguments: &mut Arguments,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
    ) -> ExpansionResult<()> {
        let mut formals = self.argument_types(target);
        // Data intrinsics derive their remaining parameter types from the
        // first argument. Use the checker's shared signature rules so nested
        // empty constructors receive the same context as ordinary calls.
        if self.inference_active
            && formals.is_empty()
            && !self.local_target(target)
            && let Some(operation) = crate::core::data_operation(target)
            && let Arguments::Positional(values) = arguments
            && let Some(first) = values.first_mut()
        {
            self.expression(first, substitution, instance)?;
            if let Some(TypeRef {
                kind: TypeRefKind::Function { parameters, .. },
                ..
            }) = self.probe_type_with_data_signature(first, None, Some(operation))
            {
                formals = parameters
                    .into_iter()
                    .map(|ty| (String::new(), ty))
                    .collect();
            }
        }
        self.arguments_with_formals(arguments, &formals, substitution, instance)
    }

    pub(super) fn arguments_with_formals(
        &mut self,
        arguments: &mut Arguments,
        formals: &[(String, TypeRef)],
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
    ) -> ExpansionResult<()> {
        match arguments {
            Arguments::Positional(values) => {
                for (index, value) in values.iter_mut().enumerate() {
                    let mut expected = formals.get(index).map(|(_, ty)| ty.clone());
                    if let Some(t) = &mut expected {
                        self.ty(t, substitution, instance)?;
                    }
                    self.expression_expected(value, substitution, instance, expected.as_ref())?;
                }
            }
            Arguments::Named(values) => {
                for (label, value, _) in values {
                    let mut expected = formals
                        .iter()
                        .find(|(n, _)| n == label)
                        .map(|(_, ty)| ty.clone());
                    if let Some(t) = &mut expected {
                        self.ty(t, substitution, instance)?;
                    }
                    self.expression_expected(value, substitution, instance, expected.as_ref())?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn needs_inference(&self, body: &Block) -> bool {
        struct Calls<'a> {
            templates: &'a HashMap<String, Declaration>,
            found: bool,
        }
        impl Visitor for Calls<'_> {
            fn expression(&mut self, e: &mut Expression) {
                if let ExpressionKind::Call { target, .. } | ExpressionKind::Spawn { target, .. } =
                    &e.kind
                {
                    self.found |= self.templates.contains_key(&target.join("."))
                        || target.len() > 1
                            && self
                                .templates
                                .contains_key(&target[..target.len() - 1].join("."));
                    self.found |= target.len() > 1
                        && self.templates.keys().any(|name| {
                            crate::methods::parts(name).is_some_and(|(_, member)| {
                                Some(member) == target.last().map(String::as_str)
                            })
                        });
                }
                if let ExpressionKind::GenericCall {
                    target,
                    variant,
                    member_type_arguments,
                    ..
                } = &e.kind
                {
                    let method = variant.as_ref().map_or_else(
                        || target.join("."),
                        |member| format!("{}.{member}", target.join(".")),
                    );
                    self.found |= variant.is_none()
                        && target.len() > 1
                        && self.templates.keys().any(|name| {
                            crate::methods::parts(name).is_some_and(|(_, member)| {
                                Some(member) == target.last().map(String::as_str)
                            })
                        });
                    self.found |= variant.is_some()
                        && member_type_arguments.is_empty()
                        && self.templates.get(&method).is_some_and(|declaration| {
                            matches!(declaration, Declaration::Function(function)
                                if function.type_parameters.len() > function.owner_type_parameter_count)
                        });
                }
                if let ExpressionKind::Name(path) = &e.kind {
                    self.found |= self.templates.contains_key(&path.join("."));
                }
            }
        }
        let mut calls = Calls {
            templates: &self.templates,
            found: false,
        };
        ast_walk::block(&mut body.clone(), &mut calls);
        calls.found
    }

    pub(super) fn probe_type(
        &mut self,
        expression: &Expression,
        expected: Option<&TypeRef>,
    ) -> Option<TypeRef> {
        self.probe_type_with_data_signature(expression, expected, None)
    }

    fn probe_type_with_data_signature(
        &mut self,
        expression: &Expression,
        expected: Option<&TypeRef>,
        data_operation: Option<crate::DataOperation>,
    ) -> Option<TypeRef> {
        let mut module = self.module.clone();
        let mut declarations = self.known.clone();
        for declaration in &self.output {
            declarations.insert(declaration.name().to_owned(), declaration.clone());
        }
        module.declarations = declarations
            .into_values()
            .filter(|d| parameters(d).is_empty())
            .collect();
        // Resolve signatures without checking bodies. This also makes concrete
        // nominal instances available when a non-generic helper returns one.
        for declaration in &mut module.declarations {
            match declaration {
                Declaration::Function(f) => {
                    for p in &mut f.parameters {
                        self.ty(&mut p.ty, &HashMap::new(), 0).ok()?;
                    }
                    self.ty(&mut f.result, &HashMap::new(), 0).ok()?;
                }
                Declaration::Record(r) => {
                    for field in &mut r.fields {
                        self.ty(&mut field.ty, &HashMap::new(), 0).ok()?;
                    }
                }
                Declaration::Error(error) => {
                    if let Some(payload) = &mut error.payload {
                        self.ty(&mut payload.ty, &HashMap::new(), 0).ok()?;
                    }
                }
                Declaration::Enum(enumeration) => {
                    for field in enumeration.variants.iter_mut().flat_map(|v| &mut v.fields) {
                        self.ty(&mut field.ty, &HashMap::new(), 0).ok()?;
                    }
                }
                _ => {}
            }
        }
        for declaration in &self.output {
            if !module
                .declarations
                .iter()
                .any(|d| d.name() == declaration.name())
            {
                module.declarations.push(declaration.clone());
            }
        }
        let function = match self.known.get(&self.function_name) {
            Some(Declaration::Function(function)) => Some(function),
            _ => None,
        };
        let mut ty = crate::sema::probe_expression_type(
            &module,
            self.options,
            expression,
            expected,
            crate::sema::ProbeScope {
                function,
                locals: &self.type_locals,
                option_shadow_modules: self.option_shadow_modules,
            },
            data_operation,
        )?;
        // The checker can synthesize prelude Option instances while probing.
        // Keep those instances available to subsequent probes and constraints.
        self.ty(&mut ty, &HashMap::new(), expression.range.instance)
            .ok()?;
        Some(ty)
    }

    fn abstract_type(&self, ty: &TypeRef) -> TypeRef {
        let mut ty = ty.clone();
        match &mut ty.kind {
            TypeRefKind::Named(path) => {
                if let Some((name, args)) = self.instance_types.get(&path.join(".")) {
                    ty.kind = TypeRefKind::Generic {
                        path: vec![name.clone()],
                        arguments: args.iter().map(|t| self.abstract_type(t)).collect(),
                    };
                }
            }
            TypeRefKind::List(t) | TypeRefKind::Future(t) => **t = self.abstract_type(t),
            TypeRefKind::Map(k, v) => {
                **k = self.abstract_type(k);
                **v = self.abstract_type(v);
            }
            TypeRefKind::Record(fields) => {
                for f in fields {
                    f.ty = self.abstract_type(&f.ty);
                }
            }
            TypeRefKind::Function {
                parameters,
                result,
                error_contract,
                ..
            } => {
                for p in parameters {
                    *p = self.abstract_type(p);
                }
                **result = self.abstract_type(result);
                if let Some(contract) = error_contract {
                    for ty in &mut contract.types {
                        *ty = self.abstract_type(ty);
                    }
                }
            }
            _ => {}
        }
        ty
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn inferred_arguments(
        &mut self,
        name: &str,
        variant: Option<&str>,
        arguments: &mut Arguments,
        expected: Option<&TypeRef>,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
        range: SourceRange,
        fixed: &[TypeRef],
    ) -> ExpansionResult<Vec<TypeRef>> {
        let template = self.templates.get(name).expect("known template").clone();
        let variables: HashSet<_> = parameters(&template)
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let generic_type = TypeRef {
            range,
            kind: TypeRefKind::Generic {
                path: vec![name.to_owned()],
                arguments: parameters(&template)
                    .iter()
                    .map(|p| TypeRef {
                        range: p.range,
                        kind: TypeRefKind::Named(vec![p.name.clone()]),
                    })
                    .collect(),
            },
        };
        let (formals, result) = match &template {
            Declaration::Function(f) => (
                f.parameters
                    .iter()
                    .map(|p| (p.name.clone(), p.ty.clone()))
                    .collect::<Vec<_>>(),
                if f.is_async {
                    TypeRef {
                        range,
                        kind: TypeRefKind::Future(Box::new(f.result.clone())),
                    }
                } else {
                    f.result.clone()
                },
            ),
            Declaration::Record(r) => (
                r.fields
                    .iter()
                    .map(|f| (f.name.clone(), f.ty.clone()))
                    .collect(),
                generic_type,
            ),
            Declaration::Enum(e) => {
                let fields = &e
                    .variants
                    .iter()
                    .find(|v| Some(v.name.as_str()) == variant)
                    .ok_or_else(|| error(range, "generic enum call requires a declared variant"))?
                    .fields;
                (
                    fields
                        .iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
                        .collect(),
                    generic_type,
                )
            }
            Declaration::Error(e) => (
                e.payload
                    .iter()
                    .map(|p| ("payload".into(), p.ty.clone()))
                    .collect(),
                generic_type,
            ),
            _ => {
                return Err(error(
                    range,
                    "this declaration does not support inferred invocation",
                ));
            }
        };
        let mut constraints: HashMap<_, _> = parameters(&template)
            .iter()
            .zip(fixed)
            .map(|(p, t)| (p.name.clone(), self.abstract_type(t)))
            .collect();
        if let Some(expected) = expected {
            constrain(
                &result,
                &self.abstract_type(expected),
                &variables,
                &mut constraints,
                range,
            )?;
        }
        // Typed arguments and callback signatures precede unconstrained numeric
        // literals, so the default Int never overrides a concrete CInt8 hint.
        for weak in [false, true] {
            match arguments {
                Arguments::Positional(values) => {
                    for (index, value) in values.iter_mut().enumerate() {
                        let Some((_, formal)) = formals.get(index) else {
                            continue;
                        };
                        if weak != self.weak_argument(value) {
                            continue;
                        }
                        self.argument_constraint(
                            value,
                            formal,
                            &variables,
                            &mut constraints,
                            substitution,
                            instance,
                            range,
                            weak,
                        )?;
                    }
                }
                Arguments::Named(values) => {
                    for (label, value, _) in values {
                        let Some((_, formal)) = formals.iter().find(|(name, _)| name == label)
                        else {
                            continue;
                        };
                        if weak != self.weak_argument(value) {
                            continue;
                        }
                        self.argument_constraint(
                            value,
                            formal,
                            &variables,
                            &mut constraints,
                            substitution,
                            instance,
                            range,
                            weak,
                        )?;
                    }
                }
            }
        }
        // A nested empty constructor may gain its expected type from a later
        // typed argument. Revisit only compile-time constraints, never execution.
        match arguments {
            Arguments::Positional(values) => {
                for (index, value) in values.iter_mut().enumerate() {
                    if let Some((_, formal)) = formals.get(index) {
                        self.argument_constraint(
                            value,
                            formal,
                            &variables,
                            &mut constraints,
                            substitution,
                            instance,
                            range,
                            true,
                        )?;
                    }
                }
            }
            Arguments::Named(values) => {
                for (label, value, _) in values {
                    if let Some((_, formal)) = formals.iter().find(|(name, _)| name == label) {
                        self.argument_constraint(
                            value,
                            formal,
                            &variables,
                            &mut constraints,
                            substitution,
                            instance,
                            range,
                            true,
                        )?;
                    }
                }
            }
        }
        parameters(&template)
            .iter()
            .map(|parameter| {
                let mut ty = constraints.get(&parameter.name).cloned().ok_or_else(|| {
                    error(
                        range,
                        &format!(
                            "cannot infer type parameter `{}` of `{name}`; add an expected type or explicit type arguments",
                            parameter.name
                        ),
                    )
                })?;
                self.ty(&mut ty, &HashMap::new(), instance)?;
                self.canonical_argument(&mut ty)?;
                Ok(ty)
            })
            .collect()
    }

    pub(super) fn inferred_reference(
        &mut self,
        name: &str,
        fixed: &[TypeRef],
        expected: Option<&TypeRef>,
        instance: u32,
        range: SourceRange,
    ) -> ExpansionResult<Vec<TypeRef>> {
        let Some(Declaration::Function(function)) = self.templates.get(name).cloned() else {
            return Err(error(range, "generic reference must name a function"));
        };
        let Some(expected) = expected else {
            return Err(error(
                range,
                "generic function reference requires concrete type arguments or an expected function type",
            ));
        };
        let variables = function
            .type_parameters
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let result = if function.is_async {
            TypeRef {
                range,
                kind: TypeRefKind::Future(Box::new(function.result.clone())),
            }
        } else {
            function.result.clone()
        };
        let formal = TypeRef {
            range,
            kind: TypeRefKind::Function {
                supplant: false,
                parameters: function.parameters.iter().map(|p| p.ty.clone()).collect(),
                result: Box::new(result),
                error_contract: None,
            },
        };
        let mut constraints: HashMap<_, _> = function
            .type_parameters
            .iter()
            .zip(fixed)
            .map(|(p, t)| (p.name.clone(), self.abstract_type(t)))
            .collect();
        constrain(
            &formal,
            &self.abstract_type(expected),
            &variables,
            &mut constraints,
            range,
        )?;
        function
            .type_parameters
            .iter()
            .map(|p| {
                let mut ty = constraints.get(&p.name).cloned().ok_or_else(|| {
                    error(
                        range,
                        &format!(
                            "cannot infer type parameter `{}` for function reference",
                            p.name
                        ),
                    )
                })?;
                self.ty(&mut ty, &HashMap::new(), instance)?;
                self.canonical_argument(&mut ty)?;
                Ok(ty)
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn argument_constraint(
        &mut self,
        value: &mut Expression,
        formal: &TypeRef,
        variables: &HashSet<String>,
        constraints: &mut HashMap<String, TypeRef>,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
        range: SourceRange,
        required: bool,
    ) -> ExpansionResult<()> {
        let mut expected = substitute(formal, constraints);
        let expected = if contains_variable(&expected, variables) {
            None
        } else {
            self.ty(&mut expected, &HashMap::new(), instance)?;
            Some(expected)
        };
        if let Err(problem) =
            self.expression_expected(value, substitution, instance, expected.as_ref())
        {
            if required {
                return Err(problem);
            }
            return Ok(());
        }
        if let Some(actual) = self.probe_type(value, expected.as_ref()) {
            constrain(
                formal,
                &self.abstract_type(&actual),
                variables,
                constraints,
                range,
            )?;
        }
        Ok(())
    }
}

fn numeric_only(value: &Expression) -> bool {
    match &value.kind {
        ExpressionKind::Literal(
            Literal::Int(_)
            | Literal::Unsigned(_)
            | Literal::DecimalInteger(_)
            | Literal::Float { .. },
        ) => true,
        ExpressionKind::Unary {
            operator: UnaryOperator::Negate,
            operand,
        } => numeric_only(operand),
        ExpressionKind::Binary { left, right, .. } => numeric_only(left) && numeric_only(right),
        _ => false,
    }
}

fn contains_variable(ty: &TypeRef, variables: &HashSet<String>) -> bool {
    match &ty.kind {
        TypeRefKind::Named(path) => path.len() == 1 && variables.contains(&path[0]),
        TypeRefKind::Generic { arguments, .. } => {
            arguments.iter().any(|t| contains_variable(t, variables))
        }
        TypeRefKind::List(t) | TypeRefKind::Future(t) => contains_variable(t, variables),
        TypeRefKind::Map(k, v) => {
            contains_variable(k, variables) || contains_variable(v, variables)
        }
        TypeRefKind::Record(fields) => fields.iter().any(|f| contains_variable(&f.ty, variables)),
        TypeRefKind::Function {
            parameters, result, ..
        } => {
            parameters.iter().any(|t| contains_variable(t, variables))
                || contains_variable(result, variables)
        }
        _ => false,
    }
}

pub(super) fn substitute(ty: &TypeRef, bindings: &HashMap<String, TypeRef>) -> TypeRef {
    let mut result = ty.clone();
    match &mut result.kind {
        TypeRefKind::Named(path) if path.len() == 1 => {
            if let Some(ty) = bindings.get(&path[0]) {
                result.kind = ty.kind.clone();
            }
        }
        TypeRefKind::Generic { arguments, .. } => {
            for t in arguments {
                *t = substitute(t, bindings);
            }
        }
        TypeRefKind::List(t) | TypeRefKind::Future(t) => **t = substitute(t, bindings),
        TypeRefKind::Map(k, v) => {
            **k = substitute(k, bindings);
            **v = substitute(v, bindings);
        }
        TypeRefKind::Record(fields) => {
            for f in fields {
                f.ty = substitute(&f.ty, bindings);
            }
        }
        TypeRefKind::Function {
            parameters,
            result,
            error_contract,
            ..
        } => {
            for t in parameters {
                *t = substitute(t, bindings);
            }
            **result = substitute(result, bindings);
            if let Some(contract) = error_contract {
                for ty in &mut contract.types {
                    *ty = substitute(ty, bindings);
                }
            }
        }
        _ => {}
    }
    result
}

fn constrain(
    formal: &TypeRef,
    actual: &TypeRef,
    variables: &HashSet<String>,
    bindings: &mut HashMap<String, TypeRef>,
    range: SourceRange,
) -> ExpansionResult<()> {
    if let TypeRefKind::Named(path) = &formal.kind
        && path.len() == 1
        && variables.contains(&path[0])
    {
        if let Some(previous) = bindings.get(&path[0]) {
            if key(previous) != key(actual) {
                return Err(error(
                    actual.range,
                    &format!(
                        "conflicting types for `{}`: {} and {}",
                        path[0],
                        key(previous),
                        key(actual)
                    ),
                )
                .with_secondary(previous.range, "previous type constraint is here")
                .into());
            }
        } else {
            bindings.insert(path[0].clone(), actual.clone());
        }
        return Ok(());
    }
    match (&formal.kind, &actual.kind) {
        (
            TypeRefKind::Generic {
                path: a,
                arguments: aa,
            },
            TypeRefKind::Generic {
                path: b,
                arguments: ba,
            },
        ) if a == b && aa.len() == ba.len() => {
            for (a, b) in aa.iter().zip(ba) {
                constrain(a, b, variables, bindings, range)?;
            }
        }
        (TypeRefKind::List(a), TypeRefKind::List(b))
        | (TypeRefKind::Future(a), TypeRefKind::Future(b)) => {
            constrain(a, b, variables, bindings, range)?
        }
        (TypeRefKind::Map(a, b), TypeRefKind::Map(c, d)) => {
            constrain(a, c, variables, bindings, range)?;
            constrain(b, d, variables, bindings, range)?;
        }
        (TypeRefKind::Record(a), TypeRefKind::Record(b)) if a.len() == b.len() => {
            for field in a {
                let Some(other) = b.iter().find(|f| f.name == field.name) else {
                    return Err(error(
                        range,
                        "record shape does not match generic constraint",
                    ));
                };
                constrain(&field.ty, &other.ty, variables, bindings, range)?;
            }
        }
        (
            TypeRefKind::Function {
                supplant: a,
                parameters: ap,
                result: ar,
                ..
            },
            TypeRefKind::Function {
                supplant: b,
                parameters: bp,
                result: br,
                ..
            },
        ) if a == b && ap.len() == bp.len() => {
            for (a, b) in ap.iter().zip(bp) {
                constrain(a, b, variables, bindings, range)?;
            }
            constrain(ar, br, variables, bindings, range)?;
        }
        _ if !contains_variable(formal, variables) => {} // final checker owns concrete compatibility
        _ => {
            return Err(error(
                range,
                &format!("cannot match {} with {}", key(formal), key(actual)),
            ));
        }
    }
    Ok(())
}

fn key(ty: &TypeRef) -> String {
    if let TypeRefKind::Generic { path, arguments } = &ty.kind {
        format!(
            "{}<{}>",
            path.join("."),
            arguments.iter().map(key).collect::<Vec<_>>().join(",")
        )
    } else {
        match &ty.kind {
            TypeRefKind::List(t) => format!("List<{}>", key(t)),
            TypeRefKind::Future(t) => format!("Future<{}>", key(t)),
            TypeRefKind::Map(k, v) => format!("Map<{},{}>", key(k), key(v)),
            TypeRefKind::Record(fields) => {
                let mut f = fields
                    .iter()
                    .map(|f| format!("{}:{}", f.name, key(&f.ty)))
                    .collect::<Vec<_>>();
                f.sort();
                format!("{{{}}}", f.join(","))
            }
            TypeRefKind::Function {
                supplant,
                parameters,
                result,
                error_contract,
            } => format!(
                "{}fn({}){}->{}",
                supplant,
                parameters.iter().map(key).collect::<Vec<_>>().join(","),
                error_contract
                    .as_ref()
                    .map_or_else(String::new, |contract| contract.spelling(key)),
                key(result)
            ),
            _ => spelling(ty),
        }
    }
}

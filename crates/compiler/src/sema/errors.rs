//! Dedicated Error identities and payload contracts, independent of class semantics.
use super::*;
use crate::ast::{CatchClause, CatchPattern, ErrorDecl, FieldDecl, RecordDecl};

pub(super) const SOURCE_LOCATION: &str = "$rinzu_SourceLocation";

impl Checker<'_> {
    pub(super) fn check_error_test(
        &mut self,
        value: &Expression,
        target: &TypeRef,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        let actual = self.check_expression(value, None, context)?;
        let target = self.resolve_type(target)?;
        if !self.is_error_type(&actual) || target == Type::Error || !self.is_error_type(&target) {
            self.type_error(
                range,
                "is requires an Error value and a concrete dedicated Error type",
            );
            return None;
        }
        self.error_tests.insert(range, target);
        Some(Type::Bool)
    }

    pub(super) fn error_condition_refinements(
        &self,
        expression: &Expression,
        truth: bool,
        context: &FunctionContext<'_>,
    ) -> HashMap<String, Type> {
        match &expression.kind {
            ExpressionKind::Is { value, .. } if truth => {
                if let ExpressionKind::Name(path) = &value.kind
                    && let [name] = path.as_slice()
                    && context
                        .locals
                        .get(name)
                        .is_some_and(|local| !local.mutable && local.ty == Type::Error)
                    && let Some(target) = self.error_tests.get(&expression.range)
                {
                    HashMap::from([(name.clone(), target.clone())])
                } else {
                    HashMap::new()
                }
            }
            ExpressionKind::Unary {
                operator: UnaryOperator::Not,
                operand,
            } => self.error_condition_refinements(operand, !truth, context),
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } if (*operator == BinaryOperator::And && truth)
                || (*operator == BinaryOperator::Or && !truth) =>
            {
                let mut result = self.error_condition_refinements(left, truth, context);
                result.extend(self.error_condition_refinements(right, truth, context));
                result
            }
            _ => HashMap::new(),
        }
    }

    pub(super) fn check_error_handlers(
        &mut self,
        body: &Block,
        handlers: &[CatchClause],
        cleanup: Option<&Block>,
        context: &mut FunctionContext<'_>,
        all: bool,
    ) {
        let previous_all = context.all_depth;
        context.all_depth = if all { previous_all + 1 } else { 0 };
        self.check_block(body, context);
        context.all_depth = previous_all;
        let mut caught_all = false;
        for handler in handlers {
            if caught_all {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidCatchPattern,
                    handler.range,
                    "catch is unreachable after a whole-list binding",
                    "remove this catch or put the more specific pattern first",
                ));
            }
            caught_all |= matches!(handler.pattern, CatchPattern::Binding(_));
            let names_before = context.declared_names.clone();
            let mut bindings = Vec::new();
            match &handler.pattern {
                CatchPattern::Binding(binding) => {
                    bindings.push((binding, Type::List(Box::new(Type::Error))))
                }
                CatchPattern::List { elements, rest, .. } => {
                    for element in elements {
                        let Some(ty) = self.resolve_type(&element.ty) else {
                            continue;
                        };
                        if !self.is_error_type(&ty) {
                            self.type_error(
                                element.ty.range,
                                "catch elements must name Error types",
                            );
                        }
                        self.error_patterns.insert(element.range, ty.clone());
                        if let Some(binding) = &element.binding {
                            bindings.push((binding, ty));
                        }
                    }
                    if let Some(binding) = rest {
                        bindings.push((binding, Type::List(Box::new(Type::Error))));
                    }
                }
            }
            let mut inserted = Vec::new();
            for (binding, ty) in bindings {
                if let Some(previous) = context.declared_names.get(&binding.name) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::DuplicateName,
                            binding.range,
                            format!("catch binding `{}` shadows an existing name", binding.name),
                            "use a unique binding within this handler",
                        )
                        .with_secondary(*previous, "first declaration is here"),
                    );
                } else {
                    context
                        .declared_names
                        .insert(binding.name.clone(), binding.range);
                    context.locals.insert(
                        binding.name.clone(),
                        Local {
                            callback_contract: None,
                            ty,
                            mutable: false,
                            constant: None,
                            targets: Vec::new(),
                        },
                    );
                    inserted.push(binding.name.clone());
                }
            }
            self.check_block(&handler.body, context);
            for name in inserted {
                context.locals.remove(&name);
            }
            context.declared_names = names_before;
        }
        if let Some(cleanup) = cleanup {
            let previous = (context.in_finally, context.in_task_cleanup);
            context.in_finally = true;
            // Async cleanup uses the same explicit-await rule for try and try all.
            // This does not adopt, observe or join an unawaited task.
            context.in_task_cleanup = context.function.is_async;
            self.check_block(cleanup, context);
            (context.in_finally, context.in_task_cleanup) = previous;
        }
    }

    pub(super) fn is_error_type(&self, ty: &Type) -> bool {
        *ty == Type::Error
            || matches!(ty, Type::Named(name) if matches!(self.declarations.get(name), Some(Declaration::Error(_))))
    }

    pub(super) fn source_location_type(&mut self, range: SourceRange) -> Type {
        if !self.declarations.contains_key(SOURCE_LOCATION) {
            let fields = [
                ("file", Type::String),
                ("line", Type::Int),
                ("column", Type::Int),
            ]
            .into_iter()
            .map(|(name, ty)| FieldDecl {
                name: name.to_owned(),
                is_const: true,
                is_private: false,
                ty: self.generated_type_ref(&ty, range),
                range,
            })
            .collect();
            self.declarations.insert(
                SOURCE_LOCATION.to_owned(),
                Declaration::Record(RecordDecl {
                    interfaces: Vec::new(),
                    mutable: false,
                    name: SOURCE_LOCATION.to_owned(),
                    type_parameters: Vec::new(),
                    fields,
                    range,
                }),
            );
        }
        Type::Named(SOURCE_LOCATION.to_owned())
    }

    pub(super) fn prepare_error_types(&mut self) {
        let mut errors: Vec<_> = self
            .declarations
            .values()
            .filter_map(|declaration| match declaration {
                Declaration::Error(error) => Some(error.clone()),
                _ => None,
            })
            .collect();
        errors.sort_by(|left, right| left.name.cmp(&right.name));
        for error in errors {
            let payload = if let Some(payload) = &error.payload {
                let Some(ty) = self.resolve_type(&payload.ty) else {
                    continue;
                };
                if self.contains_future(&ty, &mut HashSet::new()) {
                    self.type_error(
                        payload.range,
                        "Error payload cannot contain Future, directly or through another value",
                    );
                }
                if self.contains_call_only_type(&ty, &mut HashSet::new()) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::CallOnlyEscape,
                        payload.range,
                        "Error payload cannot retain a borrowed or call-only value",
                        "store an owned value whose lifetime is valid for the Error",
                    ));
                }
                if payload.optional {
                    self.ensure_option(&ty, payload.range, &error.name)
                } else {
                    Some(ty)
                }
            } else {
                None
            };
            let source_location = self.source_location_type(error.range);
            let Some(source) = self.ensure_option(&source_location, error.range, &error.name)
            else {
                continue;
            };
            let Some(cause) = self.ensure_option(&Type::Error, error.range, &error.name) else {
                continue;
            };
            let mut fields = vec![
                ("name".into(), Type::String),
                ("message".into(), Type::String),
                ("source".into(), source),
                ("cause".into(), cause),
            ];
            if let Some(payload) = payload {
                fields.push(("payload".into(), payload));
            }
            self.error_schemas.insert(
                error.name.clone(),
                SourceTypeSchema {
                    private_fields: Vec::new(),
                    kind: "error".into(),
                    const_fields: (0..fields.len() as u32).collect(),
                    fields,
                    variants: Vec::new(),
                },
            );
        }
    }

    pub(super) fn error_field_type(
        &mut self,
        ty: &Type,
        field: &str,
        range: SourceRange,
        scope: &str,
    ) -> Option<Type> {
        if let Type::Named(name) = ty {
            if let Some(ty) = self
                .error_schemas
                .get(name)
                .and_then(|schema| schema.fields.iter().find(|(name, _)| name == field))
                .map(|(_, ty)| ty.clone())
            {
                return Some(ty);
            }
        } else {
            match field {
                "name" | "message" => return Some(Type::String),
                "source" => {
                    let source_location = self.source_location_type(range);
                    return self.ensure_option(&source_location, range, scope);
                }
                "cause" => return self.ensure_option(&Type::Error, range, scope),
                _ => {}
            }
        }
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnknownName,
            range,
            format!("Error type `{ty}` has no field `{field}`"),
            "use name/message/source/cause or the payload of a concrete Error type",
        ));
        None
    }

    pub(super) fn check_error_constructor(
        &mut self,
        error: &ErrorDecl,
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        let Arguments::Named(arguments) = arguments else {
            self.type_error(
                range,
                "Error construction requires named arguments including message: String",
            );
            return None;
        };
        let mut seen = HashMap::new();
        for (name, expression, argument_range) in arguments {
            if let Some(previous) = seen.insert(name.as_str(), *argument_range) {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::DuplicateName,
                        *argument_range,
                        format!("Error constructor argument `{name}` appears more than once"),
                        "provide each argument at most once",
                    )
                    .with_secondary(previous, "first argument is here"),
                );
                continue;
            }
            let expected = match name.as_str() {
                "message" => Some(Type::String),
                "cause" => Some(Type::Error),
                "payload" => error
                    .payload
                    .as_ref()
                    .and_then(|payload| self.resolve_type(&payload.ty)),
                _ => None,
            };
            if let Some(expected) = expected {
                self.expect_expression(expression, &expected, context);
            } else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::UnknownName,
                    *argument_range,
                    format!("Error constructor has no argument `{name}`"),
                    "provide message, an optional cause, and the payload declared by this Error",
                ));
            }
        }
        if !seen.contains_key("message") {
            self.type_error(
                range,
                "Error constructor is missing the required message: String",
            );
        }
        if error
            .payload
            .as_ref()
            .is_some_and(|payload| !payload.optional)
            && !seen.contains_key("payload")
        {
            self.type_error(range, "Error constructor is missing its required payload");
        }
        Some(Type::Named(error.name.clone()))
    }
}

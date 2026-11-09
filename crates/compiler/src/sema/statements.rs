//! Statement and block checking, including local scope and control flow.
use super::*;
use crate::ast::{Statement, StatementKind};

impl Checker<'_> {
    pub(super) fn check_block(&mut self, block: &Block, context: &mut FunctionContext<'_>) {
        context.scope_stack.push(block.range);
        let active_before: HashSet<_> = context.locals.keys().cloned().collect();
        for statement in &block.statements {
            self.check_statement(statement, context);
        }
        context
            .locals
            .retain(|name, _| active_before.contains(name));
        context.scope_stack.pop();
    }

    fn check_statement(&mut self, statement: &Statement, context: &mut FunctionContext<'_>) {
        match &statement.kind {
            StatementKind::Throw(value) => {
                self.expect_expression(value, &Type::Error, context);
                context.facts.may_fail = true;
            }
            StatementKind::Rethrow { elements, rest } => {
                self.check_rethrow(elements, rest.as_ref(), statement.range, context);
                context.facts.may_fail = true;
            }
            StatementKind::TryErrors {
                all,
                body,
                handlers,
                cleanup,
            } => {
                self.check_error_handlers(body, handlers, cleanup.as_ref(), context, *all);
            }
            StatementKind::Block(block) => self.check_block(block, context),
            StatementKind::Local {
                mutable,
                name,
                annotation,
                initializer,
            } => {
                let values = self.lexical_constants(context);
                let expected = annotation
                    .as_ref()
                    .and_then(|value| self.resolve_type_with_constants(value, &values));
                let initializer_type = self.check_expression_with_bound(
                    initializer,
                    expected.as_ref(),
                    annotation
                        .as_ref()
                        .map(|ty| ExpectedErrorBound::Annotation(ty.range)),
                    context,
                );
                let ty = match (expected, initializer_type) {
                    (Some(expected), Some(actual)) => {
                        self.require_same(&expected, &actual, initializer.range);
                        expected
                    }
                    (None, Some(actual)) => actual,
                    (Some(expected), None) => expected,
                    (None, None) => return,
                };
                if self.contains_call_only_type(&ty, &mut HashSet::new()) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::CallOnlyEscape,
                        initializer.range,
                        "CallOnly value cannot be stored in a local binding",
                        "read the view or pass it directly to an approved synchronous native call",
                    ));
                }
                if context.owner_sync && matches!(ty, Type::Future(_)) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidAwait,
                        initializer.range,
                        "OwnerSync handler cannot create or store a Future",
                        "move asynchronous work outside the synchronous callback",
                    ));
                }
                let constant = if !*mutable && ty.is_integer() {
                    crate::constants::evaluate_integer_expression(initializer, ty.scalar(), &values)
                        .ok()
                } else {
                    None
                };
                if let Some(previous) = context.declared_names.get(name).copied() {
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::DuplicateName,
                            statement.range,
                            format!("local `{name}` shadows an existing name"),
                            "choose a name not used elsewhere in this function",
                        )
                        .with_secondary(previous, "existing declaration is here"),
                    );
                } else {
                    context.declared_names.insert(name.clone(), statement.range);
                    context.locals.insert(
                        name.clone(),
                        Local {
                            ty: ty.clone(),
                            mutable: *mutable,
                            constant,
                            callback_contract: callback_type_contract(&ty, annotation.as_ref()),
                            targets: if *mutable {
                                if let Type::FunctionReference(expected) = &ty {
                                    self.compatible_reference_targets(expected)
                                } else {
                                    Vec::new()
                                }
                            } else {
                                self.reference_targets
                                    .get(&initializer.range)
                                    .cloned()
                                    .unwrap_or_default()
                            },
                        },
                    );
                }
            }
            StatementKind::FieldAssign { base, field, value } => {
                let Some(Type::Named(identity)) = self.check_expression(base, None, context) else {
                    self.type_error(base.range, "field assignment requires a class receiver");
                    return;
                };
                let Some(Declaration::Record(record)) = self.declarations.get(&identity).cloned()
                else {
                    self.type_error(base.range, "field assignment requires a class receiver");
                    return;
                };
                if !record.mutable {
                    self.type_error(
                        base.range,
                        "immutable record fields cannot be assigned; create a new record value",
                    );
                    return;
                }
                let Some(declaration) = record.fields.iter().find(|f| &f.name == field) else {
                    self.type_error(statement.range, format!("unknown class field `{field}`"));
                    return;
                };
                if declaration.is_private && !self.owner_access(&identity, context) {
                    self.private_field_error(field, statement.range);
                    return;
                }
                if declaration.is_const {
                    self.type_error(
                        statement.range,
                        format!("const field `{field}` cannot be assigned after construction"),
                    );
                    return;
                }
                let Some(expected) = self.resolve_type(&declaration.ty) else {
                    return;
                };
                if let Some(actual) = self.check_expression(value, Some(&expected), context) {
                    self.require_same(&expected, &actual, value.range);
                    if self.contains_call_only_type(&actual, &mut HashSet::new()) {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::CallOnlyEscape,
                            value.range,
                            "borrowed value cannot escape into state",
                            "store a copied owned value",
                        ));
                    }
                }
            }
            StatementKind::Assign { name, value } => {
                let Some(local) = context.locals.get(name).cloned() else {
                    self.unknown_name(name, statement.range);
                    return;
                };
                if !local.mutable {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidAssignment,
                        statement.range,
                        format!("`{name}` is immutable"),
                        "declare it with `var` if reassignment is required",
                    ));
                }
                if let Some(actual) = self.check_expression_with_bound(
                    value,
                    Some(&local.ty),
                    local.callback_contract.map(ExpectedErrorBound::Annotation),
                    context,
                ) {
                    if self.contains_call_only_type(&actual, &mut HashSet::new()) {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::CallOnlyEscape,
                            value.range,
                            "CallOnly value cannot be assigned to local state",
                            "use the value only during the current synchronous callback",
                        ));
                    }
                    self.require_same(&local.ty, &actual, value.range);
                    if let Some(targets) = self.reference_targets.get(&value.range) {
                        let local = context.locals.get_mut(name).expect("checked local");
                        local.targets.extend(targets.iter().cloned());
                        local.targets.sort();
                        local.targets.dedup();
                    }
                }
            }
            StatementKind::Expression(expression) => {
                if let Some(ty) = self.check_expression(expression, Some(&Type::Unit), context) {
                    // Future statement values are checked by the task
                    // responsibility analysis, including their success value.
                    if !matches!(ty, Type::Future(_)) && ty != Type::Unit && ty != Type::Never {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::TypeMismatch,
                            expression.range,
                            format!("expression statement has type `{ty}`, not `Unit`"),
                            "store, return, or otherwise use the value",
                        ));
                    }
                }
            }
            StatementKind::Return(value) => {
                if context.in_finally {
                    self.invalid_finally(statement.range, "`return` is not allowed in `finally`");
                }
                let result_type = context.result.clone();
                let actual = if let Some(expression) = value {
                    let Some(actual) = self.check_expression_with_bound(
                        expression,
                        Some(&result_type),
                        Some(ExpectedErrorBound::Result(context.function.result.range)),
                        context,
                    ) else {
                        return;
                    };
                    actual
                } else {
                    Type::Unit
                };
                if self.contains_call_only_type(&actual, &mut HashSet::new()) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::CallOnlyEscape,
                        statement.range,
                        "CallOnly value cannot be returned",
                        "consume the view synchronously inside the OwnerSync handler",
                    ));
                }
                self.require_same(&result_type, &actual, statement.range);
            }
            StatementKind::Break | StatementKind::Continue => {
                if context.in_finally {
                    self.invalid_finally(
                        statement.range,
                        "loop control is not allowed in `finally`",
                    );
                }
                if context.loop_depth == 0 {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidControlFlow,
                        statement.range,
                        "loop control is only valid inside `while` or `for`",
                        "move this statement into a loop body or remove it",
                    ));
                }
            }
            StatementKind::If {
                condition,
                then_block,
                else_branch,
            } => {
                self.expect_expression(condition, &Type::Bool, context);
                let previous = context.narrowed_errors.clone();
                context
                    .narrowed_errors
                    .extend(self.error_condition_refinements(condition, true, context));
                self.check_block(then_block, context);
                context.narrowed_errors = previous.clone();
                if let Some(branch) = else_branch {
                    context
                        .narrowed_errors
                        .extend(self.error_condition_refinements(condition, false, context));
                    self.check_statement(branch, context);
                }
                context.narrowed_errors = previous;
            }
            StatementKind::While { condition, body } => {
                self.expect_expression(condition, &Type::Bool, context);
                context.loop_depth += 1;
                let previous = context.narrowed_errors.clone();
                context
                    .narrowed_errors
                    .extend(self.error_condition_refinements(condition, true, context));
                self.check_block(body, context);
                context.narrowed_errors = previous;
                context.loop_depth -= 1;
            }
            StatementKind::For {
                binding,
                binding_range,
                iterable,
                body,
            } => {
                let Some(ty) = self.check_expression(iterable, None, context) else {
                    return;
                };
                let Type::List(element) = ty else {
                    self.type_error(iterable.range, "`for` requires a List value");
                    return;
                };
                let declared_names = context.declared_names.clone();
                if binding != "_" {
                    if let Some(previous) = context.declared_names.get(binding).copied() {
                        self.diagnostics.push(
                            Diagnostic::error(
                                DiagnosticCode::DuplicateName,
                                *binding_range,
                                format!("local `{binding}` shadows an existing name"),
                                "choose an unused loop binding name",
                            )
                            .with_secondary(previous, "existing declaration is here"),
                        );
                        return;
                    }
                    context
                        .declared_names
                        .insert(binding.clone(), *binding_range);
                    let targets = if let Type::FunctionReference(signature) = element.as_ref() {
                        self.compatible_reference_targets(signature)
                    } else {
                        Vec::new()
                    };
                    context.locals.insert(
                        binding.clone(),
                        Local {
                            ty: *element,
                            mutable: false,
                            constant: None,
                            targets,
                            callback_contract: None,
                        },
                    );
                }
                context.loop_depth += 1;
                self.check_block(body, context);
                context.loop_depth -= 1;
                context.locals.remove(binding);
                context.declared_names = declared_names;
            }
            StatementKind::Switch { value, cases } => {
                let value_type = self.check_expression(value, None, context);
                let Some(Type::Named(enum_name)) = value_type else {
                    if value_type.is_some() {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::TypeMismatch,
                            value.range,
                            "`switch` requires a closed enum value",
                            "switch on a declared enum value",
                        ));
                    }
                    return;
                };
                self.check_switch(&enum_name, value.range, cases, context);
            }
            StatementKind::TryCatch {
                body,
                binding,
                binding_range,
                handler,
                cleanup,
            } => {
                let handler = crate::ast::CatchClause::whole(binding, *binding_range, handler);
                self.check_error_handlers(body, &[handler], cleanup.as_ref(), context, false);
            }
            StatementKind::TaskScope { body, cleanup } => {
                if context.in_finally && !context.in_task_cleanup {
                    self.invalid_finally(
                        statement.range,
                        "`taskScope` is not allowed in `finally`",
                    );
                }
                if !context.function.is_async || context.owner_sync {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidAwait,
                        statement.range,
                        "`taskScope` requires an async Main function",
                        "use a taskScope inside an async function",
                    ));
                }
                context.facts.requires_owner = true;
                context.facts.may_await = true;
                self.check_block(body, context);
                let previous = (context.in_finally, context.in_task_cleanup);
                context.in_finally = true;
                context.in_task_cleanup = true;
                self.check_block(cleanup, context);
                (context.in_finally, context.in_task_cleanup) = previous;
            }
            StatementKind::TryAll {
                body,
                handler,
                cleanup,
            } => {
                let handlers = handler
                    .as_ref()
                    .map(|handler| {
                        crate::ast::CatchClause::whole(
                            &handler.binding,
                            handler.binding_range,
                            &handler.body,
                        )
                    })
                    .into_iter()
                    .collect::<Vec<_>>();
                self.check_error_handlers(body, &handlers, cleanup.as_ref(), context, true);
            }
            StatementKind::TryFinally { body, cleanup } => {
                self.check_block(body, context);
                let previous = (context.in_finally, context.in_task_cleanup);
                context.in_finally = true;
                context.in_task_cleanup = context.function.is_async;
                self.check_block(cleanup, context);
                (context.in_finally, context.in_task_cleanup) = previous;
            }
        }
    }

    fn check_switch(
        &mut self,
        enum_name: &str,
        value: SourceRange,
        cases: &[crate::ast::SwitchCase],
        context: &mut FunctionContext<'_>,
    ) {
        let Some(Declaration::Enum(enum_decl)) = self.declarations.get(enum_name).cloned() else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                cases.first().map_or(self.module.range, |case| case.range),
                format!("`{enum_name}` is not a closed enum"),
                "switch on a declared enum value",
            ));
            return;
        };
        let mut seen = HashSet::new();
        for case in cases {
            let Some(variant) = case.variant(&enum_decl) else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::UnknownName,
                    case.range,
                    format!("unknown `{enum_name}` variant `{}`", case.path.join(".")),
                    format!("use a variant declared by `{enum_name}`"),
                ));
                continue;
            };
            if !seen.insert(variant.name.clone()) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::DuplicateName,
                    case.range,
                    format!("variant `{}` appears more than once", variant.name),
                    "keep exactly one case for each variant",
                ));
            }
            if variant.fields.len() != case.bindings.len() {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    case.range,
                    format!(
                        "variant `{}` has {} payload fields but the case binds {}",
                        variant.name,
                        variant.fields.len(),
                        case.bindings.len()
                    ),
                    "bind one name for each payload field in declaration order",
                ));
            }
            let case_names = context.declared_names.clone();
            let mut inserted = Vec::new();
            for (name, field) in case.bindings.iter().zip(&variant.fields) {
                if let Some(previous) = context.declared_names.get(name).copied() {
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::DuplicateName,
                            case.range,
                            format!("case binding `{name}` shadows an existing name"),
                            "choose a unique binding name",
                        )
                        .with_secondary(previous, "existing declaration is here"),
                    );
                    continue;
                }
                if let Some(ty) = self.resolve_type(&field.ty) {
                    context.declared_names.insert(name.clone(), case.range);
                    context.locals.insert(
                        name.clone(),
                        Local {
                            callback_contract: callback_type_contract(&ty, Some(&field.ty)),
                            ty,
                            mutable: false,
                            constant: None,
                            targets: Vec::new(),
                        },
                    );
                    inserted.push(name.clone());
                }
            }
            self.check_block(&case.body, context);
            for name in inserted {
                context.locals.remove(&name);
            }
            context.declared_names = case_names;
        }
        let missing: Vec<_> = enum_decl
            .variants
            .iter()
            .filter(|variant| !seen.contains(&variant.name))
            .collect();
        if !missing.is_empty() {
            let names = missing
                .iter()
                .map(|variant| variant.name.as_str())
                .collect::<Vec<_>>();
            let mut diagnostic = Diagnostic::error(
                DiagnosticCode::NonExhaustiveSwitch,
                value,
                format!("switch is missing variants: {}", names.join(", ")),
                "add one case for every missing variant",
            );
            for variant in missing {
                if variant.range.file_id != crate::FileId::MANIFEST
                    && variant.range != enum_decl.range
                {
                    diagnostic = diagnostic.with_secondary(
                        variant.range,
                        format!("missing variant `{}` is declared here", variant.name),
                    );
                }
            }
            self.diagnostics.push(diagnostic);
        }
    }
}

pub(crate) fn block_always_returns(block: &Block) -> bool {
    block
        .statements
        .last()
        .is_some_and(statement_always_returns)
}

fn statement_always_returns(statement: &Statement) -> bool {
    match &statement.kind {
        StatementKind::Block(block) => block_always_returns(block),
        StatementKind::Return(_) | StatementKind::Throw(_) | StatementKind::Rethrow { .. } => true,
        StatementKind::If {
            then_block,
            else_branch: Some(else_branch),
            ..
        } => block_always_returns(then_block) && statement_always_returns(else_branch),
        // Exhaustiveness is checked separately. A valid empty switch consumes
        // an uninhabited enum and therefore has no normal fallthrough path.
        StatementKind::Switch { cases, .. } => {
            cases.iter().all(|case| block_always_returns(&case.body))
        }
        StatementKind::TryErrors {
            body,
            handlers,
            cleanup,
            ..
        } => {
            cleanup.as_ref().is_some_and(block_always_returns)
                || (block_always_returns(body)
                    && handlers
                        .iter()
                        .all(|handler| block_always_returns(&handler.body)))
        }
        StatementKind::TryCatch { body, handler, .. } => {
            block_always_returns(body) && block_always_returns(handler)
        }
        StatementKind::TryFinally { body, cleanup }
        | StatementKind::TaskScope { body, cleanup } => {
            block_always_returns(body) || block_always_returns(cleanup)
        }
        _ => false,
    }
}

//! Error values use ordinary managed slots. Catch dispatch preserves the whole
//! ordered failure list, and binds values only after a complete pattern match.
use super::*;
use crate::ast::{CatchClause, CatchPattern, Declaration};

pub(super) fn verify_groups(function: &IrFunction, diagnostics: &mut Vec<Diagnostic>) {
    let active = |block: &BasicBlock, index: usize, scope: SourceRange| {
        let mut scopes = block.entry_scopes.clone();
        for operation in &block.operations[..index] {
            match operation.kind {
                OperationKind::EnterScope { scope } => scopes.push(scope),
                OperationKind::ExitScope { scope, .. } => scopes.retain(|active| *active != scope),
                _ => {}
            }
        }
        scopes.contains(&scope)
    };
    let mut groups = HashMap::new();
    let mut finishes = HashMap::<SourceRange, usize>::new();
    for block in &function.blocks {
        for (index, operation) in block.operations.iter().enumerate() {
            if let OperationKind::EnterErrorGroup { scope } = operation.kind {
                let cleanup = block.operations.get(index + 1).and_then(|op| {
                    if let OperationKind::EnterTry { cleanup } = op.kind {
                        Some(cleanup)
                    } else {
                        None
                    }
                });
                if groups.insert(scope, cleanup).is_some()
                    || cleanup.is_none()
                    || operation.result.is_some()
                {
                    diagnostics.push(ir_error(
                        operation.range,
                        "Error group requires a unique entry with immediate cleanup registration",
                    ));
                }
            }
        }
    }
    for block in &function.blocks {
        for (index, operation) in block.operations.iter().enumerate() {
            match operation.kind {
                OperationKind::FinishErrorGroup { scope, .. } => {
                    *finishes.entry(scope).or_default() += 1;
                    let cleanup = groups.get(&scope).copied().flatten();
                    let valid = cleanup
                        .and_then(|id| function.blocks.iter().find(|block| block.id == id))
                        .is_some_and(|cleanup| active(block, index, cleanup.range));
                    if !valid
                        || index == 0
                        || !matches!(
                            block.operations[index - 1].kind,
                            OperationKind::GcSafepoint {
                                reason: PollReason::Allocation,
                                ..
                            }
                        )
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "Error group finish must run once in its rooted cleanup",
                        ));
                    }
                }
                OperationKind::ErrorGroupOutcome { scope, cleanup, .. } => {
                    let valid = groups.values().any(|entry| *entry == Some(cleanup))
                        && function
                            .blocks
                            .iter()
                            .any(|block| block.id == cleanup && block.range == scope);
                    if !valid || !active(block, index, scope) || operation.result.is_some() {
                        diagnostics.push(ir_error(
                            operation.range,
                            "Error group outcome lacks its registered cleanup",
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    for (scope, cleanup) in groups {
        let has_outcome = function
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .any(|operation| {
                matches!(operation.kind, OperationKind::ErrorGroupOutcome { cleanup: id, .. } if Some(id) == cleanup)
            });
        if finishes.remove(&scope) != Some(1) || !has_outcome {
            diagnostics.push(ir_error(
                scope,
                "Error group requires one finish and an outcome",
            ));
        }
    }
    for scope in finishes.keys() {
        diagnostics.push(ir_error(*scope, "Error group finish has no matching entry"));
    }
}

pub(super) fn verify_operation(
    module: &IrModule,
    operation: &Operation,
    index: usize,
    values: &HashMap<ValueId, Type>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let list = Type::List(Box::new(Type::Error));
    let result = operation.result.as_ref().map(|value| &value.ty);
    let nominal_error = |ty: &Type| matches!(ty, Type::Named(name) if module.source_types.get(name).is_some_and(|schema| schema.kind == "error"));
    let valid = match &operation.kind {
        OperationKind::EnterErrorGroup { .. } => result.is_none(),
        OperationKind::FinishErrorGroup { .. } => result == Some(&list),
        OperationKind::ThrowError { value, .. } => {
            values.get(value) == Some(&Type::Error) && result.is_none()
        }
        OperationKind::CatchErrors { observes_failure } => {
            index == 0 && *observes_failure && result == Some(&list)
        }
        OperationKind::ErrorCast { operand } => match (values.get(operand), result) {
            (Some(source), Some(Type::Error)) => nominal_error(source),
            (Some(Type::Error), Some(target)) => nominal_error(target),
            _ => false,
        },
        OperationKind::ErrorIs { operand, target } => {
            values
                .get(operand)
                .is_some_and(|ty| *ty == Type::Error || nominal_error(ty))
                && nominal_error(target)
                && result == Some(&Type::Bool)
        }
        OperationKind::ErrorField { error, field } => {
            let expected = match field {
                0 | 1 => Some(Type::String),
                2 => Some(Type::Named("Option<SourceLocation>".into())),
                3 => Some(Type::Named("Option<Error>".into())),
                _ => None,
            };
            values.get(error) == Some(&Type::Error)
                && expected.as_ref().is_some_and(|ty| result == Some(ty))
        }
        OperationKind::ErrorMatch { errors, types, .. } => {
            values.get(errors) == Some(&list)
                && result == Some(&Type::Bool)
                && types
                    .iter()
                    .all(|ty| *ty == Type::Error || nominal_error(ty))
        }
        OperationKind::ErrorIndex { errors, .. } => {
            values.get(errors) == Some(&list)
                && result.is_some_and(|ty| *ty == Type::Error || nominal_error(ty))
        }
        OperationKind::ErrorRest { errors, .. } => {
            values.get(errors) == Some(&list) && result == Some(&list)
        }
        _ => true,
    };
    if !valid {
        diagnostics.push(ir_error(
            operation.range,
            "invalid typed Error operation or catch entry",
        ));
    }
}

impl Builder<'_> {
    pub(super) fn error_metadata_type(&self, field: u32) -> Type {
        match field {
            0 | 1 => Type::String,
            2 => Type::Named("Option<SourceLocation>".into()),
            3 => Type::Named("Option<Error>".into()),
            _ => unreachable!("checked common Error field"),
        }
    }

    fn error_construct(
        &mut self,
        ty: Type,
        variant: Option<u32>,
        arguments: Vec<ValueId>,
        range: SourceRange,
    ) -> ValueId {
        let Type::Named(target) = &ty else {
            unreachable!("nominal Error/Option construction")
        };
        self.emit_poll(PollReason::Allocation, range);
        self.emit_value(
            OperationKind::Construct {
                target: target.clone(),
                variant,
                arguments,
                failure: self.failure,
                cleanup_chain: self.cleanup_chain(),
            },
            ty,
            range,
        )
    }

    pub(super) fn lower_error_constructor(
        &mut self,
        identity: &str,
        labels: &[String],
        arguments: &[ValueId],
        range: SourceRange,
    ) -> ValueId {
        let schema = self.checked.source_types[identity].clone();
        let optional_payload = self
            .checked
            .syntax
            .declarations
            .iter()
            .find_map(|decl| match decl {
                Declaration::Error(error) if error.name == identity => Some(
                    error
                        .payload
                        .as_ref()
                        .is_some_and(|payload| payload.optional),
                ),
                _ => None,
            })
            .expect("checked Error declaration");
        let argument = |name: &str| {
            labels
                .iter()
                .position(|label| label == name)
                .map(|index| arguments[index])
        };
        // The display name is independent of package identity and type arguments.
        let name = identity
            .split('<')
            .next()
            .unwrap()
            .rsplit("::")
            .next()
            .unwrap();
        self.emit_poll(PollReason::Allocation, range);
        let name = self.emit_value(
            OperationKind::Constant(ConstantValue::String(name.into())),
            Type::String,
            range,
        );
        let source = self.error_construct(schema.fields[2].1.clone(), Some(0), Vec::new(), range);
        let cause = self.error_construct(
            schema.fields[3].1.clone(),
            Some(u32::from(argument("cause").is_some())),
            argument("cause").into_iter().collect(),
            range,
        );
        let mut fields = vec![
            name,
            argument("message").expect("required message"),
            source,
            cause,
        ];
        if schema.fields.len() == 5 {
            let payload = if optional_payload {
                self.error_construct(
                    schema.fields[4].1.clone(),
                    Some(u32::from(argument("payload").is_some())),
                    argument("payload").into_iter().collect(),
                    range,
                )
            } else {
                argument("payload").expect("required payload")
            };
            fields.push(payload);
        }
        self.error_construct(Type::Named(identity.into()), None, fields, range)
    }

    pub(super) fn lower_throw(&mut self, expression: &Expression, range: SourceRange) {
        let value = self.lower_expression(expression);
        self.emit_poll(PollReason::Allocation, range);
        self.emit(
            OperationKind::ThrowError {
                value,
                failure: self.failure,
                error: self.error_edge(),
                cleanup_chain: self.cleanup_chain(),
            },
            None,
            range,
        );
        self.set_terminator(Terminator::Unreachable);
    }

    fn catch_binding(&mut self, name: &str, ty: Type, value: ValueId, range: SourceRange) {
        self.local_types.insert(name.into(), ty);
        self.scopes
            .last_mut()
            .expect("handler scope")
            .locals
            .push(name.into());
        self.emit(
            OperationKind::StoreLocal {
                name: name.into(),
                value,
            },
            None,
            range,
        );
    }

    pub(super) fn lower_error_handlers(
        &mut self,
        body: &Block,
        handlers: &[CatchClause],
        cleanup: Option<&Block>,
        range: SourceRange,
    ) {
        let previous_all = self.all_scope.take();
        let try_block = self.new_block(body.range);
        let catch = self.new_block(range);
        let after = self.new_block(range);
        let cleanup_block = cleanup.map(|cleanup| self.new_block(cleanup.range));
        if let Some(cleanup_block) = cleanup_block {
            self.emit(
                OperationKind::EnterTry {
                    cleanup: cleanup_block,
                },
                None,
                range,
            );
            self.cleanup_stack.push(cleanup_block);
            self.deferred_finally
                .insert(body.range, vec![cleanup_block]);
            for handler in handlers {
                self.deferred_finally
                    .insert(handler.body.range, vec![cleanup_block]);
            }
        }
        self.goto_if_open(try_block, ExitReason::Normal);
        self.catch_stack
            .push((catch, self.cleanup_stack.len(), self.scope_stack.len()));
        self.current = try_block;
        self.lower_block(body);
        self.goto_if_open(cleanup_block.unwrap_or(after), ExitReason::Normal);
        self.catch_stack.pop();
        self.current = catch;
        let errors = self.emit_value(
            OperationKind::CatchErrors {
                observes_failure: true,
            },
            Type::List(Box::new(Type::Error)),
            range,
        );
        self.lower_error_dispatch(errors, handlers, cleanup_block.unwrap_or(after), range);
        if let (Some(cleanup_block), Some(cleanup)) = (cleanup_block, cleanup) {
            self.cleanup_stack.pop();
            self.current = cleanup_block;
            self.emit(
                OperationKind::LeaveTry {
                    cleanup: cleanup_block,
                },
                None,
                cleanup.range,
            );
            self.lower_block(cleanup);
            self.goto_if_open(after, ExitReason::Normal);
        }
        self.current = after;
        if crate::sema::block_always_returns(body)
            && handlers
                .iter()
                .all(|handler| crate::sema::block_always_returns(&handler.body))
        {
            self.set_terminator(Terminator::Unreachable);
        }
        self.all_scope = previous_all;
    }

    pub(super) fn lower_try_all(
        &mut self,
        body: &Block,
        handlers: &[CatchClause],
        cleanup: Option<&Block>,
        scope: SourceRange,
    ) {
        let dispatch_range = SourceRange {
            instance: scope
                .instance
                .wrapping_add(0x70000000 + self.blocks.len() as u32),
            ..scope
        };
        let cleanup_range = SourceRange {
            instance: dispatch_range.instance.wrapping_add(1),
            ..scope
        };
        self.all_bodies.insert(body.range, scope);
        self.all_dispatch
            .insert(dispatch_range, (scope, handlers.to_vec()));
        let dispatch = Block {
            range: dispatch_range,
            statements: Vec::new(),
        };
        let cleanup = if let Some(cleanup) = cleanup {
            Block {
                range: cleanup_range,
                statements: vec![Statement {
                    range: cleanup_range,
                    kind: StatementKind::TryFinally {
                        body: dispatch,
                        cleanup: cleanup.clone(),
                    },
                }],
            }
        } else {
            dispatch
        };
        self.all_outcomes.insert(cleanup.range);
        self.emit(OperationKind::EnterErrorGroup { scope }, None, scope);
        self.lower_statement(&Statement {
            range: scope,
            kind: StatementKind::TryFinally {
                body: body.clone(),
                cleanup,
            },
        });
    }

    pub(super) fn lower_all_dispatch(
        &mut self,
        scope: SourceRange,
        handlers: &[CatchClause],
        range: SourceRange,
    ) {
        let ty = Type::List(Box::new(Type::Error));
        self.emit_gc(PollReason::Allocation, range);
        let errors = self.emit_value(
            OperationKind::FinishErrorGroup {
                scope,
                handles: !handlers.is_empty(),
            },
            ty.clone(),
            range,
        );
        if handlers.is_empty() {
            return;
        }
        // Spill before branching: values in this IR belong to their block.
        let saved = format!("$rinzu_all_list{}", errors.0);
        self.expression_local(saved.clone(), ty.clone());
        self.emit(
            OperationKind::StoreLocal {
                name: saved.clone(),
                value: errors,
            },
            None,
            range,
        );
        self.emit_poll(PollReason::Allocation, range);
        let count = self.emit_value(
            OperationKind::DataOperation {
                kind: crate::DataOperation::ListLength,
                arguments: vec![errors],
                argument_types: vec![ty.clone()],
                error: self.error_edge(),
                failure: self.failure,
                cleanup_chain: self.cleanup_chain(),
            },
            Type::Int,
            range,
        );
        let zero = self.emit_value(
            OperationKind::Constant(ConstantValue::Int(0)),
            Type::Int,
            range,
        );
        let condition = self.emit_value(
            OperationKind::Binary {
                operator: crate::ast::BinaryOperator::Greater,
                left: count,
                right: zero,
                failure: None,
                cleanup_chain: Vec::new(),
            },
            Type::Bool,
            range,
        );
        let accepted = self.new_block(range);
        let after = self.new_block(range);
        self.set_terminator(Terminator::Branch {
            condition,
            then_target: accepted,
            else_target: after,
        });
        self.current = accepted;
        let errors = self.emit_value(
            OperationKind::LoadLocal {
                name: saved.clone(),
            },
            ty,
            range,
        );
        self.lower_error_dispatch(errors, handlers, after, range);
        self.current = after;
        self.local_types.remove(&saved);
    }

    fn lower_error_dispatch(
        &mut self,
        errors: ValueId,
        handlers: &[CatchClause],
        after: BlockId,
        range: SourceRange,
    ) {
        let saved = format!("$errors{}", errors.0);
        let list_type = Type::List(Box::new(Type::Error));
        self.local_types.insert(saved.clone(), list_type.clone());
        let scope = *self.scope_stack.last().expect("catch parent scope");
        self.scopes
            .iter_mut()
            .find(|info| info.scope == scope)
            .unwrap()
            .locals
            .push(saved.clone());
        self.emit(
            OperationKind::StoreLocal {
                name: saved.clone(),
                value: errors,
            },
            None,
            range,
        );
        for handler in handlers {
            let accepted = self.new_block(handler.body.range);
            let next = self.new_block(range);
            match &handler.pattern {
                CatchPattern::Binding(_) => self.goto_if_open(accepted, ExitReason::Normal),
                CatchPattern::List { elements, rest, .. } => {
                    let errors = self.emit_value(
                        OperationKind::LoadLocal {
                            name: saved.clone(),
                        },
                        list_type.clone(),
                        handler.range,
                    );
                    let types = elements
                        .iter()
                        .map(|element| self.checked.error_patterns[&element.range].clone())
                        .collect();
                    let condition = self.emit_value(
                        OperationKind::ErrorMatch {
                            errors,
                            types,
                            rest: rest.is_some(),
                        },
                        Type::Bool,
                        handler.range,
                    );
                    self.set_terminator(Terminator::Branch {
                        condition,
                        then_target: accepted,
                        else_target: next,
                    });
                }
            }
            self.current = accepted;
            self.lower_block_with_prelude(&handler.body, |builder| {
                let errors = builder.emit_value(
                    OperationKind::LoadLocal {
                        name: saved.clone(),
                    },
                    list_type.clone(),
                    handler.range,
                );
                match &handler.pattern {
                    CatchPattern::Binding(binding) => builder.catch_binding(
                        &binding.name,
                        Type::List(Box::new(Type::Error)),
                        errors,
                        binding.range,
                    ),
                    CatchPattern::List { elements, rest, .. } => {
                        for (index, element) in elements.iter().enumerate() {
                            if let Some(binding) = &element.binding {
                                let ty = builder.checked.error_patterns[&element.range].clone();
                                let value = builder.emit_value(
                                    OperationKind::ErrorIndex {
                                        errors,
                                        index: index as u32,
                                    },
                                    ty.clone(),
                                    element.range,
                                );
                                builder.catch_binding(&binding.name, ty, value, binding.range);
                            }
                        }
                        if let Some(binding) = rest {
                            let ty = Type::List(Box::new(Type::Error));
                            builder.emit_poll(PollReason::Allocation, binding.range);
                            let value = builder.emit_value(
                                OperationKind::ErrorRest {
                                    errors,
                                    start: elements.len() as u32,
                                },
                                ty.clone(),
                                binding.range,
                            );
                            builder.catch_binding(&binding.name, ty, value, binding.range);
                        }
                    }
                }
            });
            self.goto_if_open(after, ExitReason::Normal);
            self.current = next;
        }
        // Inferred local exhaustiveness guarantees no unmatched Error list.
        self.set_terminator(Terminator::Unreachable);
        self.local_types.remove(&saved);
    }
}

//! Ordinary Error cannot escape an entry published to native code. The
//! synchronous adapter's explicit constant fallback covers input conversion as
//! well as the body; a catch in the body only covers the latter.
use super::*;

impl Engine<'_> {
    pub fn check_native_entries(
        &self,
        expression: &Expression,
        arguments: &[&Expression],
        values: &[Tracked],
        state: &mut State,
    ) {
        let source_abi = !self.checked.native_calls.contains_key(&expression.range);
        for (argument, value) in arguments.iter().zip(values) {
            let captured = self.checked.native_captures.contains_key(&argument.range);
            let targets = self
                .checked
                .reference_targets
                .get(&argument.range)
                .cloned()
                .or_else(|| {
                    if let ExpressionKind::Name(path) = &argument.kind
                        && matches!(self.type_at(argument.range), Type::StaticHandler(_))
                    {
                        Some(vec![path.join(".")])
                    } else {
                        None
                    }
                });
            let Some(targets) = targets else { continue };
            for (index, name) in targets.into_iter().enumerate() {
                if !captured
                    && self
                        .checked
                        .source_native_signatures
                        .iter()
                        .any(|signature| signature.name == name)
                {
                    // This is the original physical C function pointer. It
                    // enters no managed body or managed input adapter.
                    continue;
                }
                let function = self
                    .checked
                    .functions
                    .iter()
                    .find(|function| function.declaration.name == name);
                if function.is_some_and(|function| function.callback_fallback.is_some()) {
                    // Sema checked a constant matching the result ABI. Both
                    // bridges settle/release the failure before returning it.
                    continue;
                }
                let effect = value.effect();
                let candidates = if captured {
                    let Type::Named(ty) = &self.checked.native_captures[&argument.range] else {
                        unreachable!("checked private callback environment")
                    };
                    effect
                        .project(
                            Projection::Field(ty.clone(), "environment".into()),
                            Value::Bottom,
                        )
                        .selection_guards(&format!("V{index}"))
                } else if matches!(self.type_at(argument.range), Type::StaticHandler(_)) {
                    vec![BTreeMap::new()]
                } else {
                    effect.target_guards(&name)
                };
                if candidates.is_empty() {
                    continue;
                }
                let mut errors = Language::new();
                if let Some(function) = function {
                    let mut inputs: Vec<_> = function
                        .parameter_types
                        .iter()
                        .map(|ty| self.typed(ty))
                        .collect();
                    if captured && let Some(first) = inputs.first_mut() {
                        *first = effect;
                    }
                    if let Some(contract) = self.contracts.get(&name) {
                        for exit in &contract.exits {
                            if let Err(Stop::Error(word, _)) = &exit.result {
                                let words = language::restricted(
                                    &Language::from([word.clone()]),
                                    &exit.guards,
                                );
                                errors.extend(substitution::language(
                                    &words, &name, &inputs, self.bound,
                                ));
                            }
                        }
                    } else {
                        errors = language::unknown(self.bound);
                    }
                } else if crate::core::reference_signature(&name).is_none() {
                    errors = language::unknown(self.bound);
                }
                let errors = candidates
                    .iter()
                    .flat_map(|guard| language::restricted(&errors, guard))
                    .collect();
                self.entry_requirement(state, argument.range, RequirementKind::Native, errors);

                if source_abi {
                    let inputs = function
                        .map(|function| function.parameter_types[usize::from(captured)..].to_vec())
                        .unwrap_or_else(|| match self.type_at(argument.range) {
                            Type::FunctionReference(signature) => signature.parameters,
                            _ => Vec::new(),
                        });
                    if inputs.iter().any(native_input_can_fail) {
                        let errors = candidates
                            .iter()
                            .flat_map(|guard| language::restricted(&language::one(Tag::Any), guard))
                            .collect();
                        self.entry_requirement(
                            state,
                            argument.range,
                            RequirementKind::NativeInput,
                            errors,
                        );
                    }
                }
            }
        }
    }
    fn entry_requirement(
        &self,
        state: &mut State,
        at: SourceRange,
        kind: RequirementKind,
        errors: Language,
    ) {
        let errors = language::restricted(&errors, &state.guards);
        if !errors.is_empty() {
            let requirement = Requirement {
                kind,
                origin: self.at(at),
                exit: self.at(at),
                errors,
            };
            if !state.requirements.contains(&requirement) {
                state.requirements.push(requirement);
            }
        }
    }
}
fn native_input_can_fail(ty: &Type) -> bool {
    match ty {
        // The physical ABI decoder canonicalizes all numeric scalar widths.
        Type::Unit | Type::Int | Type::Float | Type::FixedBytes(_) => false,
        Type::Scalar(scalar) => *scalar == crate::AbiScalar::Bool,
        Type::Bool | Type::String | Type::Native { .. } | Type::Named(_) => true,
        // Unsupported ABI shapes must not establish infallibility.
        _ => true,
    }
}

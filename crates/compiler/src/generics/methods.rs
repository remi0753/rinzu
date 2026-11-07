//! Resolve generic owner/method groups before ordinary monomorphization.
use super::*;

impl Expander<'_> {
    pub(super) fn method_expression(
        &mut self,
        expression: &mut Expression,
        substitution: &HashMap<String, TypeRef>,
        instance: u32,
        expected: Option<&TypeRef>,
    ) -> Result<bool, Diagnostic> {
        let (target, owner_args, member, method_args, reference) = match &expression.kind {
            ExpressionKind::GenericCall {
                target,
                type_arguments,
                variant,
                member_type_arguments,
                ..
            } if variant.is_some() => (
                target.clone(),
                type_arguments.clone(),
                variant.clone(),
                member_type_arguments.clone(),
                false,
            ),
            ExpressionKind::GenericReference {
                target,
                type_arguments,
                member,
                member_type_arguments,
            } if member.is_some() => (
                target.clone(),
                type_arguments.clone(),
                member.clone(),
                member_type_arguments.clone(),
                true,
            ),
            ExpressionKind::GenericCall {
                target,
                type_arguments,
                variant: None,
                ..
            } if self.local_target(target) && target.len() > 1 => (
                target.clone(),
                Vec::new(),
                None,
                type_arguments.clone(),
                false,
            ),
            ExpressionKind::Call { target, .. } | ExpressionKind::Spawn { target, .. }
                if self.local_target(target) && target.len() > 1 =>
            {
                (target.clone(), Vec::new(), None, Vec::new(), false)
            }
            ExpressionKind::GenericReference { target, .. } if self.local_target(target) => {
                return Err(error(
                    expression.range,
                    "instance method references do not bind their receiver; use a closure or an unbound Owner.method reference",
                ));
            }
            _ => return Ok(false),
        };
        let mut receiver = None;
        let (base, member, mut fixed) = if let Some(member) = member {
            (target.join("."), member, owner_args)
        } else {
            let receiver_path = target[..target.len() - 1].to_vec();
            let range = SourceRange {
                end_byte: expression.range.start_byte + receiver_path.join(".").len() as u32,
                ..expression.range
            };
            let source = Expression {
                kind: ExpressionKind::Name(receiver_path),
                range,
            };
            let Some(ty) = self.probe_type(&source, None) else {
                return Ok(false);
            };
            let TypeRefKind::Named(path) = ty.kind else {
                return Ok(false);
            };
            let owner = path.join(".");
            let (base, args) = self
                .instance_types
                .get(&owner)
                .cloned()
                .unwrap_or((owner, Vec::new()));
            receiver = Some(source);
            (base, target.last().unwrap().clone(), args)
        };
        let name = format!("{base}.{member}");
        let Some(Declaration::Function(function)) = self.templates.get(&name).cloned() else {
            if self.inference_active
                && receiver.is_some()
                && let ExpressionKind::Call { arguments, .. } = &mut expression.kind
                && let Some(Declaration::Function(method)) = self.declaration_named(&name)
                && method.parameters.first().is_some_and(|p| p.name == "self")
            {
                let formals = method
                    .parameters
                    .iter()
                    .skip(1)
                    .map(|p| (p.name.clone(), p.ty.clone()))
                    .collect::<Vec<_>>();
                self.arguments_with_formals(arguments, &formals, substitution, instance)?;
                return Ok(true);
            }
            return Ok(false);
        };
        let owner_count = function.owner_type_parameter_count;
        if fixed.len() != owner_count {
            return Err(error(
                expression.range,
                "owner type argument count does not match the method's owner",
            ));
        }
        if receiver.is_some()
            && !function
                .parameters
                .first()
                .is_some_and(|p| p.name == "self")
        {
            return Err(error(
                expression.range,
                "static methods must be called through their owner type",
            ));
        }
        if !method_args.is_empty()
            && method_args.len() != function.type_parameters.len() - owner_count
        {
            return Err(error(
                expression.range,
                "method type argument count does not match the declaration",
            ));
        }
        fixed.extend(method_args);
        for t in &mut fixed {
            self.ty(t, substitution, instance)?;
            self.canonical_argument(t)?;
        }
        let spawn = matches!(
            expression.kind,
            ExpressionKind::Spawn { .. } | ExpressionKind::GenericCall { spawn: true, .. }
        );
        let mut arguments = match &expression.kind {
            ExpressionKind::Call { arguments, .. }
            | ExpressionKind::Spawn { arguments, .. }
            | ExpressionKind::GenericCall { arguments, .. } => arguments.clone(),
            _ => Arguments::Positional(Vec::new()),
        };
        if let Some(receiver) = receiver {
            let Arguments::Positional(values) = &mut arguments else {
                return Err(error(
                    expression.range,
                    "method calls use positional arguments",
                ));
            };
            values.insert(0, receiver);
        }
        let args = if fixed.len() == function.type_parameters.len() {
            fixed
        } else if reference {
            self.inferred_reference(&name, &fixed, expected, instance, expression.range)?
        } else {
            self.inferred_arguments(
                &name,
                None,
                &mut arguments,
                expected,
                substitution,
                instance,
                expression.range,
                &fixed,
            )?
        };
        let concrete = self.instantiate(&name, &args, expression.range)?;
        if reference {
            expression.kind = ExpressionKind::Name(vec![concrete]);
        } else {
            self.arguments_expected(
                std::slice::from_ref(&concrete),
                &mut arguments,
                substitution,
                instance,
            )?;
            expression.kind = if spawn {
                ExpressionKind::Spawn {
                    target: vec![concrete],
                    arguments,
                }
            } else {
                ExpressionKind::Call {
                    target: vec![concrete],
                    arguments,
                }
            };
        }
        Ok(true)
    }
}

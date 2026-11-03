//! Values crossing a retention boundary must be free of Future handles even
//! after successful observation. Closed closure variants retain their actual
//! environment here; a callable's return type is not a captured value.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Boundary {
    ErrorPayload,
    Native,
    NativeCapture,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct Restriction {
    pub origin: Site,
    pub boundary: Boundary,
    pub inputs: BTreeSet<Input>,
    pub forbidden: bool,
}

impl Engine<'_> {
    pub fn restrict_values(
        &self,
        boundary: Boundary,
        at: Site,
        values: impl IntoIterator<Item = Value>,
        state: &mut State,
    ) {
        let mut restriction = Restriction {
            origin: at,
            boundary,
            inputs: BTreeSet::new(),
            forbidden: false,
        };
        for value in values {
            self.retained_value(&value, &mut restriction);
        }
        if restriction.forbidden || !restriction.inputs.is_empty() {
            state.restrictions.insert(restriction);
        }
    }

    pub fn transfer_restrictions(
        &self,
        restrictions: &BTreeSet<Restriction>,
        name: &str,
        effects: &[Value],
        state: &mut State,
    ) {
        for restriction in restrictions {
            let mut next = Restriction {
                inputs: BTreeSet::new(),
                ..restriction.clone()
            };
            for input in &restriction.inputs {
                let value = substitution::value(
                    &Value::Parameter {
                        owner: input.owner.clone(),
                        index: input.index,
                        path: input.path.clone(),
                        fallback: Box::new(Value::Bottom),
                    },
                    name,
                    effects,
                    self.bound,
                );
                self.retained_value(&value, &mut next);
            }
            if next.forbidden || !next.inputs.is_empty() {
                state.restrictions.insert(next);
            }
        }
    }

    fn retained_value(&self, value: &Value, restriction: &mut Restriction) {
        match value {
            Value::Future { .. } | Value::NeverFuture | Value::PanicFuture => {
                restriction.forbidden = true
            }
            Value::Error(_) if restriction.boundary == Boundary::Native => {
                restriction.forbidden = true;
            }
            Value::Opaque(name) => {
                restriction.forbidden |= self.retained_type(
                    &Type::Named(name.clone()),
                    restriction.boundary,
                    &mut BTreeSet::new(),
                );
            }
            Value::Parameter {
                owner, index, path, ..
            } => {
                // A closed primitive parameter cannot acquire a Future or
                // Error through later substitution. Function references and
                // aggregates still require their actual retained environment.
                if path.is_empty()
                    && self
                        .checked
                        .functions
                        .iter()
                        .find(|function| function.declaration.name == *owner)
                        .and_then(|function| function.parameter_types.get(*index))
                        .is_some_and(|ty| {
                            matches!(
                                ty,
                                Type::Unit
                                    | Type::Bool
                                    | Type::Int
                                    | Type::Float
                                    | Type::Scalar(_)
                                    | Type::String
                                    | Type::Bytes
                                    | Type::FixedBytes(_)
                            )
                        })
                {
                    return;
                }
                // Recursive forwarding through a nominal cycle must not grow
                // an unbounded chain of summary projections. At that point
                // only the sealed schema can prove the retained data safe.
                let mut seen = BTreeSet::new();
                for projection in path {
                    let ty = match projection {
                        Projection::Field(ty, _) | Projection::Variant(ty, _, _) => ty,
                        _ => continue,
                    };
                    if !seen.insert(ty) {
                        restriction.forbidden |= self.retained_type(
                            &Type::Named(ty.clone()),
                            restriction.boundary,
                            &mut BTreeSet::new(),
                        );
                        return;
                    }
                }
                restriction.inputs.insert(Input {
                    owner: owner.clone(),
                    index: *index,
                    path: path.clone(),
                });
            }
            Value::Aggregate { fields, .. } => {
                for value in fields.values() {
                    self.retained_value(value, restriction);
                }
            }
            Value::Variants { variants, .. } => {
                for value in variants.values().flatten() {
                    self.retained_value(value, restriction);
                }
            }
            Value::Union(values) => {
                for value in values {
                    self.retained_value(value, restriction);
                }
            }
            Value::Conditional { value, .. } | Value::Unbound(value) | Value::Collection(value) => {
                self.retained_value(value, restriction);
            }
            _ => {}
        }
    }

    fn retained_type(&self, ty: &Type, boundary: Boundary, seen: &mut BTreeSet<String>) -> bool {
        match ty {
            Type::Future(_) => true,
            Type::Error => boundary == Boundary::Native,
            Type::List(inner) => self.retained_type(inner, boundary, seen),
            Type::Map(key, value) => {
                self.retained_type(key, boundary, seen) || self.retained_type(value, boundary, seen)
            }
            Type::Named(name) if seen.insert(name.clone()) => {
                let forbidden = self.checked.source_types.get(name).is_some_and(|schema| {
                    if schema.kind == "error" {
                        // Every Error constructor already establishes its
                        // payload invariant, including concrete captures.
                        return boundary == Boundary::Native;
                    }
                    schema
                        .fields
                        .iter()
                        .any(|(_, ty)| self.retained_type(ty, boundary, seen))
                        || schema.variants.iter().any(|(_, fields)| {
                            fields
                                .iter()
                                .any(|(_, ty)| self.retained_type(ty, boundary, seen))
                        })
                });
                seen.remove(name);
                forbidden
            }
            _ => false,
        }
    }
}

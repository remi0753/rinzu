use super::*;

impl Analyzer<'_> {
    pub(super) fn condition_value(&self, value: Value, paths: &[language::Guards]) -> Value {
        let mut result = Value::Bottom;
        for guards in paths {
            result.join(value.clone().conditioned(guards.clone()));
        }
        result
    }
    pub(super) fn candidate_guards(
        &self,
        expression: &Expression,
        target: &[String],
        env: &Environment,
        name: &str,
    ) -> Vec<language::Guards> {
        if self.checked.indirect_calls.contains_key(&expression.range)
            && env.contains_key(&target[0])
        {
            self.project_name(target, env, Value::Plain)
                .target_guards(name)
        } else {
            vec![BTreeMap::new()]
        }
    }
    pub(super) fn condition_branch(
        &self,
        mut branch: Flow,
        env: &mut Environment,
        paths: &[language::Guards],
    ) -> Flow {
        let restrict = |errors: &Language| {
            paths
                .iter()
                .flat_map(|guards| language::restricted(errors, guards))
                .collect()
        };
        let restrict_value = |value: &Value| {
            let mut result = Value::Bottom;
            for guards in paths {
                result.join(value.clone().conditioned(guards.clone()));
            }
            result
        };
        branch.errors = restrict(&branch.errors);
        branch.returned = restrict_value(&branch.returned);
        branch.collected = branch.collected.as_ref().map(restrict);
        for value in env.values_mut() {
            *value = restrict_value(value);
        }
        branch
    }

    /// The converter's function object has one private environment field. It
    /// is never writable by source programs. Keep its existing state schema
    /// and marker/ABI rules, while retaining its constructor's immutable facts.
    fn closure_environment(&self, ty: &str) -> bool {
        let Some(file) = self.checked.closure_source else {
            return false;
        };
        self.checked.syntax.declarations.iter().any(|declaration| {
            let Declaration::Record(record) = declaration else {
                return false;
            };
            record.name == ty
                && record.range.file_id == file
                && record.fields.len() == 1
                && record.fields[0].name == "environment"
        })
    }
    pub(super) fn project_name(
        &self,
        path: &[String],
        env: &Environment,
        fallback: Value,
    ) -> Value {
        let Some(value) = env.get(&path[0]) else {
            return fallback;
        };
        let mut value = value.clone();
        if let Value::Unbound(inner) = value {
            value = *inner;
        }
        let mut owner = value.nominal().map(str::to_owned);
        for field in &path[1..] {
            let next = owner
                .as_ref()
                .and_then(|owner| self.checked.source_types.get(owner))
                .and_then(|schema| schema.fields.iter().find(|(name, _)| name == field))
                .map(|(_, ty)| ty.clone());
            value = value.project(
                Projection::Field(owner.clone().unwrap_or_default(), field.clone()),
                next.as_ref()
                    .map_or_else(|| fallback.clone(), |ty| self.typed(ty)),
            );
            owner = if let Some(Type::Named(name)) = next {
                Some(name)
            } else {
                None
            };
        }
        value
    }

    pub(super) fn variant_field(&self, ty: Option<&Type>, variant: &str, index: usize) -> Value {
        let Some(Type::Named(name)) = ty else {
            return Value::Plain;
        };
        self.checked
            .source_types
            .get(name)
            .and_then(|schema| schema.variants.iter().find(|(name, _)| name == variant))
            .and_then(|(_, fields)| fields.get(index))
            .map_or(Value::Plain, |(_, ty)| self.typed(ty))
    }

    pub(super) fn record_value(
        &mut self,
        expression: &Expression,
        fields: &[RecordField],
        env: &Environment,
    ) -> Evaluated {
        let mut result = Evaluated::pure(Value::Bottom);
        let mut values = BTreeMap::new();
        for field in fields {
            let evaluated = self.expression(&field.value, env, false);
            if result.success {
                result.errors.extend(evaluated.errors);
                result.success = evaluated.success;
            }
            values.insert(field.name.clone(), evaluated.value);
        }
        result.value = self
            .immutable_record(expression, values)
            .unwrap_or_else(|| self.expression_value(expression));
        result
    }

    fn immutable_record(
        &self,
        expression: &Expression,
        fields: BTreeMap<String, Value>,
    ) -> Option<Value> {
        let Type::Named(ty) = self.checked.expression_type(expression.range)? else {
            return None;
        };
        let schema = self.checked.source_types.get(ty)?;
        // A class alias may be mutated through another name. Preserve facts
        // only for immutable records / classes whose fields are all const.
        (schema.kind == "record" || self.closure_environment(ty)).then(|| Value::Aggregate {
            ty: ty.clone(),
            fields,
        })
    }

    pub(super) fn record_update(
        &mut self,
        expression: &Expression,
        base: &Expression,
        fields: &[(String, Expression, SourceRange)],
        env: &Environment,
    ) -> Evaluated {
        let mut result = self.expression(base, env, false);
        let mut values = if let Value::Aggregate { fields, .. } = &result.value {
            fields.clone()
        } else {
            BTreeMap::new()
        };
        for (name, value, _) in fields {
            let value = self.expression(value, env, false);
            if result.success {
                result.errors.extend(value.errors);
                result.success = value.success;
            }
            values.insert(name.clone(), value.value);
        }
        result.value = self
            .immutable_record(expression, values)
            .unwrap_or_else(|| self.expression_value(expression));
        result
    }

    pub(super) fn collection_literal<'a>(
        &mut self,
        entries: impl Iterator<Item = (Option<&'a Expression>, &'a Expression)>,
        env: &Environment,
    ) -> Evaluated {
        let mut result = Evaluated::pure(Value::Bottom);
        let mut element = Value::Bottom;
        for (key, value) in entries {
            if let Some(key) = key {
                let key = self.expression(key, env, false);
                if result.success {
                    result.errors.extend(key.errors);
                    result.success = key.success;
                }
            }
            let value = self.expression(value, env, false);
            if result.success {
                result.errors.extend(value.errors);
                result.success = value.success;
            }
            element.join(value.value);
        }
        result.value = Value::Collection(Box::new(element));
        result
    }

    pub(super) fn constructed_value(
        &self,
        expression: &Expression,
        target: &[String],
        arguments: &Arguments,
        values: &[Value],
    ) -> Option<Value> {
        let Type::Named(ty) = self.checked.expression_type(expression.range)? else {
            return None;
        };
        let schema = self.checked.source_types.get(ty)?;
        let target_name = target.join(".");
        if (schema.kind == "record" || self.closure_environment(ty)) && target_name == *ty {
            let fields = match arguments {
                Arguments::Named(arguments) => arguments
                    .iter()
                    .zip(values)
                    .map(|((name, _, _), value)| (name.clone(), value.clone()))
                    .collect(),
                Arguments::Positional(_) => schema
                    .fields
                    .iter()
                    .zip(values)
                    .map(|((name, _), value)| (name.clone(), value.clone()))
                    .collect(),
            };
            return Some(Value::Aggregate {
                ty: ty.clone(),
                fields,
            });
        }
        if schema.kind == "enum" && target.len() > 1 && target[..target.len() - 1].join(".") == *ty
        {
            let variant = target.last()?;
            let fields = &schema.variants.iter().find(|(name, _)| name == variant)?.1;
            let values = match arguments {
                Arguments::Positional(_) => values.to_vec(),
                Arguments::Named(arguments) => fields
                    .iter()
                    .map(|(field, _)| {
                        arguments
                            .iter()
                            .position(|(name, _, _)| name == field)
                            .and_then(|index| values.get(index))
                            .cloned()
                            .unwrap_or(Value::Plain)
                    })
                    .collect(),
            };
            return Some(Value::Variants {
                ty: ty.clone(),
                variants: BTreeMap::from([(variant.clone(), values)]),
            });
        }
        None
    }

    pub(super) fn collection_operation(
        &self,
        expression: &Expression,
        values: &[Value],
    ) -> Option<Value> {
        use crate::DataOperation::*;
        let operation = self.checked.data_operations.get(&expression.range)?;
        let first = values.first()?;
        let collection_type = match &expression.kind {
            ExpressionKind::Call {
                arguments: Arguments::Positional(values),
                ..
            }
            | ExpressionKind::GenericCall {
                arguments: Arguments::Positional(values),
                ..
            } => values
                .first()
                .and_then(|value| self.checked.expression_type(value.range))
                .map_or_else(String::new, ToString::to_string),
            _ => String::new(),
        };
        let option = |value| {
            let ty = self
                .checked
                .expression_type(expression.range)
                .unwrap()
                .to_string();
            Value::Variants {
                ty,
                variants: BTreeMap::from([("None".into(), vec![]), ("Some".into(), vec![value])]),
            }
        };
        Some(match operation {
            ListGet | MapGet => {
                let fallback =
                    self.variant_field(self.checked.expression_type(expression.range), "Some", 0);
                let at = expression.range;
                option(first.project(
                    Projection::Element(
                        collection_type,
                        (at.file_id.0, at.start_byte, at.end_byte, at.instance, 0),
                    ),
                    fallback,
                ))
            }
            ListAppend | MapInsert | ListSet => {
                let added = values.last()?.clone();
                let fallback = match self.checked.expression_type(expression.range) {
                    Some(Type::List(element)) => self.typed(element),
                    Some(Type::Map(_, value)) => self.typed(value),
                    _ => Value::Plain,
                };
                let mut element = first.project(Projection::AllElements(collection_type), fallback);
                element.join(added);
                let collection = Value::Collection(Box::new(element));
                if *operation == ListSet {
                    option(collection)
                } else {
                    collection
                }
            }
            ListRemove => option(first.clone()),
            MapRemove => first.clone(),
            _ => return None,
        })
    }
}

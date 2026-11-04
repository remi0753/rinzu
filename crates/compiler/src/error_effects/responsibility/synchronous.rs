//! Interpret synchronous managed calls with their actual heap aliases. Joining
//! independent parameter writes first would erase the ordering when they alias.
use super::*;
use expressions::Evaluation;

impl Engine<'_> {
    pub fn contains_mutable_class(&self, ty: &Type, seen: &mut BTreeSet<String>) -> bool {
        match ty {
            Type::List(inner) | Type::Future(inner) => self.contains_mutable_class(inner, seen),
            Type::Map(key, value) => {
                self.contains_mutable_class(key, seen) || self.contains_mutable_class(value, seen)
            }
            Type::Named(name) if seen.insert(name.clone()) => {
                let result = self.checked.source_types.get(name).is_some_and(|schema| {
                    schema.kind == "native-record"
                        || (schema.kind == "state"
                            && !self.private_environment(name)
                            && schema.const_fields.len() != schema.fields.len())
                        || schema
                            .fields
                            .iter()
                            .any(|(_, ty)| self.contains_mutable_class(ty, seen))
                        || schema.variants.iter().any(|(_, fields)| {
                            fields
                                .iter()
                                .any(|(_, ty)| self.contains_mutable_class(ty, seen))
                        })
                });
                seen.remove(name);
                result
            }
            Type::FunctionReference(_) => true,
            _ => false,
        }
    }
    pub fn inline_synchronous(
        &self,
        expression: &Expression,
        function: &crate::CheckedFunction,
        values: &[Tracked],
        mut state: State,
    ) -> Vec<Evaluation> {
        let context = {
            let mut contexts = self.contexts.borrow_mut();
            let next = u32::try_from(contexts.len() + 1)
                .expect("too many responsibility analysis contexts");
            *contexts
                .entry((self.context, site(expression.range)))
                .or_insert(next)
        };
        // Keep caller roots in this state under inaccessible names. Heap/task
        // aging therefore updates caller aliases even inside nested calls.
        let prefix = format!("#caller:{context}:");
        state.env = std::mem::take(&mut state.env).rename_keys(|name| format!("{prefix}{name}"));
        for (parameter, value) in function.declaration.parameters.iter().zip(values) {
            state.env.insert(parameter.name.clone(), value.clone());
        }
        let mut stack = self.stack.clone();
        stack.push(function.declaration.name.clone());
        let nested = Engine {
            checked: self.checked,
            effects: self.effects,
            contracts: self.contracts,
            bound: self.bound,
            current: &function.declaration.name,
            recursive_calls: std::cell::Cell::new(false),
            non_decreasing: std::cell::Cell::new(false),
            cyclic: false,
            mutates: std::cell::Cell::new(false),
            mutates_async: std::cell::Cell::new(false),
            disrupts_observers: std::cell::Cell::new(false),
            contexts: self.contexts,
            data_functions: self.data_functions,
            context,
            stack,
            allow_field_watch: self.allow_field_watch,
        };
        let paths = nested.block(
            &function.declaration.body,
            vec![Path {
                state,
                control: Control::Normal,
            }],
            false,
        );
        self.mutates.set(self.mutates.get() || nested.mutates.get());
        self.mutates_async
            .set(self.mutates_async.get() || nested.mutates_async.get());
        self.disrupts_observers
            .set(self.disrupts_observers.get() || nested.disrupts_observers.get());
        paths
            .into_iter()
            .map(|path| {
                let mut state = path.state;
                state.env = state
                    .env
                    .filter_map_keys(|name| name.strip_prefix(&prefix).map(str::to_owned));
                Evaluation {
                    state,
                    result: match path.control {
                        Control::Normal => Ok(Tracked::data(Value::Plain)),
                        Control::Return(value) => Ok(value),
                        Control::Error(word, token) => Err(Stop::Error(Box::new(word), token)),
                        Control::Recurse(held) => Err(Stop::Recurse(held)),
                        Control::Diverges => Err(Stop::Diverges),
                        Control::Panic => Err(Stop::Panic),
                        Control::Break | Control::Continue => {
                            unreachable!("checked function control")
                        }
                    },
                }
            })
            .collect()
    }
}

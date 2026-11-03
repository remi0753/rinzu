//! Closed data-only contexts have no task handles to relate to heap aliases.
//! They still use the ordinary contract, including Error exits and retention
//! requirements; mutation is conservatively applied by invoke_contract.
use super::*;

// These immutable results contain neither observation handles nor scoped native
// capabilities. Their scalar alternatives can be joined at a contract boundary;
// preserve separate exits for Error/observation/retention/mutation differences.
pub(super) fn immutable_result(checked: &CheckedModule, ty: &Type) -> bool {
    fn visit(checked: &CheckedModule, ty: &Type, seen: &mut BTreeSet<String>) -> bool {
        match ty {
            Type::Unit
            | Type::Bool
            | Type::Int
            | Type::Float
            | Type::Scalar(_)
            | Type::String
            | Type::Bytes
            | Type::FixedBytes(_) => true,
            Type::Named(name) => {
                if !seen.insert(name.clone()) {
                    return true;
                }
                let result = checked.source_types.get(name).is_some_and(|schema| {
                    (schema.kind == "record"
                        || (schema.kind == "state"
                            && schema.const_fields.len() == schema.fields.len()))
                        && schema.fields.iter().all(|(_, ty)| visit(checked, ty, seen))
                });
                seen.remove(name);
                result
            }
            _ => false,
        }
    }
    matches!(ty, Type::Named(_)) && visit(checked, ty, &mut BTreeSet::new())
}

fn data_type(checked: &CheckedModule, ty: &Type, seen: &mut BTreeSet<String>) -> bool {
    match ty {
        Type::Future(_) | Type::FunctionReference(_) | Type::StaticHandler(_) => false,
        Type::List(inner) => data_type(checked, inner, seen),
        Type::Map(key, value) => data_type(checked, key, seen) && data_type(checked, value, seen),
        Type::Named(name) => {
            if !seen.insert(name.clone()) {
                return true;
            }
            let result = checked.source_types.get(name).is_some_and(|schema| {
                schema
                    .fields
                    .iter()
                    .all(|(_, ty)| data_type(checked, ty, seen))
                    && schema.variants.iter().all(|(_, fields)| {
                        fields.iter().all(|(_, ty)| data_type(checked, ty, seen))
                    })
            });
            seen.remove(name);
            result
        }
        // Every Error constructor establishes the no-retained-Future invariant.
        // Native values cannot retain managed Future handles either.
        Type::Unit
        | Type::Never
        | Type::Bool
        | Type::Int
        | Type::Float
        | Type::Scalar(_)
        | Type::String
        | Type::Bytes
        | Type::FixedBytes(_)
        | Type::Native { .. }
        | Type::Error => true,
    }
}

pub(super) fn functions(checked: &CheckedModule) -> BTreeSet<String> {
    struct Data<'a> {
        checked: &'a CheckedModule,
        only_data: bool,
    }
    impl Visitor for Data<'_> {
        fn expression(&mut self, expression: &mut Expression) {
            self.only_data &= self
                .checked
                .expression_type(expression.range)
                .is_some_and(|ty| data_type(self.checked, ty, &mut BTreeSet::new()));
        }
    }
    checked
        .functions
        .iter()
        .filter_map(|function| {
            let mut data = Data {
                checked,
                only_data: function
                    .parameter_types
                    .iter()
                    .chain(std::iter::once(&function.result_type))
                    .all(|ty| data_type(checked, ty, &mut BTreeSet::new())),
            };
            ast_walk::block(&mut function.declaration.body.clone(), &mut data);
            data.only_data.then(|| function.declaration.name.clone())
        })
        .collect()
}

impl Engine<'_> {
    pub fn can_use_data_contract(&self, name: &str) -> bool {
        // A read-only data summary has no ordered writes to collapse when its
        // caller owns Futures. Mutating summaries keep the contextual alias
        // rule: primitive writes can decide a later observation path.
        self.data_functions.contains(name)
            && (self.contracts.get(name).is_some_and(|contract| {
                !contract.mutates && !contract.mutates_async && !contract.disrupts_observers
            }) || self
                .stack
                .iter()
                .all(|caller| self.data_functions.contains(caller)))
    }
}

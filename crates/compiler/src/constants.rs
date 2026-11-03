//! Compile-time integer declarations: dependency evaluation without runtime initialization.
use crate::ast::{
    BinaryOperator, ConstantDecl, Declaration, Expression, ExpressionKind, Literal, Module,
    TypeRefKind, UnaryOperator,
};
use crate::{AbiScalar, Diagnostic, DiagnosticCode, SourceRange};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegerConstant {
    pub scalar: AbiScalar,
    pub value: i128,
}

pub fn evaluate_module_constants(
    module: &Module,
) -> Result<BTreeMap<String, IntegerConstant>, Vec<Diagnostic>> {
    let mut declared = HashMap::new();
    let mut duplicates = Vec::new();
    for declaration in &module.declarations {
        if let Some(previous) = declared.insert(declaration.name(), declaration.range()) {
            duplicates.push(
                Diagnostic::error(
                    DiagnosticCode::DuplicateName,
                    declaration.range(),
                    format!("duplicate top-level name `{}`", declaration.name()),
                    "rename or remove the duplicate",
                )
                .with_secondary(previous, "first declaration is here"),
            );
        }
    }
    if !duplicates.is_empty() {
        return Err(duplicates);
    }
    let rich = crate::value_constants::rich_names(module);
    let declarations = module
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Constant(value) if !rich.contains(&value.name) => {
                Some((value.name.clone(), value))
            }
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let mut evaluator = Evaluator {
        declarations,
        active: HashSet::new(),
        values: BTreeMap::new(),
    };
    let mut names = evaluator.declarations.keys().cloned().collect::<Vec<_>>();
    names.sort();
    let mut diagnostics = Vec::new();
    for name in names {
        if let Err(error) = evaluator.declaration(&name, module.range) {
            diagnostics.push(error);
        }
    }
    if diagnostics.is_empty() {
        Ok(evaluator.values)
    } else {
        Err(diagnostics)
    }
}

struct Evaluator<'a> {
    declarations: HashMap<String, &'a ConstantDecl>,
    active: HashSet<String>,
    values: BTreeMap<String, IntegerConstant>,
}
impl Evaluator<'_> {
    fn declaration(
        &mut self,
        name: &str,
        range: SourceRange,
    ) -> Result<IntegerConstant, Diagnostic> {
        if let Some(value) = self.values.get(name) {
            return Ok(*value);
        }
        let declaration =
            self.declarations.get(name).copied().ok_or_else(|| {
                invalid(range, format!("`{name}` is not a module integer constant"))
            })?;
        if !self.active.insert(name.into()) {
            return Err(invalid(
                range,
                format!("integer constant dependency cycle at `{name}`"),
            )
            .with_secondary(declaration.range, "constant declared here"));
        }
        let result = (|| {
            let expected = match &declaration.annotation {
                None => None,
                Some(annotation) => {
                    let TypeRefKind::Named(path) = &annotation.kind else {
                        return Err(invalid(
                            annotation.range,
                            "constant annotation must be an integer scalar",
                        ));
                    };
                    let scalar = (path.len() == 1)
                        .then(|| AbiScalar::from_name(&path[0]))
                        .flatten()
                        .filter(|scalar| scalar.integer_bounds().is_some())
                        .ok_or_else(|| {
                            invalid(
                                annotation.range,
                                "constant annotation must be an integer scalar",
                            )
                        })?;
                    Some(scalar)
                }
            };
            self.expression(&declaration.initializer, expected)
        })();
        self.active.remove(name);
        if let Ok(value) = result {
            self.values.insert(name.into(), value);
        }
        result
    }

    fn hint(&mut self, expression: &Expression) -> Result<Option<AbiScalar>, Diagnostic> {
        match &expression.kind {
            ExpressionKind::Name(path) => Ok(Some(
                self.declaration(&path.join("."), expression.range)?.scalar,
            )),
            ExpressionKind::Binary { left, right, .. } => {
                Ok(self.hint(left)?.or(self.hint(right)?))
            }
            ExpressionKind::Unary { operand, .. } => self.hint(operand),
            ExpressionKind::Call {
                target,
                arguments: crate::ast::Arguments::Positional(values),
            } if pure_integer_operation(target).is_some() => values
                .first()
                .map(|value| self.hint(value))
                .transpose()
                .map(Option::flatten),
            _ => Ok(None),
        }
    }

    fn expression(
        &mut self,
        expression: &Expression,
        expected: Option<AbiScalar>,
    ) -> Result<IntegerConstant, Diagnostic> {
        let scalar = expected
            .or(self.hint(expression)?)
            .unwrap_or(AbiScalar::I64);
        let value = match &expression.kind {
            ExpressionKind::Literal(Literal::Int(value)) => *value as i128,
            ExpressionKind::Literal(Literal::Unsigned(value)) => *value as i128,
            ExpressionKind::Literal(Literal::DecimalInteger(value)) => {
                value.parse::<i128>().map_err(|_| {
                    invalid(
                        expression.range,
                        "integer constant is outside supported integer ranges",
                    )
                })?
            }
            ExpressionKind::Name(path) => {
                let value = self.declaration(&path.join("."), expression.range)?;
                if value.scalar != scalar {
                    return Err(invalid(
                        expression.range,
                        "integer constant types differ; implicit numeric conversion is forbidden",
                    ));
                }
                return Ok(value);
            }
            ExpressionKind::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => {
                if scalar.integer_bounds().is_some_and(|(min, _)| min == 0) {
                    return Err(invalid(
                        expression.range,
                        "unsigned constants do not support unary minus",
                    ));
                }
                // A sign is part of a literal's range check (-128 is a valid CInt8 literal).
                match &operand.kind {
                    ExpressionKind::Literal(Literal::Int(value)) => -(*value as i128),
                    ExpressionKind::Literal(Literal::Unsigned(value)) => -(*value as i128),
                    _ => -self.expression(operand, Some(scalar))?.value,
                }
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let a = self.expression(left, Some(scalar))?.value;
                let b = self.expression(right, Some(scalar))?.value;
                if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder)
                    && scalar
                        .integer_bounds()
                        .is_some_and(|(min, _)| min < 0 && a == min && b == -1)
                {
                    return Err(invalid(
                        expression.range,
                        "integer constant arithmetic traps",
                    ));
                }
                match operator {
                    BinaryOperator::Add => a.checked_add(b),
                    BinaryOperator::Subtract => a.checked_sub(b),
                    BinaryOperator::Multiply => a.checked_mul(b),
                    BinaryOperator::Divide => a.checked_div(b),
                    BinaryOperator::Remainder => a.checked_rem(b),
                    _ => {
                        return Err(invalid(
                            expression.range,
                            "integer constant uses a non-integer operator",
                        ));
                    }
                }
                .ok_or_else(|| invalid(expression.range, "integer constant arithmetic traps"))?
            }
            ExpressionKind::Call {
                target,
                arguments: crate::ast::Arguments::Positional(values),
            } if pure_integer_operation(target).is_some() => {
                let op = pure_integer_operation(target).unwrap();
                if values.len() != op.arity() {
                    return Err(invalid(
                        expression.range,
                        "integer operation arity mismatch",
                    ));
                }
                let a = self.expression(&values[0], Some(scalar))?.value as u64;
                let b = if values.len() > 1 {
                    self.expression(&values[1], Some(scalar))?.value as u64
                } else {
                    0
                };
                let signed = scalar.integer_bounds().is_some_and(|(min, _)| min < 0);
                let bits = rinzu_abi::integer_operation::integer_operation(
                    op,
                    (scalar.size() * 8) as u32,
                    signed,
                    a,
                    b,
                )
                .map_err(|_| invalid(expression.range, "invalid integer constant operation"))?;
                if signed {
                    i128::from(bits as i64)
                } else {
                    i128::from(bits)
                }
            }
            _ => {
                return Err(invalid(
                    expression.range,
                    "integer constant may only use literals, integer operations and module constants",
                ));
            }
        };
        scalar.check_integer(value, expression.range)?;
        Ok(IntegerConstant { scalar, value })
    }
}
fn pure_integer_operation(target: &[String]) -> Option<crate::DataOperation> {
    use crate::DataOperation::*;
    let name = crate::core::function_name(target)?;
    crate::DataOperation::ALL.into_iter().find(|op| {
        op.name() == name
            && matches!(
                op,
                BitAnd | BitOr | BitXor | BitNot | WrappingAdd | WrappingSub | WrappingMul
            )
    })
}
fn invalid(range: SourceRange, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::InvalidConstant,
        range,
        message,
        "use an acyclic module integer constant expression",
    )
}

/// Evaluate a required size/local constant using already-resolved lexical bindings.
pub(crate) fn evaluate_integer_expression(
    expression: &Expression,
    expected: Option<AbiScalar>,
    values: &BTreeMap<String, IntegerConstant>,
) -> Result<IntegerConstant, Diagnostic> {
    Evaluator {
        declarations: HashMap::new(),
        active: HashSet::new(),
        values: values.clone(),
    }
    .expression(expression, expected)
}

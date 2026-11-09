//! Call resolution and argument checking for managed, native and built-in functions.
use super::*;

impl Checker<'_> {
    pub(super) fn receiver_method_name(
        &self,
        target: &[String],
        context: &FunctionContext<'_>,
    ) -> Option<String> {
        if target.len() < 2 {
            return None;
        }
        let mut ty = context.locals.get(&target[0])?.ty.clone();
        for field in &target[1..target.len() - 1] {
            let Type::Named(name) = &ty else {
                return None;
            };
            let field = self
                .record_declaration(name)?
                .fields
                .iter()
                .find(|f| &f.name == field)?;
            ty = self.resolve_type_readonly(&field.ty)?;
        }
        let owner = match ty {
            Type::Named(name) | Type::Native { name, .. } => name,
            _ => return None,
        };
        Some(format!("{owner}.{}", target.last()?))
    }

    fn receiver_method(
        &mut self,
        target: &[String],
        range: SourceRange,
        context: &FunctionContext<'_>,
    ) -> Result<Option<(String, Expression)>, ()> {
        let receiver_path = &target[..target.len() - 1];
        let mut receiver_range = range;
        receiver_range.end_byte = range.start_byte;
        let ty = self
            .check_name(receiver_path, receiver_range, context)
            .ok_or(())?;
        let owner = match ty {
            Type::Named(name) | Type::Native { name, .. } => name,
            _ => return Ok(None),
        };
        let method = format!("{owner}.{}", target.last().expect("receiver method name"));
        let Some(Declaration::Function(declaration)) = self.declarations.get(&method) else {
            return Ok(None);
        };
        if declaration
            .parameters
            .first()
            .is_none_or(|p| p.name != "self")
        {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                format!("static method `{method}` cannot be called through a value"),
                format!("write {method}(arguments)"),
            ));
            return Err(());
        }
        if !self.method_visible(&method, range, context) {
            return Err(());
        }
        Ok(Some((
            method,
            Expression {
                kind: ExpressionKind::Name(receiver_path.to_vec()),
                range: receiver_range,
            },
        )))
    }

    pub(super) fn check_call(
        &mut self,
        target: &[String],
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        let qualified = target.join(".");
        if target.len() > 1 && context.locals.contains_key(&target[0]) {
            match self.receiver_method(target, range, context) {
                Ok(Some((method, receiver))) => {
                    let Arguments::Positional(values) = arguments else {
                        self.type_error(range, "method calls require positional arguments");
                        return None;
                    };
                    let mut values = values.clone();
                    values.insert(0, receiver);
                    let arguments = Arguments::Positional(values);
                    let target = vec![method];
                    let result =
                        self.check_function_call(&target, &arguments, range, context, false);
                    self.method_calls
                        .insert(range, ExpressionKind::Call { target, arguments });
                    return result;
                }
                Ok(None) => {}
                Err(()) => return None,
            }
        }
        if let Some(path) = self.source_indirect_fields.get(&range) {
            context
                .facts
                .indirect_sites
                .push((path.clone(), range, vec![qualified.clone()]));
            self.indirect_fields.insert(range, path.clone());
        }
        if context.locals.contains_key(&target[0]) {
            let callee_type = self.check_name(target, range, context)?;
            let possible = self
                .reference_targets
                .get(&range)
                .cloned()
                .unwrap_or_default();
            let Type::FunctionReference(signature) = callee_type else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "local call target is not a function reference",
                    "call a function or typed function reference",
                ));
                return None;
            };
            if signature.supplant && context.direct_await_operand != Some(range) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidAwait,
                    range,
                    "supplant call must be the direct operand of await",
                    "write await reference(arguments)",
                ));
                return None;
            }
            let Arguments::Positional(values) = arguments else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "function references require positional arguments",
                    "pass arguments in signature order",
                ));
                return None;
            };
            if values.len() != signature.parameters.len() {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "function reference argument count mismatch",
                    "pass one argument per parameter",
                ));
            }
            for (index, (value, parameter)) in values.iter().zip(&signature.parameters).enumerate()
            {
                if signature.supplant {
                    self.expect_native_callback_argument(value, parameter, context);
                    if let Some(targets) = self.reference_targets.get(&value.range) {
                        context.facts.calls.extend(
                            targets
                                .iter()
                                .filter(|t| self.functions.contains_key(*t))
                                .cloned(),
                        );
                    }
                } else {
                    let site = if let [function] = possible.as_slice() {
                        Some(ExpectedErrorBound::Parameter { function, index })
                    } else {
                        None
                    };
                    if let Some(actual) =
                        self.check_expression_with_bound(value, Some(parameter), site, context)
                    {
                        self.require_same(parameter, &actual, value.range);
                    }
                }
            }
            let path = if target.len() > 1 {
                format!(
                    "{}.{}",
                    context.locals[&target[0]].ty,
                    target[1..].join(".")
                )
            } else {
                target[0].clone()
            };
            self.indirect_fields.insert(range, path.clone());
            context
                .facts
                .indirect_sites
                .push((path, range, possible.clone()));
            for name in &possible {
                if let Some(native) = self.source_signatures.get(name) {
                    context.facts.native_requirements.insert(name.clone());
                    context.facts.has_cleanup_unsafe |= native.is_async;
                    context.facts.has_native |= native.is_async || native.is_supplant;
                } else {
                    context.facts.calls.insert(name.clone());
                    if context.in_finally && !context.in_task_cleanup {
                        context.facts.cleanup_calls.insert(name.clone(), range);
                    }
                }
            }
            if context.owner_sync && matches!(*signature.result, Type::Future(_)) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidAwait,
                    range,
                    "OwnerSync handler cannot call async references",
                    "use a synchronous reference",
                ));
            }
            if signature.supplant {
                context.facts.requires_owner = true;
                context.facts.has_cleanup_unsafe = true;
                self.supplant_calls.insert(
                    range,
                    SupplantCall {
                        targets: possible,
                        reference: Some(target.join(".")),
                        signature: (*signature).clone(),
                    },
                );
            } else {
                self.indirect_calls.insert(range, possible);
                let contract = signature
                    .error_bound
                    .as_ref()
                    .and_then(|_| self.callback_call_contract(target, context));
                self.indirect_signatures.insert(
                    range,
                    IndirectSignature {
                        signature: (*signature).clone(),
                        contract,
                    },
                );
            }
            context.facts.may_fail = true;
            return Some(*signature.result);
        }
        if let [root, module, ..] = target
            && root == "rinzu"
            && matches!(module.as_str(), "list" | "map")
        {
            // A valid import links the dotted path to a canonical symbol.
            self.unknown_name(&qualified, range);
            return None;
        }
        if let Some(newline) = crate::stdio::call(target) {
            if target.len() > 1
                && !self
                    .module
                    .imports
                    .iter()
                    .any(|import| import.path == ["rinzu", "stdio"])
            {
                self.unknown_name(&qualified, range);
                return None;
            }
            let Arguments::Positional(values) = arguments else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "stdout requires one positional argument",
                    "pass one value to display",
                ));
                return None;
            };
            if values.len() != 1 {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "stdout requires one argument",
                    "pass one value to display",
                ));
                return None;
            }
            self.check_expression(&values[0], None, context)?;
            context.facts.may_fail = true;
            self.stdout_calls.insert(range, newline);
            return Some(Type::Unit);
        }
        let core = crate::core::function_name(target)
            .filter(|_| !self.declarations.contains_key(&qualified));
        if target.len() > 1
            && core.is_some()
            && !self
                .module
                .imports
                .iter()
                .any(|import| import.path == ["rinzu", "core"])
        {
            self.unknown_name(&qualified, range);
            return None;
        }
        if let Some(function) = crate::RuntimeFunction::from_path(target)
            .filter(|_| !self.declarations.contains_key(&qualified))
        {
            if function == crate::RuntimeFunction::Sleep
                && target.len() > 1
                && !self
                    .module
                    .imports
                    .iter()
                    .any(|import| import.path == ["rinzu", "timer"])
            {
                self.unknown_name(&qualified, range);
                return None;
            }
            return self.check_runtime_call(function, arguments, range, context);
        }
        if let Some(kind) = core.and_then(crate::TaskControlKind::from_name) {
            let Arguments::Positional(values) = arguments else {
                self.type_error(range, "task control requires positional arguments");
                return None;
            };
            let count = usize::from(kind != crate::TaskControlKind::Shutdown);
            if values.len() != count {
                self.type_error(range, "task control argument count is invalid");
                return None;
            }
            if let Some(value) = values.first() {
                let actual = self.check_expression(value, None, context)?;
                if kind == crate::TaskControlKind::ExitStatus {
                    if actual != Type::Int {
                        self.type_error(value.range, "process exit status requires Int");
                        return None;
                    }
                } else if !matches!(actual, Type::Future(_)) {
                    self.type_error(value.range, "task control requires a typed Future");
                    return None;
                }
            }
            if context.owner_sync {
                self.type_error(
                    range,
                    "task control requires async Main context, not OwnerSync",
                );
                return None;
            }
            context.facts.requires_owner = true;
            self.task_controls.insert(range, kind);
            return Some(Type::Bool);
        }
        if let Some(op) = crate::core::data_operation(target)
            .filter(|_| !self.declarations.contains_key(&qualified))
        {
            let Arguments::Positional(values) = arguments else {
                self.type_error(range, "data operation requires positional arguments");
                return None;
            };
            if values.len() != op.arity() {
                self.type_error(
                    range,
                    format!("{} requires {} arguments", op.name(), op.arity()),
                );
                return None;
            }
            if op == crate::DataOperation::ProcessArguments {
                context.facts.requires_owner = true;
                self.data_operations.insert(range, op);
                let name = if self
                    .declarations
                    .contains_key("rinzu.process::ArgumentSnapshot")
                {
                    "rinzu.process::ArgumentSnapshot"
                } else {
                    "ArgumentSnapshot"
                };
                return Some(Type::Named(name.into()));
            }
            if op.is_json() {
                let types = values
                    .iter()
                    .map(|value| self.check_expression(value, None, context))
                    .collect::<Option<Vec<_>>>()?;
                let result = context.result.clone();
                if !crate::json::signature(op, &types, &result) {
                    self.type_error(range, "invalid internal JSON operation signature");
                    return None;
                }
                if matches!(
                    op,
                    crate::DataOperation::JsonDecode | crate::DataOperation::JsonDecodeWithLimits
                ) && let Some(path) = self.unsupported_json_type(&result)
                {
                    self.type_error(range, format!("unsupported JSON target type at {path}"));
                    return None;
                }
                self.data_operations.insert(range, op);
                context.facts.may_fail = true;
                return Some(result);
            }
            let input = self.check_expression(&values[0], None, context)?;
            let Some((parameters, result)) = crate::core::data_signature(op, &input) else {
                self.type_error(
                    values[0].range,
                    format!("{} does not accept `{input}`", op.name()),
                );
                return None;
            };
            for (value, expected) in values.iter().skip(1).zip(parameters.iter().skip(1)) {
                self.expect_expression(value, expected, context);
            }
            use crate::DataOperation::*;
            if op == MapEntries
                && let Type::Map(key, value) = &input
            {
                self.intern_record(crate::collections::entry_fields(key, value), range)?;
            }
            let payload = match (op, &input) {
                (ListGet, Type::List(t)) => Some(t.as_ref()),
                (ListSet | ListRemove, _) => Some(&input),
                (StringGet | StringSlice, _) => Some(&Type::String),
                (StringFind | BytesFindAny | BytesFindNotAny, _) => Some(&Type::Int),
                (BytesSlice, _) => Some(&Type::Bytes),
                (BytesGet, _) => Some(&Type::Scalar(AbiScalar::U8)),
                (ErrorCause, _) => Some(&Type::Error),
                (ErrorSourceFile, _) => Some(&Type::Scalar(AbiScalar::U64)),
                (ErrorSourceStart | ErrorSourceEnd | ErrorNativeStatus, _) => Some(&Type::Int),
                (MapGet, Type::Map(_, v)) => Some(v.as_ref()),
                _ => None,
            };
            if let Some(payload) = payload {
                self.ensure_option(payload, range, &context.function.name)?;
            }
            self.data_operations.insert(range, op);
            context.facts.may_fail = true;
            return Some(result);
        }
        if matches!(core, Some("utf8ByteLength" | "byteLength")) {
            let Arguments::Positional(values) = arguments else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "core length functions require one positional argument",
                    "pass one String or Bytes value",
                ));
                return None;
            };
            if values.len() != 1 {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "core length functions require one argument",
                    "pass one String or Bytes value",
                ));
                return None;
            }
            let actual = self.check_expression(&values[0], None, context)?;
            let kind = if core == Some("utf8ByteLength") && actual == Type::String {
                crate::CoreLengthKind::Utf8
            } else if core == Some("byteLength")
                && matches!(actual, Type::FixedBytes(_) | Type::Bytes)
            {
                crate::CoreLengthKind::Bytes
            } else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "core length argument has an incompatible type",
                    "use utf8ByteLength for String and byteLength for Bytes(N)/ByteArray",
                ));
                return None;
            };
            self.core_lengths.insert(range, kind);
            return Some(Type::from_scalar(AbiScalar::U64));
        }
        if target.len() == 1
            && let Some(scalar) =
                AbiScalar::from_name(&target[0]).filter(|scalar| *scalar != AbiScalar::Bool)
        {
            let Arguments::Positional(values) = arguments else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "numeric conversion requires one positional argument",
                    "pass one numeric value",
                ));
                return None;
            };
            if values.len() != 1 {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "numeric conversion requires one argument",
                    "pass one numeric value",
                ));
                return None;
            }
            let source = self.check_expression(&values[0], None, context)?;
            if !source.is_numeric() {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    values[0].range,
                    "numeric conversion requires a numeric operand",
                    "use an integer or floating value",
                ));
                return None;
            }
            let result = Type::from_scalar(scalar);
            context.facts.may_fail |= source != result;
            return Some(result);
        }

        if let Some(signature) = self.source_signatures.get(&qualified).cloned() {
            if signature.is_supplant && context.direct_await_operand != Some(range) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidAwait,
                    range,
                    "supplant call must be the direct operand of await",
                    "write await nativeFunction(arguments)",
                ));
                return None;
            }
            let native = NativeFunction {
                parameters: signature.parameters.iter().map(source_abi_type).collect(),
                result: if signature.is_async {
                    Type::Future(Box::new(source_abi_type(&signature.result)))
                } else {
                    source_abi_type(&signature.result)
                },
                call_class: NativeCallClass::NativeSafe,
                metadata_complete: true,
                cleanup_safe: !signature.is_async && !signature.is_supplant,
            };
            let result = self.check_native_call(&qualified, &native, arguments, range, context);
            if let Arguments::Positional(values) = arguments {
                for (argument, parameter) in values.iter().zip(&signature.parameters) {
                    if matches!(parameter, crate::SourceAbiType::Callback { .. }) {
                        let targets = self
                            .reference_targets
                            .get(&argument.range)
                            .cloned()
                            .unwrap_or_default();
                        for target in targets {
                            if let Some(native) = self.source_signatures.get(&target) {
                                context.facts.native_requirements.insert(target);
                                context.facts.has_cleanup_unsafe |=
                                    native.is_async || native.is_supplant;
                            } else {
                                context.facts.calls.insert(target.clone());
                                if context.in_finally && !context.in_task_cleanup {
                                    context.facts.cleanup_calls.insert(target, argument.range);
                                }
                            }
                        }
                    }
                }
            }
            self.native_calls.remove(&range);
            if signature.is_supplant {
                context.facts.requires_owner = true;
                self.supplant_calls.insert(
                    range,
                    SupplantCall {
                        targets: vec![qualified.clone()],
                        reference: None,
                        signature: self
                            .reference_signature(&qualified)
                            .expect("source signature"),
                    },
                );
            } else {
                self.source_native_calls.insert(range, signature);
            }
            // Ordinary source native calls execute on the caller thread. They do not
            // inherit the legacy SDK blanket Worker prohibition.
            context.facts.has_native = context.facts.native_requirements.iter().any(|name| {
                self.source_signatures
                    .get(name)
                    .is_none_or(|signature| signature.is_async || signature.is_supplant)
            });
            return result;
        }
        if let Some(native) = self.options.native_functions.get(&qualified).cloned() {
            return self.check_native_call(&qualified, &native, arguments, range, context);
        }
        if self.functions.contains_key(&qualified) {
            let needs_normalization = target.len() > 1;
            let target = vec![qualified];
            let result = self.check_function_call(&target, arguments, range, context, false);
            if needs_normalization && matches!(arguments, Arguments::Positional(_)) {
                self.method_calls.insert(
                    range,
                    ExpressionKind::Call {
                        target,
                        arguments: arguments.clone(),
                    },
                );
            }
            return result;
        }
        if target.len() == 1
            && let Some(Declaration::Error(error)) = self.declarations.get(&target[0]).cloned()
        {
            return self.check_error_constructor(&error, arguments, range, context);
        }
        if target.len() == 1
            && let Some(record) = self.record_declaration(&target[0]).cloned()
        {
            if record.fields.iter().any(|f| f.is_private)
                && !self.owner_access(&record.name, context)
            {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::UnknownName,
                    range,
                    format!(
                        "class `{}` has a private constructor because it contains private fields",
                        record.name
                    ),
                    "construct this class through a public factory method",
                ));
                return None;
            }
            context.facts.may_fail = true;
            return self
                .check_named_arguments(&record.fields, arguments, range, context)
                .then(|| Type::Named(record.name.clone()));
        }
        if target.len() == 2
            && let Some(Declaration::Enum(enum_decl)) = self.declarations.get(&target[0]).cloned()
            && let Some(variant) = enum_decl
                .variants
                .iter()
                .find(|value| value.name == target[1])
        {
            context.facts.may_fail = true;
            return self
                .check_named_arguments(&variant.fields, arguments, range, context)
                .then(|| Type::Named(enum_decl.name.clone()));
        }
        if target.first().is_some_and(|name| name == "rinzu") {
            context.facts.has_native = true;
            context.facts.native_requirements.insert(target.join("."));
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::UnsupportedSafeFfi,
                range,
                format!(
                    "safe binding `{}` is unavailable for this target",
                    target.join(".")
                ),
                "install or generate a binding with complete safety metadata",
            ));
        } else {
            self.unknown_name(&target.join("."), range);
        }
        None
    }

    fn check_runtime_call(
        &mut self,
        function: crate::RuntimeFunction,
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        use crate::{RuntimeFunction as R, SourceAbiType as A};
        let Arguments::Positional(values) = arguments else {
            self.type_error(
                range,
                "runtime library functions require positional arguments",
            );
            return None;
        };
        let [value] = values.as_slice() else {
            self.type_error(
                range,
                "runtime library function requires exactly one argument",
            );
            return None;
        };
        let expected = match function {
            R::Sleep => Some(Type::Int),
            R::ReadResourceSize => Some(Type::String),
            R::NativeCompleted | R::NativeTransferred => None,
        };
        let actual = self.check_expression(value, expected.as_ref(), context)?;
        let parameter = if let Some(expected) = expected {
            self.require_same(&expected, &actual, value.range);
            if function == R::Sleep {
                A::Scalar(AbiScalar::I64)
            } else {
                A::String
            }
        } else if let Type::Native {
            name,
            call_only: false,
        } = actual
        {
            A::Opaque(name)
        } else {
            self.type_error(
                value.range,
                "native completion requires an opaque protocol handle",
            );
            return None;
        };
        if context.owner_sync {
            self.type_error(
                range,
                "async library functions cannot be called from OwnerSync",
            );
        }
        if context.in_finally && !context.in_task_cleanup {
            self.invalid_finally(range, "this async library call is not CleanupSafe");
        }
        let result = match function {
            R::Sleep | R::NativeCompleted => A::Unit,
            R::ReadResourceSize | R::NativeTransferred => A::Scalar(AbiScalar::I64),
        };
        // Completion functions are generic over nominal native handle types.
        let name = if matches!(parameter, A::Opaque(_)) {
            format!("{}<{}>", function.name(), parameter.canonical_name())
        } else {
            function.name().to_owned()
        };
        let signature = crate::SourceNativeSignature {
            name: name.clone(),
            binding: crate::ast::NativeBinding::Runtime(function),
            is_supplant: false,
            is_async: true,
            parameters: vec![parameter],
            result,
        };
        let ty = Type::Future(Box::new(source_abi_type(&signature.result)));
        self.source_signatures
            .insert(name.clone(), signature.clone());
        self.source_native_calls.insert(range, signature);
        context.facts.has_native = true;
        context.facts.has_cleanup_unsafe = true;
        context.facts.may_fail = true;
        context.facts.native_requirements.insert(name);
        Some(ty)
    }

    fn check_native_call(
        &mut self,
        name: &str,
        native: &NativeFunction,
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        if !native.metadata_complete {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::UnsupportedSafeFfi,
                range,
                format!("safe binding `{name}` has incomplete metadata"),
                "complete call class, thread, blocking, callback, lifetime, and exception metadata",
            ));
            return None;
        }
        if native.call_class == NativeCallClass::OwnerThreadSync && !context.owner_sync {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::UnsupportedSafeFfi,
                range,
                format!("native call `{name}` requires an OwnerSync context"),
                "invoke it from the generated owner-thread callback context",
            ));
        }
        let Arguments::Positional(arguments) = arguments else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                "native function calls use positional arguments",
                "pass positional arguments in binding declaration order",
            ));
            return None;
        };
        if native.parameters.len() != arguments.len() {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                format!(
                    "native function `{name}` expects {} arguments but received {}",
                    native.parameters.len(),
                    arguments.len()
                ),
                "pass exactly one argument for each generated parameter",
            ));
        }
        for (argument, parameter) in arguments.iter().zip(&native.parameters) {
            if let Type::StaticHandler(signature) = parameter {
                self.check_static_handler_reference(argument, signature);
            } else {
                self.expect_native_callback_argument(argument, parameter, context);
            }
        }
        if context.in_finally && !context.in_task_cleanup && !native.cleanup_safe {
            self.invalid_finally(range, "this native call is not CleanupSafe");
        }
        context.facts.has_native = true;
        context.facts.has_cleanup_unsafe |= !native.cleanup_safe;
        context.facts.may_fail = true;
        context.facts.native_requirements.insert(name.to_owned());
        self.native_calls.insert(range, native.call_class);
        Some(native.result.clone())
    }

    fn expect_native_callback_argument(
        &mut self,
        argument: &Expression,
        parameter: &Type,
        context: &mut FunctionContext<'_>,
    ) {
        if self.closure_conversion && matches!(parameter, Type::FunctionReference(_)) {
            if let Some(actual) = self.check_expression(argument, Some(parameter), context) {
                if let (Type::Named(name), Type::FunctionReference(expected)) = (&actual, parameter)
                    && name.starts_with("$rinzu_closure_")
                {
                    let count = self
                        .declarations
                        .get(&format!("{name}_env"))
                        .and_then(|d| {
                            if let Declaration::Enum(e) = d {
                                Some(e.variants.len())
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    let mut parameters = vec![actual.clone()];
                    parameters.extend(expected.parameters.clone());
                    let targets = (0..count)
                        .map(|i| format!("{name}_callback_{i}"))
                        .collect::<Vec<_>>();
                    if targets.is_empty()
                        || targets.iter().any(|target| {
                            self.functions.get(target).is_none_or(|s| {
                                s.parameters != parameters
                                    || s.result != *expected.result
                                    || s.is_async
                            })
                        })
                    {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::TypeMismatch,
                            argument.range,
                            "captured native callback lost its typed adapter",
                            "use a Main callback returning Scalar or Unit",
                        ));
                    } else {
                        self.native_captures.insert(argument.range, actual);
                        self.reference_targets.insert(argument.range, targets);
                        context.facts.requires_owner = true;
                    }
                } else {
                    self.require_same(parameter, &actual, argument.range);
                }
            }
        } else {
            self.expect_expression(argument, parameter, context);
        }
    }

    fn check_static_handler_reference(
        &mut self,
        expression: &Expression,
        expected: &StaticHandlerType,
    ) {
        let ExpressionKind::Name(path) = &expression.kind else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidHandlerReference,
                expression.range,
                "generated callback parameter requires a top-level function name",
                "pass a capture-free top-level function directly at this call site",
            ));
            return;
        };
        if path.len() != 1 {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidHandlerReference,
                expression.range,
                "callback handler must resolve to one imported or local top-level function",
                "pass the function name directly without storing it in another value",
            ));
            return;
        }
        let name = &path[0];
        if matches!(self.declarations.get(name), Some(Declaration::Function(function)) if function.fallback.is_some())
        {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::UnsupportedSafeFfi,
                expression.range,
                "callback fallback requires the source v2 callback ABI",
                "use a source native callback declaration",
            ));
            return;
        }
        let Some(actual) = self.functions.get(name) else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidHandlerReference,
                expression.range,
                format!("`{name}` is not a top-level function"),
                "declare a capture-free top-level function with the required signature",
            ));
            return;
        };
        let signature_matches = actual.parameters == expected.parameters
            && actual.result == *expected.result
            && actual.is_async == expected.is_async;
        let owner_matches = !expected.owner_sync || self.options.owner_sync_handlers.contains(name);
        if !signature_matches || !owner_matches {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidHandlerReference,
                expression.range,
                format!("handler `{name}` does not match the generated callback contract"),
                "match the generated parameter/result/async signature and callback class",
            ));
            return;
        }
        self.expression_types.insert(
            expression.range,
            Type::StaticHandler(Box::new(expected.clone())),
        );
    }

    pub(super) fn check_function_call(
        &mut self,
        target: &[String],
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
        from_spawn: bool,
    ) -> Option<Type> {
        if from_spawn
            && target
                .first()
                .is_some_and(|first| context.locals.contains_key(first))
        {
            self.diagnostics.push(local_spawn_error(range));
            return None;
        }
        let name = target.join(".");
        let Some(signature) = self.functions.get(&name).cloned() else {
            self.unknown_name(&name, range);
            return None;
        };
        if !self.method_visible(&name, range, context) {
            return None;
        }
        if from_spawn && target.len() > 1 {
            self.method_calls.insert(
                range,
                ExpressionKind::Spawn {
                    target: vec![name.clone()],
                    arguments: arguments.clone(),
                },
            );
        }
        let Arguments::Positional(arguments) = arguments else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                "function calls use positional arguments",
                "remove argument labels or call a record/enum constructor",
            ));
            return None;
        };
        if signature.parameters.len() != arguments.len() {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                format!(
                    "function `{name}` expects {} arguments but received {}",
                    signature.parameters.len(),
                    arguments.len()
                ),
                "pass exactly one positional argument for each parameter",
            ));
        }
        for (index, (argument, parameter)) in
            arguments.iter().zip(&signature.parameters).enumerate()
        {
            if let Some(actual) = self.check_expression_with_bound(
                argument,
                Some(parameter),
                Some(ExpectedErrorBound::Parameter {
                    function: &name,
                    index,
                }),
                context,
            ) {
                self.require_same(parameter, &actual, argument.range);
                if from_spawn && !self.is_transferable(&actual, &mut HashSet::new()) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidSpawn,
                        argument.range,
                        format!(
                            "spawn argument type `{actual}` is not Transferable; field path: {}",
                            self.sealed_failure_path(&actual, "Transferable")
                        ),
                        "pass only immutable Transferable values to `spawn`",
                    ));
                }
            }
        }
        if from_spawn {
            if signature.is_async {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidSpawn,
                    range,
                    format!("async function `{name}` cannot be a spawn callee"),
                    "spawn a synchronous WorkerSafe function",
                ));
            } else {
                context.facts.spawn_calls.push((name.clone(), range));
            }
            Some(signature.result)
        } else {
            if context.owner_sync && signature.is_async {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidAwait,
                    range,
                    format!("OwnerSync handler cannot call async function `{name}`"),
                    "call only synchronous functions from OwnerSync",
                ));
            }
            if signature
                .parameters
                .iter()
                .any(|ty| self.contains_call_only_type(ty, &mut HashSet::new()))
            {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::CallOnlyEscape,
                    range,
                    "CallOnly value cannot be passed to a managed function",
                    "pass it only to an approved synchronous native call",
                ));
            }
            context.facts.calls.insert(name.clone());
            if context.in_finally && !context.in_task_cleanup {
                context.facts.cleanup_calls.insert(name.clone(), range);
            }
            context.facts.may_fail = true;
            if signature.is_async {
                Some(Type::Future(Box::new(signature.result)))
            } else {
                Some(signature.result)
            }
        }
    }

    fn check_named_arguments(
        &mut self,
        fields: &[crate::ast::FieldDecl],
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> bool {
        let Arguments::Named(arguments) = arguments else {
            if fields.is_empty()
                && matches!(arguments, Arguments::Positional(values) if values.is_empty())
            {
                return true;
            }
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                "record and enum constructors use named arguments",
                "label every field as `name: value`",
            ));
            return false;
        };
        let mut seen = HashSet::new();
        for (name, expression, argument_range) in arguments {
            if !seen.insert(name) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::DuplicateName,
                    *argument_range,
                    format!("constructor argument `{name}` appears more than once"),
                    "provide each field exactly once",
                ));
                continue;
            }
            let Some(field) = fields.iter().find(|field| &field.name == name) else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::UnknownName,
                    *argument_range,
                    format!("constructor has no field `{name}`"),
                    "use a declared field name",
                ));
                continue;
            };
            if let Some(expected) = self.resolve_type(&field.ty) {
                self.expect_expression(expression, &expected, context);
            }
        }
        let missing: Vec<_> = fields
            .iter()
            .filter(|field| !seen.contains(&field.name))
            .map(|field| field.name.as_str())
            .collect();
        if !missing.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                format!("constructor is missing fields: {}", missing.join(", ")),
                "provide every field exactly once",
            ));
        }
        missing.is_empty() && seen.len() == fields.len()
    }
}

//! Lower source-native invocations with their argument and resource lifetimes.
use super::*;

impl Builder<'_> {
    pub(super) fn lower_source_native_call(
        &mut self,
        signature: &crate::SourceNativeSignature,
        source_arguments: &Arguments,
        arguments: Vec<ValueId>,
        ty: Type,
        range: SourceRange,
    ) -> ValueId {
        self.emit_poll(PollReason::NativeCall, range);
        if signature.is_async {
            return self.emit_value(
                OperationKind::SourceNativeCall {
                    call: None,
                    signature: signature.clone(),
                    arguments,
                    failure: self.failure,
                    error: self.error_edge(),
                    cleanup_chain: self.cleanup_chain(),
                },
                ty,
                range,
            );
        }
        let call = NativeCallId(self.next_native_call);
        self.next_native_call += 1;
        let Arguments::Positional(source_arguments) = source_arguments else {
            unreachable!("checked positional call")
        };
        let callbacks = self.callback_arguments(
            source_arguments,
            signature
                .parameters
                .iter()
                .enumerate()
                .filter_map(|(index, parameter)| {
                    matches!(parameter, crate::SourceAbiType::Callback { .. }).then_some(index)
                }),
        );
        self.emit(
            OperationKind::PrepareNativeCall {
                call,
                signature: signature.clone(),
                arguments: arguments.clone(),
                retained: arguments.clone(),
                callbacks,
                failure: self.failure,
                error: self.error_edge(),
                cleanup_chain: self.cleanup_chain(),
            },
            None,
            range,
        );
        self.active_native_calls.push(call);
        self.emit(
            OperationKind::SourceNativeCall {
                call: Some(call),
                signature: signature.clone(),
                arguments,
                failure: self.failure,
                error: ErrorEdge {
                    target: self.failure,
                    cleanup_chain: self.cleanup_chain(),
                    release_scopes: self.scope_stack.iter().rev().copied().collect(),
                    release_native_calls: vec![call],
                },
                cleanup_chain: self.cleanup_chain(),
            },
            None,
            range,
        );
        self.emit(OperationKind::CloseNativeCall { call }, None, range);
        self.emit(
            OperationKind::SettleNativeAdmissions {
                call,
                origin: ResumeDomain::Origin,
            },
            None,
            range,
        );
        let value = self.emit_value(
            OperationKind::ReadNativeResult {
                call,
                failure: self.failure,
                error: self.error_edge(),
                cleanup_chain: self.cleanup_chain(),
            },
            ty,
            range,
        );
        self.emit(OperationKind::ReleaseNativeCall { call }, None, range);
        self.active_native_calls.pop();
        value
    }

    pub(super) fn callback_arguments(
        &self,
        arguments: &[Expression],
        callbacks: impl Iterator<Item = usize>,
    ) -> Vec<CallbackArgumentPlan> {
        callbacks
            .map(|index| {
                let range = &arguments[index].range;
                CallbackArgumentPlan {
                    index,
                    capture: self.checked.native_captures.get(range).cloned(),
                    targets: self
                        .checked
                        .reference_targets
                        .get(range)
                        .cloned()
                        .unwrap_or_default(),
                }
            })
            .collect()
    }
}

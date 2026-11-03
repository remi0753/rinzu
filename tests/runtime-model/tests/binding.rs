use rinzu_runtime_model::binding::{
    AdapterDescription, BodyOperation, CallbackClass, CompileError, CustomCapabilities,
    GeneratedAdapter, GenerationOutcome, LogicalFrame, NativeCallMetadata, RejectionReason,
    SdkDescription, generate_binding, synchronous_reentry_stack, validate_callback_body,
};

#[test]
fn logical_stack_and_compile_time_callback_restrictions_are_explicit() {
    let stack = synchronous_reentry_stack("App.onClick", "Widget.perform", "Delegate.didRun");
    assert_eq!(
        stack.frames,
        [
            LogicalFrame::Managed("App.onClick"),
            LogicalFrame::Native("Widget.perform"),
            LogicalFrame::OwnerSync("Delegate.didRun"),
        ]
    );
    assert_eq!(
        validate_callback_body(
            CallbackClass::OwnerSync,
            &[BodyOperation::ReadCallOnly, BodyOperation::EscapeCallOnly]
        ),
        Err(CompileError::CallOnlyEscape)
    );
    assert_eq!(
        validate_callback_body(
            CallbackClass::OwnerSync,
            &[BodyOperation::ReadCallOnly, BodyOperation::Await]
        ),
        Err(CompileError::AwaitInSynchronousCallback)
    );
    assert!(
        validate_callback_body(
            CallbackClass::OwnerAsync,
            &[BodyOperation::Await, BodyOperation::Return]
        )
        .is_ok()
    );
}

#[test]
fn standard_and_custom_sdk_descriptions_require_different_contracts() {
    let queue_sdk = SdkDescription {
        name: "QueueUiKit",
        call: NativeCallMetadata {
            may_synchronously_callback: Some(true),
            blocking: Some(false),
        },
        adapter: AdapterDescription::Standard {
            post_primitive: Some("dispatch_main"),
        },
    };
    let game_sdk = SdkDescription {
        name: "FrameGameKit",
        call: NativeCallMetadata {
            may_synchronously_callback: Some(false),
            blocking: Some(true),
        },
        adapter: AdapterDescription::Custom {
            post_primitive: Some("request_tick"),
            prepare: Some("install_runtime_hook"),
            deactivate: Some("remove_runtime_hook"),
            capabilities: Some(CustomCapabilities {
                owner_thread_known: true,
                nested_modes_known: true,
            }),
        },
    };
    assert!(matches!(
        generate_binding(queue_sdk),
        GenerationOutcome::Safe(binding)
            if binding.adapter == GeneratedAdapter::StandardDefaults
    ));
    assert!(matches!(
        generate_binding(game_sdk),
        GenerationOutcome::Safe(binding)
            if binding.adapter == GeneratedAdapter::CustomContract
    ));

    let missing_custom_contract = SdkDescription {
        adapter: AdapterDescription::Custom {
            post_primitive: Some("request_tick"),
            prepare: None,
            deactivate: Some("remove_runtime_hook"),
            capabilities: None,
        },
        ..game_sdk
    };
    assert_eq!(
        generate_binding(missing_custom_contract),
        GenerationOutcome::RawOnly(RejectionReason::MissingCustomPrepare)
    );

    let unknown_metadata = SdkDescription {
        call: NativeCallMetadata {
            may_synchronously_callback: None,
            blocking: Some(false),
        },
        ..queue_sdk
    };
    assert_eq!(
        generate_binding(unknown_metadata),
        GenerationOutcome::RawOnly(RejectionReason::UnknownSynchronousReentry)
    );
}

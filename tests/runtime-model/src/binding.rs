//! Binding-generator safety defaults and synchronous-reentry diagnostics.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallbackClass {
    OwnerSync,
    OwnerAsync,
    ForeignOneWay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyOperation {
    ReadCallOnly,
    EscapeCallOnly,
    Await,
    Return,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompileError {
    CallOnlyEscape,
    AwaitInSynchronousCallback,
}

/// Check callback restrictions before lowering to native ABI stubs.
///
/// # Errors
///
/// Rejects escape of a call-only native argument and suspension in an
/// `OwnerSync` callback.
pub fn validate_callback_body(
    class: CallbackClass,
    operations: &[BodyOperation],
) -> Result<(), CompileError> {
    if operations.contains(&BodyOperation::EscapeCallOnly) {
        return Err(CompileError::CallOnlyEscape);
    }
    if class == CallbackClass::OwnerSync && operations.contains(&BodyOperation::Await) {
        return Err(CompileError::AwaitInSynchronousCallback);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogicalFrame {
    Managed(&'static str),
    Native(&'static str),
    OwnerSync(&'static str),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalStack {
    pub frames: Vec<LogicalFrame>,
}

#[must_use]
pub fn synchronous_reentry_stack(
    managed: &'static str,
    native: &'static str,
    callback: &'static str,
) -> LogicalStack {
    LogicalStack {
        frames: vec![
            LogicalFrame::Managed(managed),
            LogicalFrame::Native(native),
            LogicalFrame::OwnerSync(callback),
        ],
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCallMetadata {
    pub may_synchronously_callback: Option<bool>,
    pub blocking: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CustomCapabilities {
    pub owner_thread_known: bool,
    pub nested_modes_known: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterDescription {
    Standard {
        post_primitive: Option<&'static str>,
    },
    Custom {
        post_primitive: Option<&'static str>,
        prepare: Option<&'static str>,
        deactivate: Option<&'static str>,
        capabilities: Option<CustomCapabilities>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SdkDescription {
    pub name: &'static str,
    pub call: NativeCallMetadata,
    pub adapter: AdapterDescription,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeneratedAdapter {
    StandardDefaults,
    CustomContract,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SafeBinding {
    pub sdk_name: &'static str,
    pub adapter: GeneratedAdapter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RejectionReason {
    UnknownSynchronousReentry,
    UnknownBlockingBehavior,
    MissingPostPrimitive,
    MissingCustomPrepare,
    MissingCustomDeactivate,
    MissingCustomCapabilities,
    UnknownOwnerThread,
    UnknownNestedModes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationOutcome {
    Safe(SafeBinding),
    RawOnly(RejectionReason),
}

#[must_use]
pub fn generate_binding(sdk: SdkDescription) -> GenerationOutcome {
    if sdk.call.may_synchronously_callback.is_none() {
        return GenerationOutcome::RawOnly(RejectionReason::UnknownSynchronousReentry);
    }
    if sdk.call.blocking.is_none() {
        return GenerationOutcome::RawOnly(RejectionReason::UnknownBlockingBehavior);
    }
    match sdk.adapter {
        AdapterDescription::Standard { post_primitive } => {
            if post_primitive.is_none() {
                GenerationOutcome::RawOnly(RejectionReason::MissingPostPrimitive)
            } else {
                GenerationOutcome::Safe(SafeBinding {
                    sdk_name: sdk.name,
                    adapter: GeneratedAdapter::StandardDefaults,
                })
            }
        }
        AdapterDescription::Custom {
            post_primitive,
            prepare,
            deactivate,
            capabilities,
        } => {
            if post_primitive.is_none() {
                return GenerationOutcome::RawOnly(RejectionReason::MissingPostPrimitive);
            }
            if prepare.is_none() {
                return GenerationOutcome::RawOnly(RejectionReason::MissingCustomPrepare);
            }
            if deactivate.is_none() {
                return GenerationOutcome::RawOnly(RejectionReason::MissingCustomDeactivate);
            }
            let Some(capabilities) = capabilities else {
                return GenerationOutcome::RawOnly(RejectionReason::MissingCustomCapabilities);
            };
            if !capabilities.owner_thread_known {
                return GenerationOutcome::RawOnly(RejectionReason::UnknownOwnerThread);
            }
            if !capabilities.nested_modes_known {
                return GenerationOutcome::RawOnly(RejectionReason::UnknownNestedModes);
            }
            GenerationOutcome::Safe(SafeBinding {
                sdk_name: sdk.name,
                adapter: GeneratedAdapter::CustomContract,
            })
        }
    }
}

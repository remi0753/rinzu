//! Binding metadata validation and generated-binding boundary.

pub mod compilation;
pub mod consumption;
mod generator;
mod metadata;
pub mod native_binding;
pub mod source_package;

pub use generator::{GeneratedBinding, GenerationError, generate_binding};
pub use metadata::{
    ArgumentLifetime, BindingMetadata, BindingMetadataError, BlockingBehavior,
    CallbackArgumentMetadata, CallbackClass, CallbackMetadata, DeliveryPolicy, FunctionMetadata,
    NestedDelivery, ParameterMetadata, ShutdownPolicy, SynchronousReentry, ThreadRequirement,
    UnregisterPolicy, ValidatedMetadata, validate_metadata,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BindingAbi {
    pub version: u32,
}

#[must_use]
pub const fn binding_abi() -> BindingAbi {
    BindingAbi {
        version: rinzu_abi::ABI_VERSION,
    }
}

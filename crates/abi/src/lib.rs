//! Versioned native boundary shared by the Rinzu runtime and generated bindings.

use core::ffi::{c_char, c_void};
pub mod data_operation;
pub mod integer_operation;

pub const ABI_VERSION_MAJOR: u32 = 1;
pub const ABI_VERSION_MINOR: u32 = 1;
pub const ABI_VERSION_PATCH: u32 = 0;
pub const ABI_VERSION: u32 =
    (ABI_VERSION_MAJOR << 24) | (ABI_VERSION_MINOR << 12) | ABI_VERSION_PATCH;

pub type Status = i32;

pub const STATUS_OK: Status = 0;
pub const STATUS_INVALID_ARGUMENT: Status = 1;
pub const STATUS_ABI_MISMATCH: Status = 2;
pub const STATUS_OUT_OF_MEMORY: Status = 3;
pub const STATUS_WRONG_THREAD: Status = 4;
pub const STATUS_UNSUPPORTED: Status = 5;
pub const STATUS_PANIC: Status = 6;
pub const STATUS_QUEUE_FULL: Status = 7;

pub const HOST_POST_ACCEPTED: i32 = 0;
pub const HOST_POST_CLOSED: i32 = 1;
pub const HOST_POST_FAILURE: i32 = 2;

pub const HOST_NESTED_ALL_MODES: u32 = 0;
pub const HOST_NESTED_DECLARED_MODES: u32 = 1;
pub const HOST_NESTED_UNSUPPORTED: u32 = 2;

pub const HOST_RETURN_RESUME_MANAGED: u32 = 0;

pub type HostOwnerTurnCallback = unsafe extern "C" fn(context: *mut c_void, generation: u64);
pub type HostPrepareCallback = unsafe extern "C" fn(
    context: *mut c_void,
    owner_turn: HostOwnerTurnCallback,
    owner_context: *mut c_void,
    generation: u64,
    out_ready_token: *mut *mut c_void,
) -> Status;
pub type HostPostOwnerTurnCallback =
    unsafe extern "C" fn(context: *mut c_void, ready_token: *mut c_void, generation: u64) -> i32;
pub type HostDeactivateCallback =
    unsafe extern "C" fn(context: *mut c_void, ready_token: *mut c_void, generation: u64);
pub type HostWorkCallback = unsafe extern "C" fn(context: *mut c_void);

pub const NATIVE_PACKAGE_ENTRY_V1: &str = "rinzu_native_package_v1";

pub type NativeInvokeHandlerV1 =
    unsafe extern "C" fn(runtime_context: *mut c_void, handler: u64) -> i64;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NativeInvocationContextV1 {
    pub struct_size: u32,
    pub abi_version: u32,
    pub runtime_context: *mut c_void,
    pub invoke_handler: Option<NativeInvokeHandlerV1>,
}

pub type NativeFunctionV1 = unsafe extern "C" fn(
    invocation: *const NativeInvocationContextV1,
    arguments: *const i64,
    argument_count: usize,
    out_result: *mut i64,
) -> Status;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NativeImportV1 {
    pub import_key: u64,
    pub canonical_key: *const c_char,
    pub signature: *const c_char,
    pub address: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NativePackageDescriptorV1 {
    pub struct_size: u32,
    pub abi_version: u32,
    pub package_name: *const c_char,
    pub metadata_digest: *const u8,
    pub metadata_digest_size: u32,
    pub reserved: u32,
    pub import_count: usize,
    pub imports: *const NativeImportV1,
}

pub type NativePackageEntryV1 = unsafe extern "C" fn() -> *const NativePackageDescriptorV1;

/// Source-driven ABI is separate from the legacy uniform invocation ABI.
pub const SOURCE_PACKAGE_ENTRY_V2: &str = "rinzu_source_package_v2";
pub const SOURCE_PACKAGE_MAJOR: u16 = 2;
pub const SOURCE_PACKAGE_MINOR: u16 = 6;
/// Shared native records and call-scoped ordinary record context tokens.
pub const NATIVE_DATA_CONTRACT_VERSION: u32 = 1;
pub const CALLBACK_RULE_VERSION: u32 = 1;
pub const TEMPLATE_LAYOUT_VERSION: u32 = 3;
pub const CLOSED_SERVICE_REGISTRY_VERSION: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SourceImportV2 {
    pub import_key: [u8; 32],
    pub symbol: *const c_char,
    /// Untyped address: only validated signature-specific glue may call it.
    pub address: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SourcePackageDescriptorV2 {
    pub struct_size: u32,
    pub major: u16,
    pub minor: u16,
    pub descriptor: *const u8,
    pub descriptor_len: usize,
    pub import_count: usize,
    pub imports: *const SourceImportV2,
}

pub type SourcePackageEntryV2 = unsafe extern "C" fn() -> *const SourcePackageDescriptorV2;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HostLoopProfileV1 {
    pub struct_size: u32,
    pub abi_version: u32,
    pub context: *mut c_void,
    pub prepare: Option<HostPrepareCallback>,
    pub post_owner_turn: Option<HostPostOwnerTurnCallback>,
    pub deactivate: Option<HostDeactivateCallback>,
    pub nested_delivery: u32,
    pub return_policy: u32,
    pub turn_budget: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeOptions {
    pub struct_size: u32,
    pub abi_version: u32,
    pub flags: u64,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            abi_version: ABI_VERSION,
            flags: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AbiDescriptor {
    pub struct_size: u32,
    pub abi_version: u32,
    pub pointer_width_bits: u32,
    pub pointer_align_bytes: u32,
    pub runtime_options_size: u32,
    pub runtime_options_align: u32,
    pub status_size: u32,
    pub host_loop_profile_v1_size: u32,
    pub host_loop_profile_v1_align: u32,
    pub native_import_v1_size: u32,
    pub native_import_v1_align: u32,
    pub native_package_descriptor_v1_size: u32,
    pub native_package_descriptor_v1_align: u32,
    pub native_invocation_context_v1_size: u32,
    pub native_invocation_context_v1_align: u32,
    pub reserved: u32,
}

impl Default for AbiDescriptor {
    fn default() -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            abi_version: ABI_VERSION,
            pointer_width_bits: usize::BITS,
            pointer_align_bytes: align_of::<usize>() as u32,
            runtime_options_size: size_of::<RuntimeOptions>() as u32,
            runtime_options_align: align_of::<RuntimeOptions>() as u32,
            status_size: size_of::<Status>() as u32,
            host_loop_profile_v1_size: size_of::<HostLoopProfileV1>() as u32,
            host_loop_profile_v1_align: align_of::<HostLoopProfileV1>() as u32,
            native_import_v1_size: size_of::<NativeImportV1>() as u32,
            native_import_v1_align: align_of::<NativeImportV1>() as u32,
            native_package_descriptor_v1_size: size_of::<NativePackageDescriptorV1>() as u32,
            native_package_descriptor_v1_align: align_of::<NativePackageDescriptorV1>() as u32,
            native_invocation_context_v1_size: size_of::<NativeInvocationContextV1>() as u32,
            native_invocation_context_v1_align: align_of::<NativeInvocationContextV1>() as u32,
            reserved: 0,
        }
    }
}

pub type WakeCallback = unsafe extern "C" fn(context: *mut c_void);

#[must_use]
pub const fn abi_major(version: u32) -> u32 {
    version >> 24
}

#[must_use]
pub const fn is_compatible(version: u32) -> bool {
    abi_major(version) == ABI_VERSION_MAJOR
}

const _: () = {
    assert!(size_of::<Status>() == 4);
    assert!(size_of::<RuntimeOptions>() == 16);
    assert!(align_of::<RuntimeOptions>() == 8);
    assert!(size_of::<AbiDescriptor>() == 64);
    assert!(align_of::<AbiDescriptor>() == 4);
    assert!(size_of::<HostLoopProfileV1>() == 56);
    assert!(align_of::<HostLoopProfileV1>() == 8);
    assert!(size_of::<NativeImportV1>() == 32);
    assert!(align_of::<NativeImportV1>() == 8);
    assert!(size_of::<NativePackageDescriptorV1>() == 48);
    assert!(align_of::<NativePackageDescriptorV1>() == 8);
    assert!(size_of::<NativeInvocationContextV1>() == 24);
    assert!(align_of::<NativeInvocationContextV1>() == 8);
};

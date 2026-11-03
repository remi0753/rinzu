use rinzu_abi::{
    ABI_VERSION, AbiDescriptor, HostLoopProfileV1, NativeImportV1, NativeInvocationContextV1,
    NativePackageDescriptorV1, RuntimeOptions, Status,
};

unsafe extern "C" {
    fn rinzu_c_abi_version() -> u32;
    fn rinzu_c_status_size() -> usize;
    fn rinzu_c_runtime_options_size() -> usize;
    fn rinzu_c_runtime_options_align() -> usize;
    fn rinzu_c_descriptor_size() -> usize;
    fn rinzu_c_descriptor_align() -> usize;
    fn rinzu_c_host_loop_profile_size() -> usize;
    fn rinzu_c_host_loop_profile_align() -> usize;
    fn rinzu_c_native_import_size() -> usize;
    fn rinzu_c_native_import_align() -> usize;
    fn rinzu_c_native_package_descriptor_size() -> usize;
    fn rinzu_c_native_package_descriptor_align() -> usize;
    fn rinzu_c_native_invocation_context_size() -> usize;
    fn rinzu_c_native_invocation_context_align() -> usize;
}

#[test]
fn c_and_rust_layouts_match() {
    // SAFETY: these no-argument functions are compiled from the checked C fixture.
    unsafe {
        assert_eq!(rinzu_c_abi_version(), ABI_VERSION);
        assert_eq!(rinzu_c_status_size(), size_of::<Status>());
        assert_eq!(rinzu_c_runtime_options_size(), size_of::<RuntimeOptions>());
        assert_eq!(
            rinzu_c_runtime_options_align(),
            align_of::<RuntimeOptions>()
        );
        assert_eq!(rinzu_c_descriptor_size(), size_of::<AbiDescriptor>());
        assert_eq!(rinzu_c_descriptor_align(), align_of::<AbiDescriptor>());
        assert_eq!(
            rinzu_c_host_loop_profile_size(),
            size_of::<HostLoopProfileV1>()
        );
        assert_eq!(
            rinzu_c_host_loop_profile_align(),
            align_of::<HostLoopProfileV1>()
        );
        assert_eq!(rinzu_c_native_import_size(), size_of::<NativeImportV1>());
        assert_eq!(rinzu_c_native_import_align(), align_of::<NativeImportV1>());
        assert_eq!(
            rinzu_c_native_package_descriptor_size(),
            size_of::<NativePackageDescriptorV1>()
        );
        assert_eq!(
            rinzu_c_native_package_descriptor_align(),
            align_of::<NativePackageDescriptorV1>()
        );
        assert_eq!(
            rinzu_c_native_invocation_context_size(),
            size_of::<NativeInvocationContextV1>()
        );
        assert_eq!(
            rinzu_c_native_invocation_context_align(),
            align_of::<NativeInvocationContextV1>()
        );
    }
}

#[test]
fn source_v2_layout_matches_c_and_frozen_offsets() {
    use core::mem::offset_of;
    use rinzu_abi::{SourceImportV2 as I, SourcePackageDescriptorV2 as D};
    unsafe extern "C" {
        fn rinzu_c_source_v2_layout(index: usize) -> usize;
    }
    let rust = [
        size_of::<I>(),
        align_of::<I>(),
        offset_of!(I, import_key),
        offset_of!(I, symbol),
        offset_of!(I, address),
        size_of::<D>(),
        align_of::<D>(),
        offset_of!(D, struct_size),
        offset_of!(D, major),
        offset_of!(D, minor),
        offset_of!(D, descriptor),
        offset_of!(D, descriptor_len),
        offset_of!(D, import_count),
        offset_of!(D, imports),
    ];
    assert_eq!(rust, [48, 8, 0, 32, 40, 40, 8, 0, 4, 6, 8, 16, 24, 32]);
    for (index, value) in rust.into_iter().enumerate() {
        // SAFETY: C fixture returns layout constants and dereferences no pointers.
        assert_eq!(unsafe { rinzu_c_source_v2_layout(index) }, value);
    }
}

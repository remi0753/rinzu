//! Import identities for the retained versioned native package ABI.
//! SDK authoring uses source declarations and generated source-package v2.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeImport {
    pub export_name: String,
    pub import_key: u64,
    pub canonical_key: String,
    pub abi_signature: String,
    pub symbol: String,
    pub runtime_service: Option<String>,
}

#[must_use]
pub fn canonical_import_key(package: &str, export: &str, abi_signature: &str) -> String {
    format!("{package}@0.2|{export}|{abi_signature}")
}

#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    })
}

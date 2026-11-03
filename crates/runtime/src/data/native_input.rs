use super::*;

/// One ordinary native ByteArray result. External storage is copied before
/// the result call ledger releases its native lifetime.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NativeByteArray {
    pub data: *mut u8,
    pub length: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeInputError {
    NullString,
    NullByteArray,
    InvalidUtf8,
    Data(DataError),
}

fn checked<T>(value: Result<T, DataError>) -> Result<T, NativeInputError> {
    match value {
        Err(
            DataError::AddressSpaceExhausted | DataError::Heap(HeapError::AddressSpaceExhausted),
        ) => {
            // SAFETY: a resource fatal must not allocate or unwind through external code.
            crate::native_fatal::resource()
        }
        other => other.map_err(NativeInputError::Data),
    }
}

unsafe fn copy(pointer: *const u8, length: usize) -> Vec<u8> {
    let mut bytes = Vec::<u8>::new();
    if length > isize::MAX as usize || bytes.try_reserve_exact(length).is_err() {
        // SAFETY: no managed cleanup or further allocation is required on OOM.
        crate::native_fatal::resource()
    }
    if length != 0 {
        // SAFETY: destination is a reserved native buffer; the external contract
        // supplies length readable bytes. An external read fault is process-fatal.
        unsafe { libc::memcpy(bytes.as_mut_ptr().cast(), pointer.cast(), length) };
        // SAFETY: memcpy has initialized exactly length bytes before returning.
        unsafe { bytes.set_len(length) };
    }
    bytes
}

impl ManagedData {
    /// Receive a descriptor and copy only its readable byte range.
    ///
    /// # Safety
    /// A non-null descriptor and its positive-length payload must stay readable
    /// and unchanged until this method returns. Invalid native memory is fatal.
    pub unsafe fn receive_native_byte_array(
        &mut self,
        heap: &mut DomainHeap,
        descriptor: *const NativeByteArray,
    ) -> Result<ManagedHandle, NativeInputError> {
        checked(self.require_domain(heap))?;
        if descriptor.is_null() {
            return Err(NativeInputError::NullByteArray);
        }
        // SAFETY: the exact external result contract provides one readable POD.
        let view = unsafe { descriptor.read_unaligned() };
        if view.length > isize::MAX as usize {
            // SAFETY: resource failure must not allocate or unwind across FFI.
            crate::native_fatal::resource()
        }
        if view.length != 0 && view.data.is_null() {
            // SAFETY: an invalid native payload is not an ordinary Error.
            crate::native_fatal::invariant()
        }
        let bytes = if view.length == 0 {
            &[]
        } else {
            // SAFETY: external storage remains stable through this bounded
            // copy, including allocation. Pack directly into owned heap data
            // without an intermediate native Vec and another payload copy.
            unsafe { std::slice::from_raw_parts(view.data, view.length) }
        };
        checked(self.create_bytes(heap, view.length as u64, bytes))
    }

    /// Copy all N bytes into this Domain before managed body entry. Empty input
    /// performs no read, including for a null native pointer.
    ///
    /// # Safety
    /// For N > 0, a non-null pointer must expose N readable bytes until this copy
    /// finishes. A null pointer or failed native read is a process fatal.
    pub unsafe fn receive_native_bytes(
        &mut self,
        heap: &mut DomainHeap,
        length: u64,
        pointer: *const u8,
    ) -> Result<ManagedHandle, NativeInputError> {
        checked(self.require_domain(heap))?;
        if length != 0 && pointer.is_null() {
            // SAFETY: a known positive-N null is a copy fault, never an OOM Error.
            crate::native_fatal::invariant()
        }
        let length = usize::try_from(length).unwrap_or_else(|_| {
            // SAFETY: unsupported native size is a resource fatal.
            crate::native_fatal::resource()
        });
        // SAFETY: the caller provides the exact fixed-length external contract.
        let bytes = unsafe { copy(pointer, length) };
        let _accounting = crate::resource_accounting::Allocation::new(
            crate::resource_accounting::Kind::NativeInput,
            length as u64,
        );
        checked(self.create_bytes(heap, length as u64, &bytes))
    }

    /// Receive a NUL-terminated UTF-8 string through its one physical pointer.
    /// NULL/invalid UTF-8 are recoverable and do not allocate a managed value.
    ///
    /// # Safety
    /// Non-null input must remain readable through a terminating NUL until copy
    /// finishes. Invalid native memory is process-fatal rather than an Error.
    pub unsafe fn receive_native_string(
        &mut self,
        heap: &mut DomainHeap,
        pointer: *const u8,
    ) -> Result<ManagedHandle, NativeInputError> {
        checked(self.require_domain(heap))?;
        if pointer.is_null() {
            return Err(NativeInputError::NullString);
        }
        // SAFETY: the SDK provides a readable NUL-terminated native string.
        let length = unsafe { libc::strlen(pointer.cast()) };
        // SAFETY: strlen established the readable prefix; copy owns all input.
        let bytes = unsafe { copy(pointer, length) };
        let _accounting = crate::resource_accounting::Allocation::new(
            crate::resource_accounting::Kind::NativeInput,
            length as u64,
        );
        let text = std::str::from_utf8(&bytes).map_err(|_| NativeInputError::InvalidUtf8)?;
        checked(self.create_string(heap, text))
    }
}

//! Rooted immutable managed data values. Slots contain stable identities,
//! never semispace addresses; all payloads and references remain in the Domain heap.

use crate::failure::{Diagnostic, ErrorKind, Failure, FailureCause, SourcePosition};
use crate::heap::{DomainHeap, FinalizerPolicy, HeapError, HeapRef, HeapValue, RootSlot};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod packed_read_tests {
    use super::*;
    use crate::heap::CollectionKind;

    #[test]
    fn owned_packed_reads_keep_unicode_nul_padding_aliases_and_gc() {
        let mut heap = DomainHeap::new(72);
        let mut data = ManagedData::new(72);
        for text in [
            String::new(),
            "A\0B".into(),
            "日本語🙂".into(),
            "x".repeat(7),
            "x".repeat(8),
            "x".repeat(9),
            "x".repeat(65536),
        ] {
            let string = data.create_string(&mut heap, &text).unwrap();
            let equal = data.create_string(&mut heap, &text).unwrap();
            let binary = data
                .create_bytes(&mut heap, text.len() as u64, text.as_bytes())
                .unwrap();
            for _ in 0..2 {
                heap.collect(CollectionKind::Full).unwrap();
                assert_eq!(data.string_length(&heap, string).unwrap(), text.len());
                assert!(data.strings_equal(&heap, string, equal).unwrap());
                assert_eq!(
                    data.string_hash(&heap, string).unwrap(),
                    crate::key::string_hash(&text)
                );
                assert_eq!(
                    data.byte_length(&heap, binary, u64::MAX).unwrap(),
                    text.len() as u64
                );
                let mut exported = data.string(&heap, string).unwrap();
                assert_eq!(exported, text);
                exported.push('!');
                assert_eq!(data.string(&heap, string).unwrap(), text);
                let mut bytes = data.bytes(&heap, binary, text.len() as u64).unwrap();
                assert_eq!(bytes, text.as_bytes());
                if let Some(first) = bytes.first_mut() {
                    *first ^= 255;
                }
                assert_eq!(
                    data.bytes(&heap, binary, text.len() as u64).unwrap(),
                    text.as_bytes()
                );
            }
        }
        let other = DomainHeap::new(73);
        let string = data.create_string(&mut heap, "owner").unwrap();
        assert!(data.string(&other, string).is_err());
        assert!(data.bytes(&heap, string, 5).is_err());
    }

    #[test]
    fn packed_reads_reject_corrupt_lengths_padding_utf8_and_field_types() {
        let mut heap = DomainHeap::new(74);
        let mut data = ManagedData::new(74);
        for (length, word) in [(-1, 120), (9, 120), (1, 0xff), (1, 0x0178)] {
            let handle = data.create_string(&mut heap, "x").unwrap();
            let reference = data.reference(&heap, handle, DataKind::String).unwrap();
            heap.write_field(reference, 0, HeapValue::Integer(length))
                .unwrap();
            heap.write_field(reference, 1, HeapValue::Integer(word))
                .unwrap();
            assert_eq!(data.string(&heap, handle), Err(DataError::InvalidEncoding));
        }
        let handle = data.create_string(&mut heap, "x").unwrap();
        let reference = data.reference(&heap, handle, DataKind::String).unwrap();
        heap.write_field(reference, 1, HeapValue::Null).unwrap();
        assert_eq!(data.string(&heap, handle), Err(DataError::InvalidEncoding));
        let handle = data.create_bytes(&mut heap, 1, b"x").unwrap();
        let reference = data.reference(&heap, handle, DataKind::Bytes(1)).unwrap();
        heap.write_field(reference, 1, HeapValue::Integer(0x0178))
            .unwrap();
        assert_eq!(
            data.bytes(&heap, handle, 1),
            Err(DataError::InvalidEncoding)
        );
    }
}

mod arguments;
pub use arguments::ProgramArguments;
mod bytes_operations;
mod collections;
mod composite;
pub mod display;
mod error_operations;
mod future_references;
#[cfg(unix)]
mod json;
#[cfg(unix)]
pub use json::JsonCall;
mod managed_error;
pub use managed_error::ManagedError;
mod error_source;
pub use error_source::SourceCatalog;
mod native_input;
mod native_opaque;
mod native_record;
mod native_record_loans;
mod native_userdata;
mod numeric_operations;
mod operations;
mod text_operations;
pub use composite::ManagedSlot;
pub use native_input::{NativeByteArray, NativeInputError};
pub use native_opaque::{NativeOpaqueError, OpaqueReservation};
pub use native_record::{
    NativeRecordBuffer, NativeRecordError, NativeRecordField, NativeRecordIdentity,
    NativeRecordLayout,
};
pub use native_record_loans::{
    NATIVE_RECORD_SCRATCH_LIMIT, NativeRecordLoan, NativeRecordLoanError, NativeRecordLoanStats,
    NativeRecordScratchStats,
};
pub use native_userdata::{
    NativeUserDataError, NativeUserDataEvidence, NativeUserDataInvocation,
    NativeUserDataProcessStats, NativeUserDataRegistry, NativeUserDataScope, NativeUserDataToken,
    native_context_identity, userdata_process_stats,
};

const HANDLE_BASE: u64 = 1 << 62;
static NEXT_HANDLE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(HANDLE_BASE);
const STRING_TYPE: u32 = 0xffff_ff01;
const ERROR_TYPE: u32 = 0xffff_ff02;
const BYTES_TYPE: u32 = 0xffff_ff03;
const COMPOSITE_TYPE: u32 = 0xffff_ff04;
const OPAQUE_TYPE: u32 = 0xffff_ff05;
const COLLECTION_BUFFER_TYPE: u32 = 0xffff_ff06;

/// Length followed by canonical little-endian, zero-padded integer words.
/// The borrowed bytes remain immutable until heap allocation has completed.
struct PackedWords<'a> {
    length: Option<i64>,
    chunks: std::slice::Chunks<'a, u8>,
}
impl<'a> PackedWords<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, DataError> {
        Ok(Self {
            length: Some(i64::try_from(bytes.len()).map_err(|_| DataError::AddressSpaceExhausted)?),
            chunks: bytes.chunks(8),
        })
    }
}
impl Iterator for PackedWords<'_> {
    type Item = i64;
    fn next(&mut self) -> Option<i64> {
        if let Some(length) = self.length.take() {
            return Some(length);
        }
        let bytes = self.chunks.next()?;
        let mut word = [0; 8];
        word[..bytes.len()].copy_from_slice(bytes);
        Some(u64::from_le_bytes(word) as i64)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = usize::from(self.length.is_some()) + self.chunks.len();
        (count, Some(count))
    }
}
impl ExactSizeIterator for PackedWords<'_> {}
impl std::iter::FusedIterator for PackedWords<'_> {}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ManagedHandle(u64);
impl ManagedHandle {
    pub const fn bits(self) -> u64 {
        self.0
    }
    pub const fn from_bits(bits: u64) -> Option<Self> {
        if bits > HANDLE_BASE && bits <= i64::MAX as u64 {
            Some(Self(bits))
        } else {
            None
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DataKind {
    String,
    Error,
    RichError(u64),
    Bytes(u64),
    Composite(usize),
    ReservedOpaque(usize),
    NativeOpaque(usize),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataError {
    /// A normal language Error returned by a checked intrinsic.
    Thrown(crate::error_value::ErrorValue),
    ManagedThrown(ManagedHandle),
    Interrupted(crate::failure::FailureCause),
    CollectionOverflow {
        operation: rinzu_abi::data_operation::DataOperation,
        current: i64,
        added: i64,
    },
    /// Explicit, uncatchable managed failure. The owned message may be written
    /// only after the caller publishes roots and enters native-safe state.
    Panic(String),
    Heap(HeapError),
    WrongDomain,
    UnknownHandle,
    WrongKind,
    InvalidEncoding,
    NotRecoverable,
    LeasedHandle,
    AddressSpaceExhausted,
    Value(crate::value::ValueError),
}
impl From<HeapError> for DataError {
    fn from(error: HeapError) -> Self {
        Self::Heap(error)
    }
}

#[derive(Debug)]
pub struct ManagedData {
    domain: u64,
    roots: BTreeMap<ManagedHandle, (RootSlot, DataKind)>,
    schema: Option<std::sync::Arc<crate::value::ValueSchema>>,
    error_leases: BTreeMap<ManagedHandle, std::sync::Weak<()>>,
    source_catalog: std::sync::Arc<SourceCatalog>,
    native_records: native_record_loans::NativeRecordLoans,
}
impl ManagedData {
    pub fn new(domain: u64) -> Self {
        Self {
            domain,
            roots: BTreeMap::new(),
            schema: None,
            error_leases: BTreeMap::new(),
            source_catalog: Default::default(),
            native_records: Default::default(),
        }
    }
    fn require_domain(&self, heap: &DomainHeap) -> Result<(), DataError> {
        if heap.owner_thread() != std::thread::current().id() {
            return Err(DataError::Heap(HeapError::WrongThread {
                expected: heap.owner_thread(),
                actual: std::thread::current().id(),
            }));
        }
        if heap.domain_id() == self.domain {
            Ok(())
        } else {
            Err(DataError::WrongDomain)
        }
    }
    fn root(
        &mut self,
        heap: &mut DomainHeap,
        reference: HeapRef,
        kind: DataKind,
    ) -> Result<ManagedHandle, DataError> {
        self.require_domain(heap)?;
        let next = NEXT_HANDLE
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |value| value.checked_add(1).filter(|next| *next <= i64::MAX as u64),
            )
            .map_err(|_| DataError::AddressSpaceExhausted)?
            + 1;
        let root = heap.create_root(Some(reference))?;
        let handle = ManagedHandle(next);
        self.roots.insert(handle, (root, kind));
        Ok(handle)
    }
    fn reference(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
        expected: DataKind,
    ) -> Result<HeapRef, DataError> {
        self.require_domain(heap)?;
        let (root, kind) = self.roots.get(&handle).ok_or(DataError::UnknownHandle)?;
        if *kind != expected {
            return Err(DataError::WrongKind);
        }
        heap.root_target(*root)?.ok_or(DataError::InvalidEncoding)
    }
    pub fn create_string(
        &mut self,
        heap: &mut DomainHeap,
        value: &str,
    ) -> Result<ManagedHandle, DataError> {
        self.require_domain(heap)?;
        let reference = heap.allocate_integers(STRING_TYPE, PackedWords::new(value.as_bytes())?)?;
        self.root(heap, reference, DataKind::String)
    }
    fn string_at(heap: &DomainHeap, reference: HeapRef) -> Result<String, DataError> {
        if heap.header(reference)?.type_id != STRING_TYPE {
            return Err(DataError::WrongKind);
        }
        let mut fields = heap.fields_iter(reference)?;
        let first = fields.next();
        let length = integer(first.as_ref())?;
        let length = usize::try_from(length).map_err(|_| DataError::InvalidEncoding)?;
        if fields.len() != length.div_ceil(8) {
            return Err(DataError::InvalidEncoding);
        }
        let mut bytes = Vec::with_capacity(length);
        for field in fields {
            let word = (integer(Some(&field))? as u64).to_le_bytes();
            let count = (length - bytes.len()).min(8);
            bytes.extend_from_slice(&word[..count]);
            if word[count..].iter().any(|byte| *byte != 0) {
                return Err(DataError::InvalidEncoding);
            }
        }
        String::from_utf8(bytes).map_err(|_| DataError::InvalidEncoding)
    }
    pub fn string(&self, heap: &DomainHeap, handle: ManagedHandle) -> Result<String, DataError> {
        Self::string_at(heap, self.reference(heap, handle, DataKind::String)?)
    }
    pub fn string_length(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
    ) -> Result<usize, DataError> {
        let mut fields = heap.fields_iter(self.reference(heap, handle, DataKind::String)?)?;
        let first = fields.next();
        let length =
            usize::try_from(integer(first.as_ref())?).map_err(|_| DataError::InvalidEncoding)?;
        if fields.len() != length.div_ceil(8) {
            return Err(DataError::InvalidEncoding);
        }
        Ok(length)
    }
    pub fn strings_equal(
        &self,
        heap: &DomainHeap,
        left: ManagedHandle,
        right: ManagedHandle,
    ) -> Result<bool, DataError> {
        let left = self.reference(heap, left, DataKind::String)?;
        let right = self.reference(heap, right, DataKind::String)?;
        // Packed bytes have canonical zero padding. Compare content, including
        // embedded NUL, without allocating a second copy or normalizing Unicode.
        Ok(heap.fields_iter(left)?.eq(heap.fields_iter(right)?))
    }
    pub fn string_hash(&self, heap: &DomainHeap, handle: ManagedHandle) -> Result<u64, DataError> {
        let reference = self.reference(heap, handle, DataKind::String)?;
        let mut fields = heap.fields_iter(reference)?;
        let first = fields.next();
        let length =
            usize::try_from(integer(first.as_ref())?).map_err(|_| DataError::InvalidEncoding)?;
        if fields.len() != length.div_ceil(8) {
            return Err(DataError::InvalidEncoding);
        }
        let mut hash = crate::key::KeyHasher::default();
        hash.string_header(length);
        let mut remaining = length;
        for field in fields {
            let bytes = (integer(Some(&field))? as u64).to_le_bytes();
            let count = remaining.min(8);
            if bytes[count..].iter().any(|byte| *byte != 0) {
                return Err(DataError::InvalidEncoding);
            }
            hash.write(&bytes[..count]);
            remaining -= count;
        }
        Ok(hash.finish())
    }
    pub fn create_error(
        &mut self,
        heap: &mut DomainHeap,
        failure: &Failure,
    ) -> Result<ManagedHandle, DataError> {
        if !failure.recoverable() {
            return Err(DataError::NotRecoverable);
        }
        if let Some(error) = failure.managed.get(&0) {
            if !failure.cleanup.is_empty() {
                return Err(DataError::WrongKind);
            }
            let handle = self.leased_error(heap, error)?;
            if let Some(trace) = failure.traces.get(&0) {
                self.first_error_trace(heap, handle, std::sync::Arc::clone(trace))?;
            }
            return Ok(handle);
        }
        let mut message = failure.primary.cause.message().to_owned();
        for diagnostic in &failure.cleanup {
            message.push_str("; cleanup: ");
            message.push_str(diagnostic.cause.message());
        }
        let message = self.create_string(heap, &message)?;
        let result = (|| {
            let reference = self.reference(heap, message, DataKind::String)?;
            let count = failure
                .cleanup
                .len()
                .checked_add(1)
                .and_then(|value| i64::try_from(value).ok())
                .ok_or(DataError::AddressSpaceExhausted)?;
            let mut fields = vec![HeapValue::Reference(reference), HeapValue::Integer(count)];
            for diagnostic in std::iter::once(&failure.primary).chain(&failure.cleanup) {
                let source = diagnostic.source;
                fields.extend([
                    HeapValue::Integer(error_code(diagnostic.cause)?),
                    HeapValue::Integer(i64::from(source.is_some())),
                    HeapValue::Integer(source.map_or(0, |position| position.file as i64)),
                    HeapValue::Integer(source.map_or(0, |position| i64::from(position.start_byte))),
                    HeapValue::Integer(source.map_or(0, |position| i64::from(position.end_byte))),
                ]);
            }
            let reference = heap.allocate(ERROR_TYPE, &fields, FinalizerPolicy::None)?;
            self.root(heap, reference, DataKind::Error)
        })();
        self.release(heap, message)?;
        let handle = result?;
        if let Some(trace) = failure.traces.get(&0) {
            self.first_error_trace(heap, handle, std::sync::Arc::clone(trace))?;
        }
        Ok(handle)
    }
    pub fn error_message(
        &mut self,
        heap: &mut DomainHeap,
        error: ManagedHandle,
    ) -> Result<ManagedHandle, DataError> {
        if self.error_record_type(heap, error)?.is_some() {
            let ManagedSlot::Reference(message) = self.error_record_field(heap, error, 1)? else {
                return Err(DataError::InvalidEncoding);
            };
            return Ok(message);
        }
        if matches!(self.any_reference(heap, error)?.1, DataKind::RichError(_)) {
            let value = self.error_value(heap, error)?;
            return self.create_string(heap, &value.root().message);
        }
        let fields = heap.fields(self.reference(heap, error, DataKind::Error)?)?;
        let Some(HeapValue::Reference(message)) = fields.first() else {
            return Err(DataError::InvalidEncoding);
        };
        if heap.header(*message)?.type_id != STRING_TYPE {
            return Err(DataError::InvalidEncoding);
        }
        self.root(heap, *message, DataKind::String)
    }
    pub fn error(&self, heap: &DomainHeap, error: ManagedHandle) -> Result<Failure, DataError> {
        let reference = self.reference(heap, error, DataKind::Error)?;
        Self::failure_at(heap, reference)
    }
    fn failure_at(heap: &DomainHeap, reference: HeapRef) -> Result<Failure, DataError> {
        if heap.header(reference)?.type_id != ERROR_TYPE {
            return Err(DataError::WrongKind);
        }
        let fields = heap.fields(reference)?;
        let count =
            usize::try_from(integer(fields.get(1))?).map_err(|_| DataError::InvalidEncoding)?;
        if count == 0 || count.checked_mul(5).and_then(|n| n.checked_add(2)) != Some(fields.len()) {
            return Err(DataError::InvalidEncoding);
        }
        let mut diagnostics = Vec::with_capacity(count);
        for fields in fields[2..].chunks_exact(5) {
            let cause = FailureCause::Error(match integer(fields.first())? {
                0 => ErrorKind::Conversion,
                1 => ErrorKind::NullNativeValue,
                2 => ErrorKind::InvalidUtf8,
                3 => ErrorKind::NativeRecordBusy,
                4 => ErrorKind::UnobservedChildFailure,
                5 => ErrorKind::Io,
                6 => ErrorKind::SupplantBusy,
                7 => ErrorKind::UnboundValue,
                _ => return Err(DataError::InvalidEncoding),
            });
            let present = integer(fields.get(1))?;
            let source = match present {
                0 => None,
                1 => Some(SourcePosition {
                    file: integer(fields.get(2))? as u64,
                    start_byte: u32::try_from(integer(fields.get(3))?)
                        .map_err(|_| DataError::InvalidEncoding)?,
                    end_byte: u32::try_from(integer(fields.get(4))?)
                        .map_err(|_| DataError::InvalidEncoding)?,
                }),
                _ => return Err(DataError::InvalidEncoding),
            };
            diagnostics.push(Diagnostic { cause, source });
        }
        Ok(Failure {
            managed: Default::default(),
            traces: Default::default(),
            primary: diagnostics.remove(0),
            cleanup: diagnostics,
        })
    }
    pub fn release(
        &mut self,
        heap: &mut DomainHeap,
        handle: ManagedHandle,
    ) -> Result<(), DataError> {
        self.require_domain(heap)?;
        if self
            .error_leases
            .get(&handle)
            .is_some_and(|lease| lease.strong_count() != 0)
        {
            return Err(DataError::LeasedHandle);
        }
        let (root, _) = *self.roots.get(&handle).ok_or(DataError::UnknownHandle)?;
        heap.release_root(root)?;
        self.roots.remove(&handle);
        self.error_leases.remove(&handle);
        Ok(())
    }
    /// Publish a complete live-root set before collecting. Validate the entire set
    /// first, so a malformed stack map cannot partially discard existing roots.
    pub fn retain_roots(
        &mut self,
        heap: &mut DomainHeap,
        live: impl IntoIterator<Item = ManagedHandle>,
    ) -> Result<(), DataError> {
        self.require_domain(heap)?;
        let mut live: BTreeSet<_> = live.into_iter().collect();
        // Native loans own private heap roots independently of source handles.
        self.error_leases.retain(|handle, lease| {
            if lease.strong_count() == 0 {
                false
            } else {
                live.insert(*handle);
                true
            }
        });
        if live.iter().any(|handle| !self.roots.contains_key(handle)) {
            return Err(DataError::UnknownHandle);
        }
        let dead: Vec<_> = self
            .roots
            .keys()
            .filter(|handle| !live.contains(handle))
            .copied()
            .collect();
        for handle in dead {
            self.release(heap, handle)?;
        }
        Ok(())
    }
    pub fn release_all(&mut self, heap: &mut DomainHeap) -> Result<(), DataError> {
        self.require_domain(heap)?;
        if self.native_records.has_loans() {
            return Err(DataError::LeasedHandle);
        }
        for (root, _) in self.roots.values() {
            heap.release_root(*root)?;
        }
        self.roots.clear();
        self.error_leases.clear();
        Ok(())
    }
    /// Validate new roots without scanning unrelated provisional roots. The
    /// complete live set is retained when a collection is due.
    pub fn validate_roots(
        &self,
        heap: &DomainHeap,
        live: impl IntoIterator<Item = ManagedHandle>,
    ) -> Result<(), DataError> {
        self.require_domain(heap)?;
        for handle in live {
            if !self.roots.contains_key(&handle) {
                return Err(DataError::UnknownHandle);
            }
        }
        Ok(())
    }
    pub fn root_count(&self) -> usize {
        self.roots.len() + self.native_records.root_count()
    }
}
fn integer(field: Option<&HeapValue>) -> Result<i64, DataError> {
    if let Some(HeapValue::Integer(value)) = field {
        Ok(*value)
    } else {
        Err(DataError::InvalidEncoding)
    }
}
fn error_code(cause: FailureCause) -> Result<i64, DataError> {
    Ok(match cause {
        FailureCause::Error(ErrorKind::Conversion) => 0,
        FailureCause::Error(ErrorKind::NullNativeValue) => 1,
        FailureCause::Error(ErrorKind::InvalidUtf8) => 2,
        FailureCause::Error(ErrorKind::NativeRecordBusy) => 3,
        FailureCause::Error(ErrorKind::UnobservedChildFailure) => 4,
        FailureCause::Error(ErrorKind::Io) => 5,
        FailureCause::Error(ErrorKind::SupplantBusy) => 6,
        FailureCause::Error(ErrorKind::UnboundValue) => 7,
        _ => return Err(DataError::NotRecoverable),
    })
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RootLease(u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedRootError {
    MissingFrame,
    UnknownLease,
    IdentityExhausted,
    PublicationInProgress,
    InvalidPublication,
}

#[derive(Debug, Default)]
struct ManagedRootFrame {
    words: Vec<u64>,
    count: usize,
    prepared: Option<usize>,
}

/// Caller snapshots remain roots while a callee runs. Fresh allocations stay
/// provisionally rooted in ManagedData until the next stack-map publication.
#[derive(Debug, Default)]
pub struct ManagedRoots {
    frames: Vec<ManagedRootFrame>,
    depth: usize,
    leases: std::collections::HashMap<RootLease, ManagedHandle>,
    next_lease: u64,
}
impl ManagedRoots {
    pub fn enter_frame(&mut self) {
        if self.depth != 0 && self.frames[self.depth - 1].prepared.is_some() {
            std::process::abort();
        }
        if self.depth == self.frames.len() {
            self.frames.push(ManagedRootFrame::default());
        } else {
            self.frames[self.depth].count = 0;
        }
        self.depth += 1;
    }
    pub fn publish(
        &mut self,
        live: impl IntoIterator<Item = ManagedHandle>,
    ) -> Result<(), ManagedRootError> {
        let frame = self
            .depth
            .checked_sub(1)
            .and_then(|index| self.frames.get_mut(index))
            .ok_or(ManagedRootError::MissingFrame)?;
        if frame.prepared.is_some() {
            return Err(ManagedRootError::PublicationInProgress);
        }
        frame.words.clear();
        frame
            .words
            .extend(live.into_iter().map(ManagedHandle::bits));
        frame.count = frame.words.len();
        Ok(())
    }
    /// Private compiler protocol. The pointer stays valid until commit; generated
    /// code must only write `count` aligned u64 slots and cannot call or yield.
    /// Raw words allow the unbound (zero) sentinel without invalid handle values.
    pub fn prepare_publication(&mut self, count: usize) -> Result<*mut u64, ManagedRootError> {
        if count > isize::MAX as usize / core::mem::size_of::<u64>() {
            return Err(ManagedRootError::InvalidPublication);
        }
        let frame = self
            .depth
            .checked_sub(1)
            .and_then(|index| self.frames.get_mut(index))
            .ok_or(ManagedRootError::MissingFrame)?;
        if frame.prepared.is_some() {
            return Err(ManagedRootError::PublicationInProgress);
        }
        if count > frame.words.len() {
            if frame.words.try_reserve(count - frame.words.len()).is_err() {
                // SAFETY: failure cannot unwind into generated/native code.
                unsafe { libc::_exit(71) }
            }
            frame.words.resize(count, 0);
        }
        frame.prepared = Some(count);
        Ok(frame.words.as_mut_ptr())
    }
    pub fn commit_publication(&mut self, count: usize) -> Result<(), ManagedRootError> {
        let frame = self
            .depth
            .checked_sub(1)
            .and_then(|index| self.frames.get_mut(index))
            .ok_or(ManagedRootError::MissingFrame)?;
        if frame.prepared != Some(count) {
            return Err(ManagedRootError::InvalidPublication);
        }
        frame.count = count;
        frame.prepared = None;
        Ok(())
    }
    pub fn leave_frame(&mut self) -> Result<(), ManagedRootError> {
        let index = self
            .depth
            .checked_sub(1)
            .ok_or(ManagedRootError::MissingFrame)?;
        if self.frames[index].prepared.is_some() {
            return Err(ManagedRootError::PublicationInProgress);
        }
        self.frames[index].count = 0;
        self.depth = index;
        Ok(())
    }
    pub fn pin(&mut self, handle: ManagedHandle) -> Result<RootLease, ManagedRootError> {
        if self.leases.try_reserve(1).is_err() {
            // SAFETY: a known root allocation failure cannot allocate a diagnostic
            // or unwind through a native callback/physical call.
            unsafe { libc::_exit(71) }
        }
        self.next_lease = self
            .next_lease
            .checked_add(1)
            .ok_or(ManagedRootError::IdentityExhausted)?;
        let lease = RootLease(self.next_lease);
        self.leases.insert(lease, handle);
        Ok(lease)
    }
    pub fn unpin(&mut self, lease: RootLease) -> Result<(), ManagedRootError> {
        self.leases
            .remove(&lease)
            .map(|_| ())
            .ok_or(ManagedRootError::UnknownLease)
    }
    pub fn live(&self) -> impl Iterator<Item = ManagedHandle> + '_ {
        if self.frames[..self.depth]
            .iter()
            .any(|frame| frame.prepared.is_some())
        {
            std::process::abort();
        }
        self.frames[..self.depth]
            .iter()
            .flat_map(|frame| &frame.words[..frame.count])
            .filter_map(|word| ManagedHandle::from_bits(*word))
            .chain(self.leases.values().copied())
    }
    pub fn current_live(&self) -> impl Iterator<Item = ManagedHandle> + '_ {
        self.frames[..self.depth]
            .last()
            .into_iter()
            .flat_map(|frame| {
                if frame.prepared.is_some() {
                    std::process::abort();
                }
                &frame.words[..frame.count]
            })
            .filter_map(|word| ManagedHandle::from_bits(*word))
    }
    pub fn frame_count(&self) -> usize {
        self.depth
    }
    pub fn lease_count(&self) -> usize {
        self.leases.len()
    }
}

#[cfg(test)]
mod root_storage_tests {
    use super::*;

    #[test]
    fn direct_publication_handles_unbound_words_and_rejects_incomplete_frames() {
        let handle = ManagedHandle::from_bits(HANDLE_BASE + 1).unwrap();
        let mut roots = ManagedRoots::default();
        roots.enter_frame();
        roots.publish([handle]).unwrap();
        let words = roots.prepare_publication(64).unwrap();
        assert_eq!(roots.frames[0].count, 1);
        // SAFETY: prepare reserved 64 words and this region cannot call or yield.
        unsafe {
            assert_eq!(*words, handle.bits());
            for index in 0..64 {
                words
                    .add(index)
                    .write(if index == 1 { handle.bits() } else { 0 });
            }
        }
        assert_eq!(
            roots.publish([]),
            Err(ManagedRootError::PublicationInProgress)
        );
        assert_eq!(
            roots.leave_frame(),
            Err(ManagedRootError::PublicationInProgress)
        );
        assert_eq!(
            roots.commit_publication(1),
            Err(ManagedRootError::InvalidPublication)
        );
        roots.commit_publication(64).unwrap();
        assert_eq!(roots.live().collect::<Vec<_>>(), [handle]);
        assert_eq!(roots.current_live().collect::<Vec<_>>(), [handle]);
        let reused = roots.prepare_publication(0).unwrap();
        assert_eq!(reused, words);
        roots.commit_publication(0).unwrap();
        assert_eq!(roots.live().count(), 0);
        roots.leave_frame().unwrap();
        assert_eq!(
            roots.prepare_publication(1),
            Err(ManagedRootError::MissingFrame)
        );
    }

    #[test]
    fn repeated_calls_reuse_frames_and_only_publish_the_active_prefix() {
        let caller = ManagedHandle::from_bits(HANDLE_BASE + 1).unwrap();
        let callee = ManagedHandle::from_bits(HANDLE_BASE + 2).unwrap();
        let mut roots = ManagedRoots::default();
        roots.enter_frame();
        roots.publish([caller]).unwrap();
        roots.enter_frame();
        roots.publish([callee; 32]).unwrap();
        let buffer = roots.frames[1].words.as_ptr();
        let capacity = roots.frames[1].words.capacity();
        roots.leave_frame().unwrap();
        for _ in 0..10000 {
            roots.enter_frame();
            assert_eq!(roots.live().collect::<Vec<_>>(), [caller]);
            roots.publish([callee; 4]).unwrap();
            roots.publish([callee]).unwrap();
            assert_eq!(roots.live().collect::<Vec<_>>(), [caller, callee]);
            assert_eq!(roots.frames[1].words.as_ptr(), buffer);
            assert_eq!(roots.frames[1].words.capacity(), capacity);
            roots.publish([]).unwrap();
            roots.leave_frame().unwrap();
        }
        assert_eq!(roots.frames.len(), 2);
        roots.leave_frame().unwrap();
        assert_eq!(roots.frame_count(), 0);
        assert_eq!(roots.live().count(), 0);
    }
}

//! Owner-local native record loans. Only empty, bounded scratch survives a call.
use super::*;
use std::sync::Arc;

pub const NATIVE_RECORD_SCRATCH_LIMIT: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NativeRecordLoan {
    domain: u64,
    serial: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeRecordLoanError {
    Busy,
    WrongDomain,
    UnknownLoan,
    InvalidPhase,
    Record(NativeRecordError),
}
impl From<NativeRecordError> for NativeRecordLoanError {
    fn from(value: NativeRecordError) -> Self {
        Self::Record(value)
    }
}
impl From<DataError> for NativeRecordLoanError {
    fn from(value: DataError) -> Self {
        NativeRecordError::from(value).into()
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeRecordLoanStats {
    pub calls: usize,
    pub targets: usize,
    pub bytes: usize,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeRecordScratchStats {
    pub retained_bytes: usize,
    pub cached_batches: usize,
    pub active_bytes: usize,
    /// Allocations/reallocations for batch, alias, C and validation storage;
    /// common managed roots and ordinary returned values are separate.
    pub temporary_allocations: usize,
}
#[derive(Debug)]
struct Entry {
    identity: NativeRecordIdentity,
    layout: Arc<NativeRecordLayout>,
    // Valid only within preparation or validated publication; roots survive GC.
    reference: Option<HeapRef>,
    root: Option<RootSlot>,
    offset: usize,
}
#[derive(Clone, Copy)]
struct PreparedTarget<'a> {
    layout: &'a Arc<NativeRecordLayout>,
    handle: ManagedHandle,
    reference: HeapRef,
    identity: NativeRecordIdentity,
}
fn checked_target<'a>(
    data: &ManagedData,
    heap: &DomainHeap,
    layout: &'a Arc<NativeRecordLayout>,
    handle: ManagedHandle,
    checked: &mut [Option<PreparedTarget<'a>>; 4],
) -> Result<PreparedTarget<'a>, NativeRecordLoanError> {
    if let Some(proof) = checked
        .iter()
        .flatten()
        .find(|proof| proof.handle == handle && Arc::ptr_eq(proof.layout, layout))
    {
        return Ok(*proof);
    }
    let reference = data.reference(heap, handle, DataKind::Composite(layout.ty))?;
    if let Some(proof) = checked
        .iter()
        .flatten()
        .find(|proof| proof.reference == reference && Arc::ptr_eq(proof.layout, layout))
    {
        return Ok(PreparedTarget { handle, ..*proof });
    }
    let identity = data.native_record_target_at_reference(heap, layout, reference)?;
    let proof = PreparedTarget {
        layout,
        handle,
        reference,
        identity,
    };
    if !checked
        .iter()
        .flatten()
        .any(|proof| proof.identity == identity)
        && let Some(slot) = checked.iter_mut().find(|slot| slot.is_none())
    {
        *slot = Some(proof);
    }
    Ok(proof)
}
#[derive(Debug)]
struct Batch {
    serial: u64,
    entries: Vec<Entry>,
    arguments: Vec<usize>,
    staged: Vec<crate::scalar::ScalarValue>,
    buffer: Option<NativeRecordBuffer>,
    phase: Phase,
    next: Option<Box<Batch>>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Prepared,
    Entered,
    Returned,
    Committed,
}
impl Batch {
    fn empty() -> Self {
        Self {
            serial: 0,
            entries: Vec::new(),
            arguments: Vec::new(),
            staged: Vec::new(),
            buffer: None,
            phase: Phase::Prepared,
            next: None,
        }
    }
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(
                self.entries
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Entry>()),
            )
            .saturating_add(
                self.arguments
                    .capacity()
                    .saturating_mul(std::mem::size_of::<usize>()),
            )
            .saturating_add(
                self.staged
                    .capacity()
                    .saturating_mul(std::mem::size_of::<crate::scalar::ScalarValue>()),
            )
            .saturating_add(self.buffer.as_ref().map_or(0, NativeRecordBuffer::capacity))
    }
    fn pointer(&self, entry: usize) -> *const u8 {
        // SAFETY: preparation checked every range against this stable buffer.
        unsafe {
            self.buffer
                .as_ref()
                .unwrap()
                .as_ptr()
                .add(self.entries[entry].offset)
        }
    }
}
#[derive(Debug, Default)]
struct Batches {
    head: Option<Box<Batch>>,
}
impl Batches {
    fn iter(&self) -> impl Iterator<Item = &Batch> {
        let mut next = self.head.as_deref();
        std::iter::from_fn(move || {
            let batch = next?;
            next = batch.next.as_deref();
            Some(batch)
        })
    }
    fn get(&self, loan: &NativeRecordLoan) -> Option<&Batch> {
        self.iter().find(|batch| batch.serial == loan.serial)
    }
    fn get_mut(&mut self, loan: &NativeRecordLoan) -> Option<&mut Batch> {
        let mut next = self.head.as_deref_mut();
        while let Some(batch) = next {
            if batch.serial == loan.serial {
                return Some(batch);
            }
            next = batch.next.as_deref_mut();
        }
        None
    }
    fn insert(&mut self, mut batch: Box<Batch>) {
        batch.next = self.head.take();
        self.head = Some(batch);
    }
    fn remove(&mut self, loan: &NativeRecordLoan) -> Option<Box<Batch>> {
        let mut link = &mut self.head;
        loop {
            if link.as_ref()?.serial == loan.serial {
                let mut batch = link.take().unwrap();
                *link = batch.next.take();
                return Some(batch);
            }
            link = &mut link.as_mut().unwrap().next;
        }
    }
}
fn fatal_writeback() -> ! {
    crate::native_fatal::invariant()
}
#[derive(Debug, Default)]
pub(super) struct NativeRecordLoans {
    serial: u64,
    batches: Batches,
    pool: Batches,
    retained: usize,
    allocations: usize,
}
impl NativeRecordLoans {
    pub(super) fn root_count(&self) -> usize {
        self.batches
            .iter()
            .flat_map(|batch| batch.entries.iter().filter_map(|entry| entry.root))
            .count()
    }
    pub(super) fn has_loans(&self) -> bool {
        self.batches.head.is_some()
    }
    fn busy(&self, identity: NativeRecordIdentity) -> bool {
        self.batches.iter().any(|batch| {
            batch.phase != Phase::Committed
                && batch.entries.iter().any(|entry| entry.identity == identity)
        })
    }
    fn allocated(&mut self) {
        self.allocations = self
            .allocations
            .checked_add(1)
            .unwrap_or_else(|| crate::native_fatal::resource());
    }
    fn take_scratch(&mut self) -> Box<Batch> {
        if let Some(mut batch) = self.pool.head.take() {
            self.pool.head = batch.next.take();
            self.retained -= batch.bytes();
            batch
        } else {
            self.allocated();
            let layout = std::alloc::Layout::new::<Batch>();
            // SAFETY: the nonzero Batch layout is checked before initializing its Box.
            let pointer = unsafe { std::alloc::alloc(layout) }.cast::<Batch>();
            if pointer.is_null() {
                crate::native_fatal::resource();
            }
            // SAFETY: this uniquely owned allocation has the exact Box<Batch> layout.
            unsafe {
                pointer.write(Batch::empty());
                Box::from_raw(pointer)
            }
        }
    }
    fn cache(&mut self, mut batch: Box<Batch>) {
        batch.entries.clear(); // Drop all layout Arcs and invalidate every managed reference.
        batch.arguments.clear();
        batch.staged.clear();
        batch.serial = 0;
        let bytes = batch.bytes();
        if bytes <= NATIVE_RECORD_SCRATCH_LIMIT.saturating_sub(self.retained) {
            self.retained += bytes;
            self.pool.insert(batch);
        }
    }
    fn reserve<T>(&mut self, values: &mut Vec<T>, count: usize) {
        if values.capacity() < count {
            self.allocated();
            values
                .try_reserve_exact(count - values.len())
                .unwrap_or_else(|_| crate::native_fatal::resource());
        }
    }
}
impl Drop for NativeRecordLoans {
    fn drop(&mut self) {
        if self
            .batches
            .iter()
            .any(|batch| matches!(batch.phase, Phase::Entered | Phase::Returned))
        {
            fatal_writeback();
        }
    }
}
fn validate_updates(
    data: &mut ManagedData,
    heap: &DomainHeap,
    loan: NativeRecordLoan,
    staged: &mut Vec<crate::scalar::ScalarValue>,
) -> Result<(), NativeRecordLoanError> {
    staged.clear();
    let count = data
        .native_records
        .batches
        .get(&loan)
        .unwrap()
        .entries
        .len();
    for index in 0..count {
        let batch = data.native_records.batches.get(&loan).unwrap();
        let entry = &batch.entries[index];
        let reference = heap
            .root_target(entry.root.unwrap())
            .map_err(DataError::from)?
            .ok_or(DataError::InvalidEncoding)?;
        if data.native_record_target_at_reference(heap, &entry.layout, reference)? != entry.identity
        {
            return Err(NativeRecordError::InvalidLayout.into());
        }
        // SAFETY: the returned loan owns this initialized struct and reserved staging.
        unsafe {
            entry.layout.receive_into(batch.pointer(index), staged)?;
        }
        data.native_records.batches.get_mut(&loan).unwrap().entries[index].reference =
            Some(reference);
    }
    Ok(())
}
impl ManagedData {
    pub fn enter_native_records(
        &mut self,
        heap: &DomainHeap,
        loan: NativeRecordLoan,
    ) -> Result<(), NativeRecordLoanError> {
        self.require_domain(heap)?;
        if loan.domain != self.domain {
            return Err(NativeRecordLoanError::WrongDomain);
        }
        let batch = self
            .native_records
            .batches
            .get_mut(&loan)
            .ok_or(NativeRecordLoanError::UnknownLoan)?;
        if batch.phase != Phase::Prepared {
            return Err(NativeRecordLoanError::InvalidPhase);
        }
        for entry in &mut batch.entries {
            entry.reference = None;
        }
        batch.phase = Phase::Entered;
        Ok(())
    }
    pub fn return_native_records(
        &mut self,
        heap: &DomainHeap,
        loan: NativeRecordLoan,
    ) -> Result<(), NativeRecordLoanError> {
        self.require_domain(heap)?;
        if loan.domain != self.domain {
            return Err(NativeRecordLoanError::WrongDomain);
        }
        let batch = self
            .native_records
            .batches
            .get_mut(&loan)
            .ok_or(NativeRecordLoanError::UnknownLoan)?;
        if batch.phase != Phase::Entered {
            return Err(NativeRecordLoanError::InvalidPhase);
        }
        batch.phase = Phase::Returned;
        Ok(())
    }
    /// Validate all fields before publication; Busy remains set until all writes finish.
    pub fn commit_native_records(&mut self, heap: &mut DomainHeap, loan: NativeRecordLoan) {
        let result = (|| -> Result<(), NativeRecordLoanError> {
            self.require_domain(heap)?;
            if loan.domain != self.domain {
                return Err(NativeRecordLoanError::WrongDomain);
            }
            let batch = self
                .native_records
                .batches
                .get_mut(&loan)
                .ok_or(NativeRecordLoanError::UnknownLoan)?;
            if batch.phase != Phase::Returned {
                return Err(NativeRecordLoanError::InvalidPhase);
            }
            let mut staged = std::mem::take(&mut batch.staged);
            let validation = validate_updates(self, heap, loan, &mut staged);
            if validation.is_err() {
                // Do not even drop staging/diagnostic objects after detecting
                // invalid writeback: an allocator's deallocation may take locks.
                fatal_writeback();
            }
            let count = self
                .native_records
                .batches
                .get(&loan)
                .unwrap()
                .entries
                .len();
            let mut start = 0;
            for index in 0..count {
                let entry = &self.native_records.batches.get(&loan).unwrap().entries[index];
                let end = start + entry.layout.fields().len();
                // No allocation, GC or reentry can invalidate the complete
                // target proofs between validation and scalar publication.
                let publication = heap.write_integer_fields(
                    entry.reference.unwrap(),
                    2,
                    staged[start..end].iter().map(|value| value.bits()),
                );
                if publication.is_err() {
                    fatal_writeback();
                }
                start = end;
            }
            let batch = self.native_records.batches.get_mut(&loan).unwrap();
            for entry in &mut batch.entries {
                entry.reference = None;
            }
            batch.staged = staged;
            batch.phase = Phase::Committed;
            Ok(())
        })();
        if result.is_err() {
            fatal_writeback();
        }
    }
    fn finish_record_batch(
        &mut self,
        heap: &mut DomainHeap,
        loan: NativeRecordLoan,
    ) -> Result<(), NativeRecordLoanError> {
        let mut batch = self
            .native_records
            .batches
            .remove(&loan)
            .ok_or(NativeRecordLoanError::UnknownLoan)?;
        for entry in &mut batch.entries {
            if let Some(root) = entry.root.take() {
                heap.release_root(root).map_err(DataError::from)?;
            }
        }
        self.native_records.cache(batch);
        Ok(())
    }
    pub fn release_native_records(
        &mut self,
        heap: &mut DomainHeap,
        loan: NativeRecordLoan,
    ) -> Result<(), NativeRecordLoanError> {
        self.require_domain(heap)?;
        if loan.domain != self.domain {
            return Err(NativeRecordLoanError::WrongDomain);
        }
        let phase = self
            .native_records
            .batches
            .get(&loan)
            .ok_or(NativeRecordLoanError::UnknownLoan)?
            .phase;
        if matches!(phase, Phase::Entered | Phase::Returned) {
            fatal_writeback();
        }
        if phase != Phase::Committed {
            return Err(NativeRecordLoanError::InvalidPhase);
        }
        self.finish_record_batch(heap, loan)
    }
    pub(super) fn restore_native_record_pointer(
        &mut self,
        heap: &mut DomainHeap,
        layout: &NativeRecordLayout,
        pointer: *const u8,
    ) -> Result<Option<ManagedHandle>, NativeRecordError> {
        let known = self.native_records.batches.iter().find_map(|batch| {
            batch
                .entries
                .iter()
                .enumerate()
                .find(|(index, entry)| {
                    batch.pointer(*index) == pointer && entry.layout.as_ref() == layout
                })
                .map(|(_, entry)| entry.root.unwrap())
        });
        match known {
            None => Ok(None),
            Some(root) => {
                let reference = heap
                    .root_target(root)
                    .map_err(DataError::from)?
                    .ok_or(DataError::InvalidEncoding)?;
                Ok(Some(self.root(
                    heap,
                    reference,
                    DataKind::Composite(layout.ty),
                )?))
            }
        }
    }
    pub(super) fn require_native_record_readable(
        &self,
        heap: &DomainHeap,
        reference: HeapRef,
    ) -> Result<(), DataError> {
        if !self.native_records.has_loans() {
            return Ok(());
        }
        let identity = NativeRecordIdentity {
            domain: self.domain,
            object: heap.header(reference)?.object_id,
        };
        if self.native_records.busy(identity) {
            return Err(DataError::Thrown(crate::error_value::ErrorValue::new(
                4,
                "native record is already in use",
            )));
        }
        Ok(())
    }
    pub fn native_record_loan_stats(&self) -> NativeRecordLoanStats {
        NativeRecordLoanStats {
            calls: self.native_records.batches.iter().count(),
            targets: self
                .native_records
                .batches
                .iter()
                .map(|batch| batch.entries.len())
                .sum(),
            bytes: self
                .native_records
                .batches
                .iter()
                .flat_map(|batch| &batch.entries)
                .map(|entry| entry.layout.size())
                .sum(),
        }
    }
    pub fn native_record_scratch_stats(&self) -> NativeRecordScratchStats {
        NativeRecordScratchStats {
            retained_bytes: self.native_records.retained,
            cached_batches: self.native_records.pool.iter().count(),
            active_bytes: self.native_records.batches.iter().map(Batch::bytes).sum(),
            temporary_allocations: self.native_records.allocations,
        }
    }
    /// Called after all source argument expressions have completed. Preflight the
    /// complete set, then snapshot and acquire each distinct target exactly once.
    pub fn prepare_native_records(
        &mut self,
        heap: &mut DomainHeap,
        arguments: &[(Arc<NativeRecordLayout>, ManagedHandle)],
    ) -> Result<NativeRecordLoan, NativeRecordLoanError> {
        self.prepare_native_records_iter(
            heap,
            arguments.iter().map(|(layout, handle)| (layout, *handle)),
        )
    }
    /// A restartable view of already evaluated arguments avoids caller-owned
    /// record/alias staging allocations. Clones must enumerate the same inputs.
    pub fn prepare_native_records_iter<'a>(
        &mut self,
        heap: &mut DomainHeap,
        arguments: impl Iterator<Item = (&'a Arc<NativeRecordLayout>, ManagedHandle)> + Clone,
    ) -> Result<NativeRecordLoan, NativeRecordLoanError> {
        self.require_domain(heap)?;
        let argument_count = arguments.clone().count();
        if argument_count == 0 {
            return Err(NativeRecordError::InvalidLayout.into());
        }
        // Busy and invalid target checks precede even cold scratch allocation.
        // A rejected reloan cannot be changed into an OOM by its preparation.
        let mut checked = [None; 4];
        for (layout, handle) in arguments.clone() {
            let identity = checked_target(self, heap, layout, handle, &mut checked)?.identity;
            if self.native_records.busy(identity) {
                return Err(NativeRecordLoanError::Busy);
            }
        }
        let mut batch = self.native_records.take_scratch();
        let result =
            (|| -> Result<(), NativeRecordLoanError> {
                self.native_records
                    .reserve(&mut batch.arguments, argument_count);
                let mut size = 0usize;
                let mut alignment = 1;
                let mut fields = 0usize;
                for (layout, handle) in arguments {
                    let proof = checked_target(self, heap, layout, handle, &mut checked)?;
                    let identity = proof.identity;
                    if self.native_records.busy(identity) {
                        return Err(NativeRecordLoanError::Busy);
                    }
                    let index = if let Some(index) = batch
                        .entries
                        .iter()
                        .position(|entry| entry.identity == identity)
                    {
                        if !Arc::ptr_eq(&batch.entries[index].layout, layout)
                            && batch.entries[index].layout.as_ref() != layout.as_ref()
                        {
                            return Err(NativeRecordError::InvalidLayout.into());
                        }
                        index
                    } else {
                        let index = batch.entries.len();
                        self.native_records.reserve(&mut batch.entries, index + 1);
                        let padding =
                            (layout.alignment() - size % layout.alignment()) % layout.alignment();
                        let offset = size
                            .checked_add(padding)
                            .unwrap_or_else(|| crate::native_fatal::resource());
                        size = offset
                            .checked_add(layout.size())
                            .unwrap_or_else(|| crate::native_fatal::resource());
                        alignment = alignment.max(layout.alignment());
                        fields = fields
                            .checked_add(layout.fields().len())
                            .unwrap_or_else(|| crate::native_fatal::resource());
                        batch.entries.push(Entry {
                            identity,
                            layout: Arc::clone(layout),
                            reference: Some(proof.reference),
                            root: None,
                            offset,
                        });
                        index
                    };
                    batch.arguments.push(index);
                }
                self.native_records.reserve(&mut batch.staged, fields);
                if !batch.buffer.as_ref().is_some_and(|buffer| {
                    buffer.capacity() >= size && buffer.alignment() >= alignment
                }) {
                    self.native_records.allocated();
                    batch.buffer = Some(NativeRecordBuffer::allocate(size, alignment));
                }
                for entry in &mut batch.entries {
                    let start = batch.staged.len();
                    let reference = entry.reference.unwrap();
                    // Only system allocation follows preflight; the checked
                    // object cannot move or change before this scalar copy.
                    let fields = heap
                        .object_view(reference)
                        .map_err(DataError::from)?
                        .fields()
                        .map_err(DataError::from)?
                        .skip(2);
                    for (field, native) in fields.zip(entry.layout.fields()) {
                        let HeapValue::Integer(bits) = field else {
                            return Err(DataError::InvalidEncoding.into());
                        };
                        let value = crate::scalar::ScalarValue::from_bits(native.kind, bits as u64)
                            .map_err(|_| DataError::InvalidEncoding)?;
                        batch.staged.push(value);
                    }
                    batch.buffer.as_mut().unwrap().write(
                        entry.offset,
                        &entry.layout,
                        &batch.staged[start..],
                    );
                    entry.root = Some(heap.create_root(Some(reference)).map_err(DataError::from)?);
                }
                Ok(())
            })();
        if let Err(error) = result {
            for entry in &mut batch.entries {
                if let Some(root) = entry.root.take() {
                    heap.release_root(root).map_err(DataError::from)?;
                }
            }
            self.native_records.cache(batch);
            return Err(error);
        }
        self.native_records.serial = self
            .native_records
            .serial
            .checked_add(1)
            .unwrap_or_else(|| crate::native_fatal::resource());
        let loan = NativeRecordLoan {
            domain: self.domain,
            serial: self.native_records.serial,
        };
        batch.serial = loan.serial;
        batch.phase = Phase::Prepared;
        self.native_records.batches.insert(batch);
        Ok(loan)
    }
    pub fn native_record_argument_pointer(
        &self,
        heap: &DomainHeap,
        loan: NativeRecordLoan,
        argument: usize,
    ) -> Result<*mut u8, NativeRecordLoanError> {
        self.require_domain(heap)?;
        if loan.domain != self.domain {
            return Err(NativeRecordLoanError::WrongDomain);
        }
        let batch = self
            .native_records
            .batches
            .get(&loan)
            .ok_or(NativeRecordLoanError::UnknownLoan)?;
        let index = *batch
            .arguments
            .get(argument)
            .ok_or(NativeRecordLoanError::UnknownLoan)?;
        Ok(batch.pointer(index).cast_mut())
    }
    pub fn discard_prepared_native_records(
        &mut self,
        heap: &mut DomainHeap,
        loan: NativeRecordLoan,
    ) -> Result<(), NativeRecordLoanError> {
        self.require_domain(heap)?;
        if loan.domain != self.domain {
            return Err(NativeRecordLoanError::WrongDomain);
        }
        if self
            .native_records
            .batches
            .get(&loan)
            .ok_or(NativeRecordLoanError::UnknownLoan)?
            .phase
            != Phase::Prepared
        {
            return Err(NativeRecordLoanError::InvalidPhase);
        }
        self.finish_record_batch(heap, loan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scalar::{ScalarKind, ScalarValue};
    use crate::value::{ValueSchema, ValueType};

    #[test]
    fn invalid_later_target_does_not_publish_an_earlier_valid_target() {
        let mut heap = DomainHeap::new(912);
        let mut data = ManagedData::new(912);
        data.configure_values(
            &heap,
            Arc::new(
                ValueSchema::new(vec![
                    ValueType::Scalar(ScalarKind::Bool),
                    ValueType::State {
                        identity: "Flag".into(),
                        fields: vec![0],
                    },
                ])
                .unwrap(),
            ),
        )
        .unwrap();
        let layout = Arc::new(
            NativeRecordLayout::new(
                1,
                1,
                1,
                vec![NativeRecordField {
                    kind: ScalarKind::Bool,
                    offset: 0,
                }],
            )
            .unwrap(),
        );
        let mut create = || {
            data.create_composite(
                &mut heap,
                1,
                None,
                &[ManagedSlot::Scalar(
                    ScalarValue::from_bits(ScalarKind::Bool, 0).unwrap(),
                )],
            )
            .unwrap()
        };
        let first = create();
        let second = create();
        let loan = data
            .prepare_native_records(&mut heap, &[(Arc::clone(&layout), first), (layout, second)])
            .unwrap();
        let a = data.native_record_argument_pointer(&heap, loan, 0).unwrap();
        let b = data.native_record_argument_pointer(&heap, loan, 1).unwrap();
        data.enter_native_records(&heap, loan).unwrap();
        // SAFETY: only the loan-owned C buffers are modified, never Rust bools.
        unsafe {
            a.write(1);
            b.write(2);
        }
        data.return_native_records(&heap, loan).unwrap();
        assert_eq!(
            validate_updates(&mut data, &heap, loan, &mut Vec::with_capacity(2)),
            Err(NativeRecordLoanError::Record(NativeRecordError::Conversion))
        );
        let reference = data
            .reference(&heap, first, DataKind::Composite(1))
            .unwrap();
        assert_eq!(heap.integer_field(reference, 2).unwrap(), 0);
        // SAFETY: the loan still owns this C buffer; repair its byte encoding
        // before committing the test fixture's otherwise fatal writeback.
        unsafe {
            b.write(0);
        }
        data.commit_native_records(&mut heap, loan);
        assert_eq!(heap.integer_field(reference, 2).unwrap(), 1);
        data.release_native_records(&mut heap, loan).unwrap();
    }
}

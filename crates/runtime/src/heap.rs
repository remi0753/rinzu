//! Domain-local stop-the-Domain moving heap used by the baseline runtime.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, Weak};
use std::thread::ThreadId;

use crate::StableHandle;

pub const OBJECT_ALIGNMENT: usize = align_of::<u64>();
const HEADER_WORDS: usize = 3;
const FIELD_WORDS: usize = 2;
const TAG_NULL: u64 = 0;
const TAG_INTEGER: u64 = 1;
const TAG_REFERENCE: u64 = 2;
const FLAG_NATIVE_FINALIZER: u64 = 1;
const PACKED_INTEGERS: u64 = 1 << 50;
const GENERATION_YOUNG: u64 = 0;
const GENERATION_OLD: u64 = 1;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HeapRef {
    epoch: u64,
    offset_words: usize,
}

impl HeapRef {
    #[must_use]
    pub const fn epoch(self) -> u64 {
        self.epoch
    }

    #[must_use]
    pub const fn offset_bytes(self) -> usize {
        self.offset_words * size_of::<u64>()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeapValue {
    Null,
    Integer(i64),
    Reference(HeapRef),
}

/// Private, validated readonly words. The immutable heap loan prevents moving
/// GC/writes; the Owner marker prevents sending that loan to another Domain.
#[derive(Clone)]
pub(crate) struct HeapFields<'a> {
    words: std::slice::ChunksExact<'a, u64>,
    epoch: u64,
    _owner: std::marker::PhantomData<std::rc::Rc<()>>,
}
/// A checked object borrowed from one heap epoch. The immutable borrow prevents
/// collection or mutation until its fields have been consumed.
pub(crate) struct HeapObjectView<'a> {
    heap: &'a DomainHeap,
    reference: HeapRef,
    pub header: ObjectHeader,
    _owner: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl<'a> HeapObjectView<'a> {
    pub fn fields(self) -> Result<HeapFields<'a>, HeapError> {
        self.heap.fields_from_header(self.reference, self.header)
    }
}
impl Iterator for HeapFields<'_> {
    type Item = HeapValue;
    fn next(&mut self) -> Option<Self::Item> {
        let words = self.words.next()?;
        if let [word] = words {
            return Some(HeapValue::Integer(*word as i64));
        }
        Some(match words[0] {
            TAG_NULL => HeapValue::Null,
            TAG_INTEGER => HeapValue::Integer(words[1] as i64),
            TAG_REFERENCE => HeapValue::Reference(HeapRef {
                epoch: self.epoch,
                offset_words: words[1] as usize,
            }),
            _ => unreachable!("field tags validated before borrowing"),
        })
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        if n != 0 {
            self.words.nth(n - 1)?;
        }
        self.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.words.size_hint()
    }
}
impl ExactSizeIterator for HeapFields<'_> {}
impl std::iter::FusedIterator for HeapFields<'_> {}
impl DoubleEndedIterator for HeapFields<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let words = self.words.next_back()?;
        if let [word] = words {
            return Some(HeapValue::Integer(*word as i64));
        }
        Some(match words[0] {
            TAG_NULL => HeapValue::Null,
            TAG_INTEGER => HeapValue::Integer(words[1] as i64),
            TAG_REFERENCE => HeapValue::Reference(HeapRef {
                epoch: self.epoch,
                offset_words: words[1] as usize,
            }),
            _ => unreachable!("field tags validated before borrowing"),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinalizerPolicy {
    None,
    NativeOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectionKind {
    Minor,
    Full,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectHeader {
    pub size_bytes: u32,
    pub type_id: u32,
    pub object_id: u64,
    pub field_count: u32,
    pub age: u16,
    pub is_old: bool,
    pub finalizer: FinalizerPolicy,
    packed: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RootSlot {
    index: usize,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WeakHandle {
    index: usize,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeFinalizer {
    pub object_id: u64,
    pub type_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionReport {
    pub kind: CollectionKind,
    pub moved_objects: usize,
    pub reclaimed_objects: usize,
    pub promoted_objects: usize,
    pub cleared_weak_handles: usize,
    /// These records may trigger native-only release. Managed callbacks are
    /// never run from GC finalization.
    pub native_finalizers: Vec<NativeFinalizer>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeapError {
    WrongThread {
        expected: ThreadId,
        actual: ThreadId,
    },
    StaleReference(HeapRef),
    InvalidObject(HeapRef),
    UnknownRoot(RootSlot),
    UnknownWeakHandle(WeakHandle),
    ForeignReference,
    InvalidStableHandle,
    AddressSpaceExhausted,
}

impl fmt::Display for HeapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongThread { expected, actual } => {
                write!(
                    formatter,
                    "heap owner is {expected:?}, accessed from {actual:?}"
                )
            }
            Self::StaleReference(reference) => write!(
                formatter,
                "heap reference from epoch {} is stale",
                reference.epoch
            ),
            Self::InvalidObject(reference) => {
                write!(
                    formatter,
                    "invalid heap object at byte {}",
                    reference.offset_bytes()
                )
            }
            Self::UnknownRoot(root) => write!(formatter, "unknown root slot {root:?}"),
            Self::UnknownWeakHandle(handle) => {
                write!(formatter, "unknown weak handle {handle:?}")
            }
            Self::ForeignReference => {
                formatter.write_str("reference belongs to another heap epoch")
            }
            Self::InvalidStableHandle => formatter.write_str("invalid stable handle"),
            Self::AddressSpaceExhausted => {
                formatter.write_str("managed heap address space exhausted")
            }
        }
    }
}

impl std::error::Error for HeapError {}

#[derive(Debug)]
struct RootRecord {
    generation: u64,
    target: Option<HeapRef>,
    active: bool,
}

#[derive(Debug)]
struct WeakRecord {
    generation: u64,
    target: Option<HeapRef>,
    active: bool,
}

#[derive(Debug)]
pub(crate) struct StableTarget {
    domain_id: u64,
    generation: u64,
    target: Mutex<Option<HeapRef>>,
}

impl StableTarget {
    #[cfg(test)]
    pub(crate) fn empty_for_abi_test() -> Arc<Self> {
        Arc::new(Self {
            domain_id: 0,
            generation: 0,
            target: Mutex::new(None),
        })
    }
}

/// A compact semispace. References are byte-aligned offsets plus a collection
/// epoch; only registered roots, object fields, and stable handles are rewritten.
#[derive(Debug)]
pub struct DomainHeap {
    domain_id: u64,
    owner_thread: ThreadId,
    epoch: u64,
    words: Vec<u64>,
    object_offsets: Vec<usize>,
    roots: Vec<RootRecord>,
    free_roots: Vec<usize>,
    weak_handles: Vec<WeakRecord>,
    stable_handles: Vec<Weak<StableTarget>>,
    // Weak targets keep diagnostics only while their Error graph is alive.
    error_traces: std::collections::BTreeMap<u64, (HeapRef, Arc<crate::error_trace::ErrorTrace>)>,
    next_object_id: u64,
    next_handle_generation: u64,
    accounting: bool,
    gc: crate::gc::GcPolicy,
    #[cfg(test)]
    pub(crate) copied_fields: usize,
}

impl DomainHeap {
    pub(crate) fn first_error_trace(
        &mut self,
        target: HeapRef,
        trace: Arc<crate::error_trace::ErrorTrace>,
    ) -> Result<Arc<crate::error_trace::ErrorTrace>, HeapError> {
        let object = self.header(target)?.object_id;
        Ok(Arc::clone(
            &self.error_traces.entry(object).or_insert((target, trace)).1,
        ))
    }
    pub(crate) fn error_trace(
        &self,
        target: HeapRef,
    ) -> Result<Option<Arc<crate::error_trace::ErrorTrace>>, HeapError> {
        let object = self.header(target)?.object_id;
        Ok(self
            .error_traces
            .get(&object)
            .map(|(_, trace)| Arc::clone(trace)))
    }

    #[must_use]
    pub fn new(domain_id: u64) -> Self {
        Self {
            domain_id,
            owner_thread: std::thread::current().id(),
            epoch: 1,
            words: Vec::new(),
            object_offsets: Vec::new(),
            roots: Vec::new(),
            free_roots: Vec::new(),
            weak_handles: Vec::new(),
            stable_handles: Vec::new(),
            error_traces: Default::default(),
            next_object_id: 1,
            next_handle_generation: 1,
            accounting: crate::resource_accounting::enabled(),
            gc: crate::gc::GcPolicy::default(),
            #[cfg(test)]
            copied_fields: 0,
        }
    }

    #[must_use]
    pub const fn domain_id(&self) -> u64 {
        self.domain_id
    }

    #[must_use]
    pub fn owner_thread(&self) -> ThreadId {
        self.owner_thread
    }

    #[must_use]
    pub fn allocated_bytes(&self) -> usize {
        self.words.len() * size_of::<u64>()
    }

    #[must_use]
    pub fn object_count(&self) -> usize {
        self.object_offsets.len()
    }

    /// Allocation never collects an unrooted result or moves a native borrow.
    /// Query only after publishing the current typed roots.
    #[must_use]
    pub fn collection_due(&self) -> bool {
        self.gc.collection_due()
    }

    pub fn enable_gc_stress(&mut self) {
        self.gc.enable_stress();
    }

    #[must_use]
    pub fn gc_stress(&self) -> bool {
        self.gc.is_stress()
    }

    pub fn allocate(
        &mut self,
        type_id: u32,
        fields: &[HeapValue],
        finalizer: FinalizerPolicy,
    ) -> Result<HeapRef, HeapError> {
        self.require_owner()?;
        for field in fields {
            if let HeapValue::Reference(reference) = field {
                self.validate_reference(*reference)?;
            }
        }
        let (reference, size_bytes) =
            self.begin_allocation(type_id, fields.len(), finalizer, false)?;
        for field in fields {
            let (tag, payload) = Self::encode_value(*field);
            self.words.push(tag);
            self.words.push(payload);
        }
        self.finish_allocation(reference, size_bytes);
        Ok(reference)
    }

    /// Reserve fields for a private result. Null tags keep unwritten slots
    /// inert. The caller fills the result before its next safepoint.
    pub(crate) fn allocate_null_fields(
        &mut self,
        type_id: u32,
        count: usize,
    ) -> Result<HeapRef, HeapError> {
        self.require_owner()?;
        let (reference, size) =
            self.begin_allocation(type_id, count, FinalizerPolicy::None, false)?;
        self.words
            .resize(reference.offset_words + size as usize / size_of::<u64>(), 0);
        self.finish_allocation(reference, size);
        Ok(reference)
    }

    /// Copy already checked fields within this Domain. As with write_field,
    /// the collector sees these edges by tracing the complete rooted graph.
    pub(crate) fn copy_fields(
        &mut self,
        source: HeapRef,
        start: usize,
        target: HeapRef,
        offset: usize,
        count: usize,
    ) -> Result<(), HeapError> {
        let end = start
            .checked_add(count)
            .ok_or(HeapError::AddressSpaceExhausted)?;
        let source_header = self.header(source)?;
        if source_header.packed || end > source_header.field_count as usize {
            return Err(HeapError::InvalidObject(source));
        }
        let target_end = offset
            .checked_add(count)
            .ok_or(HeapError::AddressSpaceExhausted)?;
        let target_header = self.header(target)?;
        if target_header.packed || target_end > target_header.field_count as usize {
            return Err(HeapError::InvalidObject(target));
        }
        let source_start = source.offset_words + HEADER_WORDS;
        let target_start = target.offset_words + HEADER_WORDS;
        self.words.copy_within(
            source_start + start * FIELD_WORDS..source_start + end * FIELD_WORDS,
            target_start + offset * FIELD_WORDS,
        );
        #[cfg(test)]
        {
            self.copied_fields += count;
        }
        Ok(())
    }

    /// Packed String/Bytes words cannot contain references. Stream them into
    /// the heap without an intermediate Vec<HeapValue> or a reference scan.
    pub(crate) fn allocate_integers(
        &mut self,
        type_id: u32,
        fields: impl ExactSizeIterator<Item = i64>,
    ) -> Result<HeapRef, HeapError> {
        self.require_owner()?;
        let (reference, size_bytes) =
            self.begin_allocation(type_id, fields.len(), FinalizerPolicy::None, true)?;
        for value in fields {
            self.words.push(value as u64);
        }
        // ExactSizeIterator is a safe trait. Never publish malformed heap
        // metadata if a private producer accidentally reports the wrong count.
        if self.words.len() - reference.offset_words != size_bytes as usize / size_of::<u64>() {
            self.words.truncate(reference.offset_words);
            return Err(HeapError::InvalidObject(reference));
        }
        self.finish_allocation(reference, size_bytes);
        Ok(reference)
    }

    #[inline]
    fn begin_allocation(
        &mut self,
        type_id: u32,
        count: usize,
        finalizer: FinalizerPolicy,
        packed: bool,
    ) -> Result<(HeapRef, u32), HeapError> {
        let field_words = count
            .checked_mul(if packed { 1 } else { FIELD_WORDS })
            .ok_or(HeapError::AddressSpaceExhausted)?;
        let object_words = HEADER_WORDS
            .checked_add(field_words)
            .ok_or(HeapError::AddressSpaceExhausted)?;
        let size_bytes = object_words
            .checked_mul(size_of::<u64>())
            .and_then(|size| u32::try_from(size).ok())
            .ok_or(HeapError::AddressSpaceExhausted)?;
        let field_count = u32::try_from(count).map_err(|_| HeapError::AddressSpaceExhausted)?;
        let offset_words = self.words.len();
        let object_id = self.next_object_id;
        self.next_object_id = self
            .next_object_id
            .checked_add(1)
            .ok_or(HeapError::AddressSpaceExhausted)?;
        self.words.reserve(object_words);
        self.words
            .push((u64::from(type_id) << 32) | u64::from(size_bytes));
        self.words.push(object_id);
        self.words.push(
            Self::encode_metadata(field_count, 0, GENERATION_YOUNG, finalizer)
                | if packed { PACKED_INTEGERS } else { 0 },
        );
        Ok((
            HeapRef {
                epoch: self.epoch,
                offset_words,
            },
            size_bytes,
        ))
    }

    #[inline]
    fn finish_allocation(&mut self, reference: HeapRef, size_bytes: u32) {
        self.object_offsets.push(reference.offset_words);
        self.gc.allocated(size_bytes as usize);
        if self.accounting {
            crate::resource_accounting::acquire_recorded(
                crate::resource_accounting::Kind::Managed,
                u64::from(size_bytes),
            );
            crate::resource_accounting::acquire_recorded(
                crate::resource_accounting::Kind::ManagedObjects,
                1,
            );
        }
    }

    pub fn create_root(&mut self, target: Option<HeapRef>) -> Result<RootSlot, HeapError> {
        self.require_owner()?;
        self.validate_optional_reference(target)?;
        let (index, generation) = if let Some(index) = self.free_roots.pop() {
            let record = &mut self.roots[index];
            debug_assert!(!record.active);
            record.target = target;
            record.active = true;
            (index, record.generation)
        } else {
            // Reserve during creation so releasing a root after native return
            // never allocates. Released slots can all fit in this free list.
            self.free_roots
                .reserve(self.roots.len() + 1 - self.free_roots.len());
            let index = self.roots.len();
            self.roots.push(RootRecord {
                generation: 1,
                target,
                active: true,
            });
            (index, 1)
        };
        Ok(RootSlot { index, generation })
    }

    pub fn set_root(&mut self, root: RootSlot, target: Option<HeapRef>) -> Result<(), HeapError> {
        self.require_owner()?;
        self.validate_optional_reference(target)?;
        let record = self.root_record_mut(root)?;
        record.target = target;
        Ok(())
    }

    pub fn root_target(&self, root: RootSlot) -> Result<Option<HeapRef>, HeapError> {
        self.require_owner()?;
        Ok(self.root_record(root)?.target)
    }

    pub fn release_root(&mut self, root: RootSlot) -> Result<(), HeapError> {
        self.require_owner()?;
        let record = self.root_record_mut(root)?;
        record.active = false;
        record.target = None;
        if let Some(generation) = record.generation.checked_add(1) {
            record.generation = generation;
            self.free_roots.push(root.index);
        }
        Ok(())
    }

    pub fn create_weak(&mut self, target: HeapRef) -> Result<WeakHandle, HeapError> {
        self.require_owner()?;
        self.validate_reference(target)?;
        let index = self.weak_handles.len();
        let generation = 1;
        self.weak_handles.push(WeakRecord {
            generation,
            target: Some(target),
            active: true,
        });
        Ok(WeakHandle { index, generation })
    }

    pub fn resolve_weak(&self, handle: WeakHandle) -> Result<Option<HeapRef>, HeapError> {
        self.require_owner()?;
        let record = self
            .weak_handles
            .get(handle.index)
            .filter(|record| record.active && record.generation == handle.generation)
            .ok_or(HeapError::UnknownWeakHandle(handle))?;
        Ok(record.target)
    }

    pub fn release_weak(&mut self, handle: WeakHandle) -> Result<(), HeapError> {
        self.require_owner()?;
        let record = self
            .weak_handles
            .get_mut(handle.index)
            .filter(|record| record.active && record.generation == handle.generation)
            .ok_or(HeapError::UnknownWeakHandle(handle))?;
        record.active = false;
        record.target = None;
        record.generation = record.generation.saturating_add(1);
        Ok(())
    }

    pub fn create_stable_handle(
        &mut self,
        target: HeapRef,
    ) -> Result<*mut StableHandle, HeapError> {
        self.require_owner()?;
        self.validate_reference(target)?;
        let generation = self.next_handle_generation;
        self.next_handle_generation = self
            .next_handle_generation
            .checked_add(1)
            .ok_or(HeapError::AddressSpaceExhausted)?;
        let stable_target = Arc::new(StableTarget {
            domain_id: self.domain_id,
            generation,
            target: Mutex::new(Some(target)),
        });
        self.stable_handles.push(Arc::downgrade(&stable_target));
        Ok(StableHandle::from_target(stable_target))
    }

    /// Resolves an opaque C stable handle on its Domain owner thread.
    ///
    /// # Safety
    ///
    /// `handle` must identify a live owned or borrowed stable-handle reference.
    pub unsafe fn resolve_stable_handle(
        &self,
        handle: *const StableHandle,
    ) -> Result<Option<HeapRef>, HeapError> {
        self.require_owner()?;
        // SAFETY: the caller promises a live stable-handle pointer.
        let handle = unsafe { handle.as_ref() }.ok_or(HeapError::InvalidStableHandle)?;
        if handle.target.domain_id != self.domain_id {
            return Err(HeapError::InvalidStableHandle);
        }
        Ok(*handle
            .target
            .target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Returns the generation native code must pair with this opaque handle.
    ///
    /// # Safety
    ///
    /// `handle` must identify a live owned or borrowed stable-handle reference.
    pub unsafe fn stable_handle_generation(
        &self,
        handle: *const StableHandle,
    ) -> Result<u64, HeapError> {
        self.require_owner()?;
        // SAFETY: the caller promises a live stable-handle pointer.
        let handle = unsafe { handle.as_ref() }.ok_or(HeapError::InvalidStableHandle)?;
        if handle.target.domain_id != self.domain_id {
            return Err(HeapError::InvalidStableHandle);
        }
        Ok(handle.target.generation)
    }

    pub fn header(&self, reference: HeapRef) -> Result<ObjectHeader, HeapError> {
        self.require_owner()?;
        self.validate_reference(reference)?;
        self.decode_header(&self.words, reference.offset_words)
    }

    pub fn fields(&self, reference: HeapRef) -> Result<Vec<HeapValue>, HeapError> {
        Ok(self.fields_iter(reference)?.collect())
    }

    pub(crate) fn packed_words(&self, reference: HeapRef) -> Result<&[u64], HeapError> {
        let header = self.header(reference)?;
        if !header.packed {
            return Err(HeapError::InvalidObject(reference));
        }
        let start = reference.offset_words + HEADER_WORDS;
        self.words
            .get(start..start + header.field_count as usize)
            .ok_or(HeapError::InvalidObject(reference))
    }

    /// Clone only the requested payload range. Reserve before borrowing
    /// either address, since reservation may move the entire semispace.
    pub(crate) fn copy_packed_bytes(
        &mut self,
        type_id: u32,
        source: HeapRef,
        range: std::ops::Range<usize>,
    ) -> Result<HeapRef, HeapError> {
        let words = self.packed_words(source)?;
        let length = usize::try_from(*words.first().ok_or(HeapError::InvalidObject(source))?)
            .map_err(|_| HeapError::InvalidObject(source))?;
        if range.end > length || range.start > range.end || words.len() != 1 + length.div_ceil(8) {
            return Err(HeapError::InvalidObject(source));
        }
        let n = range.len();
        let (target, size) =
            self.begin_allocation(type_id, 1 + n.div_ceil(8), FinalizerPolicy::None, true)?;
        self.words.push(n as u64);
        self.words
            .resize(target.offset_words + size as usize / 8, 0);
        let from = source.offset_words + HEADER_WORDS + 1;
        let to = target.offset_words + HEADER_WORDS + 1;
        let base = self.words.as_mut_ptr();
        // SAFETY: checked range lies in source's packed payload. The new
        // object's payload is initialized, disjoint, and sized for n bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(
                base.add(from).cast::<u8>().add(range.start),
                base.add(to).cast::<u8>(),
                n,
            );
        }
        self.finish_allocation(target, size);
        Ok(target)
    }

    pub(crate) fn fields_iter(&self, reference: HeapRef) -> Result<HeapFields<'_>, HeapError> {
        let header = self.header(reference)?;
        self.fields_from_header(reference, header)
    }

    pub(crate) fn object_view(&self, reference: HeapRef) -> Result<HeapObjectView<'_>, HeapError> {
        Ok(HeapObjectView {
            heap: self,
            reference,
            header: self.header(reference)?,
            _owner: std::marker::PhantomData,
        })
    }

    fn fields_from_header(
        &self,
        reference: HeapRef,
        header: ObjectHeader,
    ) -> Result<HeapFields<'_>, HeapError> {
        let start = reference
            .offset_words
            .checked_add(HEADER_WORDS)
            .ok_or(HeapError::AddressSpaceExhausted)?;
        let stride = if header.packed { 1 } else { FIELD_WORDS };
        let count = (header.field_count as usize)
            .checked_mul(stride)
            .ok_or(HeapError::AddressSpaceExhausted)?;
        let end = start
            .checked_add(count)
            .ok_or(HeapError::AddressSpaceExhausted)?;
        let words = self
            .words
            .get(start..end)
            .ok_or(HeapError::InvalidObject(reference))?;
        // Keep decode_fields' full validation before callers inspect metadata.
        // A malformed later tag must not be hidden by an early length error.
        for field in
            words
                .chunks_exact(FIELD_WORDS)
                .take(if header.packed { 0 } else { usize::MAX })
        {
            match field[0] {
                TAG_NULL | TAG_INTEGER => {}
                TAG_REFERENCE => {
                    usize::try_from(field[1]).map_err(|_| HeapError::AddressSpaceExhausted)?;
                }
                _ => return Err(HeapError::InvalidObject(reference)),
            }
        }
        Ok(HeapFields {
            words: words.chunks_exact(stride),
            epoch: self.epoch,
            _owner: std::marker::PhantomData,
        })
    }

    /// The single reference write barrier for Domain-local objects. The current
    /// collector traces the full rooted graph at each collection, including Minor.
    /// Validate new references here before publishing an edge; no concurrent scan.
    pub(crate) fn write_field(
        &mut self,
        reference: HeapRef,
        index: usize,
        value: HeapValue,
    ) -> Result<(), HeapError> {
        let header = self.header(reference)?;
        if header.packed || index >= header.field_count as usize {
            return Err(HeapError::InvalidObject(reference));
        }
        if let HeapValue::Reference(child) = value {
            self.validate_reference(child)?;
        }
        let offset = reference.offset_words + HEADER_WORDS + index * FIELD_WORDS;
        let (tag, payload) = Self::encode_value(value);
        self.words[offset] = tag;
        self.words[offset + 1] = payload;
        Ok(())
    }

    /// Publish a validated flat scalar range. Check the whole destination before
    /// writing; integer payloads never create GC edges or invoke a write barrier.
    pub(crate) fn write_integer_fields(
        &mut self,
        reference: HeapRef,
        start: usize,
        values: impl ExactSizeIterator<Item = u64>,
    ) -> Result<(), HeapError> {
        let header = self.header(reference)?;
        let end = start
            .checked_add(values.len())
            .ok_or(HeapError::AddressSpaceExhausted)?;
        if end > header.field_count as usize {
            return Err(HeapError::InvalidObject(reference));
        }
        if header.packed {
            let base = reference.offset_words + HEADER_WORDS;
            let fields = self
                .words
                .get_mut(base + start..base + end)
                .ok_or(HeapError::InvalidObject(reference))?;
            for (field, value) in fields.iter_mut().zip(values) {
                *field = value;
            }
            return Ok(());
        }
        let begin_words = reference.offset_words + HEADER_WORDS + start * FIELD_WORDS;
        let end_words = reference.offset_words + HEADER_WORDS + end * FIELD_WORDS;
        let fields = self
            .words
            .get_mut(begin_words..end_words)
            .ok_or(HeapError::InvalidObject(reference))?;
        if fields
            .chunks_exact(FIELD_WORDS)
            .any(|field| field[0] != TAG_INTEGER)
        {
            return Err(HeapError::InvalidObject(reference));
        }
        for (field, value) in fields.chunks_exact_mut(FIELD_WORDS).zip(values) {
            field[1] = value;
        }
        Ok(())
    }

    /// Private fixed integer metadata access does not allocate or trace an
    /// integer as a reference, even when its bits equal a managed identity.
    pub(crate) fn integer_field(&self, reference: HeapRef, index: usize) -> Result<i64, HeapError> {
        let header = self.header(reference)?;
        if index >= header.field_count as usize {
            return Err(HeapError::InvalidObject(reference));
        }
        if header.packed {
            return Ok(self.words[reference.offset_words + HEADER_WORDS + index] as i64);
        }
        let offset = reference.offset_words + HEADER_WORDS + index * FIELD_WORDS;
        if self.words[offset] != TAG_INTEGER {
            return Err(HeapError::InvalidObject(reference));
        }
        Ok(self.words[offset + 1] as i64)
    }

    /// A typed immutable projection validates only the selected slot. Whole
    /// object decoders still validate every tag, including malformed tails.
    pub(crate) fn field_at(
        &self,
        reference: HeapRef,
        index: usize,
    ) -> Result<HeapValue, HeapError> {
        let header = self.header(reference)?;
        if index >= header.field_count as usize {
            return Err(HeapError::InvalidObject(reference));
        }
        if header.packed {
            return Ok(HeapValue::Integer(
                self.words[reference.offset_words + HEADER_WORDS + index] as i64,
            ));
        }
        let offset = reference.offset_words + HEADER_WORDS + index * FIELD_WORDS;
        Ok(match self.words[offset] {
            TAG_NULL => HeapValue::Null,
            TAG_INTEGER => HeapValue::Integer(self.words[offset + 1] as i64),
            TAG_REFERENCE => HeapValue::Reference(HeapRef {
                epoch: self.epoch,
                offset_words: usize::try_from(self.words[offset + 1])
                    .map_err(|_| HeapError::AddressSpaceExhausted)?,
            }),
            _ => return Err(HeapError::InvalidObject(reference)),
        })
    }

    /// Used only to initialize fixed native-wrapper integer metadata after
    /// physical return; never mutate a reference tag or allocate new fields.
    pub(crate) fn set_integer_field(
        &mut self,
        reference: HeapRef,
        index: usize,
        value: i64,
    ) -> Result<(), HeapError> {
        if self.header(reference)?.packed {
            return Err(HeapError::InvalidObject(reference));
        }
        self.integer_field(reference, index)?;
        let offset = reference.offset_words + HEADER_WORDS + index * FIELD_WORDS + 1;
        self.words[offset] = value as u64;
        Ok(())
    }

    pub fn collect(&mut self, kind: CollectionKind) -> Result<CollectionReport, HeapError> {
        self.require_owner()?;
        let old_epoch = self.epoch;
        let old_offsets = self.object_offsets.iter().copied().collect::<HashSet<_>>();
        let mut reachable = HashSet::new();
        let mut live_words = 0_usize;
        let mut stack = Vec::new();
        for root in self.roots.iter().filter(|root| root.active) {
            if let Some(reference) = root.target {
                stack.push(reference);
            }
        }
        self.stable_handles
            .retain(|handle| handle.strong_count() > 0);
        for handle in &self.stable_handles {
            if let Some(handle) = handle.upgrade()
                && let Some(reference) = *handle
                    .target
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            {
                stack.push(reference);
            }
        }
        while let Some(reference) = stack.pop() {
            if reference.epoch != old_epoch || !old_offsets.contains(&reference.offset_words) {
                return Err(HeapError::ForeignReference);
            }
            if !reachable.insert(reference.offset_words) {
                continue;
            }
            let header = self.header(reference)?;
            live_words = live_words.saturating_add(header.size_bytes as usize / size_of::<u64>());
            if !header.packed {
                for field in self.fields_from_header(reference, header)? {
                    if let HeapValue::Reference(reference) = field {
                        stack.push(reference);
                    }
                }
            }
        }

        let new_epoch = self
            .epoch
            .checked_add(1)
            .ok_or(HeapError::AddressSpaceExhausted)?;
        let mut forwarding = HashMap::with_capacity(reachable.len());
        // Reachability already visited every live object's fields. Reserve its
        // encoded size once rather than recopying a growing destination Vec.
        // A malformed header cannot make this hint exceed the old heap size.
        let mut new_words = Vec::with_capacity(live_words.min(self.words.len()));
        let mut new_offsets = Vec::with_capacity(reachable.len());
        let mut promoted_objects = 0;
        let mut finalizers = Vec::new();

        for offset in &self.object_offsets {
            let header = self.decode_header(&self.words, *offset)?;
            if reachable.contains(offset) {
                let new_offset = new_words.len();
                let object_words = usize::try_from(header.size_bytes)
                    .expect("u32 object size fits usize")
                    / size_of::<u64>();
                // Copy straight into the destination semispace. A temporary
                // Vec here allocates and copies every surviving object twice.
                new_words.extend_from_slice(&self.words[*offset..*offset + object_words]);
                let new_age = header.age.saturating_add(1);
                let was_old = header.is_old;
                let is_old = was_old || new_age >= 2;
                if is_old && !was_old {
                    promoted_objects += 1;
                }
                new_words[new_offset + 2] = Self::encode_metadata(
                    header.field_count,
                    new_age,
                    if is_old {
                        GENERATION_OLD
                    } else {
                        GENERATION_YOUNG
                    },
                    header.finalizer,
                ) | if header.packed { PACKED_INTEGERS } else { 0 };
                new_offsets.push(new_offset);
                forwarding.insert(*offset, new_offset);
            } else if header.finalizer == FinalizerPolicy::NativeOnly {
                finalizers.push(NativeFinalizer {
                    object_id: header.object_id,
                    type_id: header.type_id,
                });
            }
        }

        for offset in &new_offsets {
            let header = self.decode_header(&new_words, *offset)?;
            if header.packed {
                continue;
            }
            for index in 0..usize::try_from(header.field_count).expect("u32 fits usize") {
                let tag_index = *offset + HEADER_WORDS + index * FIELD_WORDS;
                if new_words[tag_index] == TAG_REFERENCE {
                    let old_target = usize::try_from(new_words[tag_index + 1])
                        .map_err(|_| HeapError::AddressSpaceExhausted)?;
                    let new_target =
                        forwarding
                            .get(&old_target)
                            .copied()
                            .ok_or(HeapError::InvalidObject(HeapRef {
                                epoch: old_epoch,
                                offset_words: old_target,
                            }))?;
                    new_words[tag_index + 1] =
                        u64::try_from(new_target).map_err(|_| HeapError::AddressSpaceExhausted)?;
                }
            }
        }

        for root in self.roots.iter_mut().filter(|root| root.active) {
            root.target = Self::forward(root.target, old_epoch, new_epoch, &forwarding)?;
        }
        for handle in &self.stable_handles {
            if let Some(handle) = handle.upgrade() {
                let mut target = handle
                    .target
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *target = Self::forward(*target, old_epoch, new_epoch, &forwarding)?;
            }
        }
        let mut cleared_weak_handles = 0;
        for weak in self.weak_handles.iter_mut().filter(|weak| weak.active) {
            let old_target = weak.target;
            weak.target = Self::forward_weak(old_target, old_epoch, new_epoch, &forwarding)?;
            if old_target.is_some() && weak.target.is_none() {
                cleared_weak_handles += 1;
            }
        }
        let mut trace_error = None;
        self.error_traces.retain(|_, (target, _)| {
            match Self::forward_weak(Some(*target), old_epoch, new_epoch, &forwarding) {
                Ok(Some(forwarded)) => {
                    *target = forwarded;
                    true
                }
                Ok(None) => false,
                Err(error) => {
                    trace_error = Some(error);
                    false
                }
            }
        });
        if let Some(error) = trace_error {
            return Err(error);
        }

        let moved_objects = new_offsets.len();
        let reclaimed_objects = self.object_offsets.len() - moved_objects;
        if self.accounting {
            crate::resource_accounting::release(
                crate::resource_accounting::Kind::Managed,
                ((self.words.len() - new_words.len()) * size_of::<u64>()) as u64,
            );
            crate::resource_accounting::release(
                crate::resource_accounting::Kind::ManagedObjects,
                reclaimed_objects as u64,
            );
        }
        self.words = new_words;
        self.object_offsets = new_offsets;
        self.epoch = new_epoch;
        let root_bytes = self
            .roots
            .iter()
            .filter(|root| root.active)
            .count()
            .saturating_add(self.stable_handles.len())
            .saturating_mul(size_of::<u64>());
        self.gc.collected(self.allocated_bytes(), root_bytes);
        Ok(CollectionReport {
            kind,
            moved_objects,
            reclaimed_objects,
            promoted_objects,
            cleared_weak_handles,
            native_finalizers: finalizers,
        })
    }

    fn require_owner(&self) -> Result<(), HeapError> {
        let actual = std::thread::current().id();
        if actual == self.owner_thread {
            Ok(())
        } else {
            Err(HeapError::WrongThread {
                expected: self.owner_thread,
                actual,
            })
        }
    }

    fn validate_reference(&self, reference: HeapRef) -> Result<(), HeapError> {
        if reference.epoch != self.epoch {
            return Err(HeapError::StaleReference(reference));
        }
        if self
            .object_offsets
            .binary_search(&reference.offset_words)
            .is_err()
        {
            return Err(HeapError::InvalidObject(reference));
        }
        Ok(())
    }

    fn validate_optional_reference(&self, reference: Option<HeapRef>) -> Result<(), HeapError> {
        if let Some(reference) = reference {
            self.validate_reference(reference)?;
        }
        Ok(())
    }

    fn root_record(&self, root: RootSlot) -> Result<&RootRecord, HeapError> {
        self.roots
            .get(root.index)
            .filter(|record| record.active && record.generation == root.generation)
            .ok_or(HeapError::UnknownRoot(root))
    }

    fn root_record_mut(&mut self, root: RootSlot) -> Result<&mut RootRecord, HeapError> {
        self.roots
            .get_mut(root.index)
            .filter(|record| record.active && record.generation == root.generation)
            .ok_or(HeapError::UnknownRoot(root))
    }

    fn decode_header(&self, words: &[u64], offset: usize) -> Result<ObjectHeader, HeapError> {
        let Some(layout) = words.get(offset).copied() else {
            return Err(HeapError::InvalidObject(HeapRef {
                epoch: self.epoch,
                offset_words: offset,
            }));
        };
        let Some(object_id) = words.get(offset + 1).copied() else {
            return Err(HeapError::InvalidObject(HeapRef {
                epoch: self.epoch,
                offset_words: offset,
            }));
        };
        let Some(metadata) = words.get(offset + 2).copied() else {
            return Err(HeapError::InvalidObject(HeapRef {
                epoch: self.epoch,
                offset_words: offset,
            }));
        };
        let size_bytes = layout as u32;
        let type_id = (layout >> 32) as u32;
        let field_count = metadata as u32;
        let age = (metadata >> 32) as u16;
        let is_old = ((metadata >> 48) & 1) == GENERATION_OLD;
        let finalizer = if ((metadata >> 49) & FLAG_NATIVE_FINALIZER) != 0 {
            FinalizerPolicy::NativeOnly
        } else {
            FinalizerPolicy::None
        };
        Ok(ObjectHeader {
            size_bytes,
            type_id,
            object_id,
            field_count,
            age,
            is_old,
            finalizer,
            packed: metadata & PACKED_INTEGERS != 0,
        })
    }

    fn encode_metadata(
        field_count: u32,
        age: u16,
        generation: u64,
        finalizer: FinalizerPolicy,
    ) -> u64 {
        u64::from(field_count)
            | (u64::from(age) << 32)
            | (generation << 48)
            | (u64::from(finalizer == FinalizerPolicy::NativeOnly) << 49)
    }

    fn encode_value(value: HeapValue) -> (u64, u64) {
        match value {
            HeapValue::Null => (TAG_NULL, 0),
            HeapValue::Integer(value) => (TAG_INTEGER, value as u64),
            HeapValue::Reference(reference) => (
                TAG_REFERENCE,
                u64::try_from(reference.offset_words).expect("heap offset must fit u64"),
            ),
        }
    }

    fn forward(
        target: Option<HeapRef>,
        old_epoch: u64,
        new_epoch: u64,
        forwarding: &HashMap<usize, usize>,
    ) -> Result<Option<HeapRef>, HeapError> {
        let Some(target) = target else {
            return Ok(None);
        };
        if target.epoch != old_epoch {
            return Err(HeapError::ForeignReference);
        }
        let offset_words = forwarding
            .get(&target.offset_words)
            .copied()
            .ok_or(HeapError::InvalidObject(target))?;
        Ok(Some(HeapRef {
            epoch: new_epoch,
            offset_words,
        }))
    }

    fn forward_weak(
        target: Option<HeapRef>,
        old_epoch: u64,
        new_epoch: u64,
        forwarding: &HashMap<usize, usize>,
    ) -> Result<Option<HeapRef>, HeapError> {
        let Some(target) = target else {
            return Ok(None);
        };
        if target.epoch != old_epoch {
            return Err(HeapError::ForeignReference);
        }
        Ok(forwarding
            .get(&target.offset_words)
            .copied()
            .map(|offset_words| HeapRef {
                epoch: new_epoch,
                offset_words,
            }))
    }
}

impl Drop for DomainHeap {
    fn drop(&mut self) {
        if self.accounting {
            crate::resource_accounting::release(
                crate::resource_accounting::Kind::Managed,
                self.allocated_bytes() as u64,
            );
            crate::resource_accounting::release(
                crate::resource_accounting::Kind::ManagedObjects,
                self.object_count() as u64,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_range_rejects_later_references_before_any_payload_is_published() {
        let mut heap = DomainHeap::new(1400);
        let child = heap
            .allocate(1, &[HeapValue::Integer(3)], FinalizerPolicy::None)
            .unwrap();
        let parent = heap
            .allocate(
                2,
                &[HeapValue::Integer(1), HeapValue::Reference(child)],
                FinalizerPolicy::None,
            )
            .unwrap();
        let root = heap.create_root(Some(parent)).unwrap();
        assert!(
            heap.write_integer_fields(parent, 0, [9, 9].into_iter())
                .is_err()
        );
        assert_eq!(
            heap.fields(parent).unwrap(),
            vec![HeapValue::Integer(1), HeapValue::Reference(child)]
        );
        heap.write_integer_fields(parent, 0, [7].into_iter())
            .unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        let parent = heap.root_target(root).unwrap().unwrap();
        let fields = heap.fields(parent).unwrap();
        assert_eq!(fields[0], HeapValue::Integer(7));
        let HeapValue::Reference(child) = fields[1] else {
            panic!("lost reference");
        };
        assert_eq!(heap.fields(child).unwrap(), vec![HeapValue::Integer(3)]);
    }

    #[test]
    fn released_root_slots_reuse_storage_without_reviving_stale_generations() {
        let mut heap = DomainHeap::new(104);
        let original = heap.create_root(None).unwrap();
        let mut root = original;
        for _ in 0..4096 {
            heap.release_root(root).unwrap();
            root = heap.create_root(None).unwrap();
        }
        assert_eq!(heap.roots.len(), 1);
        assert!(heap.free_roots.capacity() >= heap.roots.len());
        assert!(heap.root_target(original).is_err());
        assert!(heap.release_root(original).is_err());
        // An exhausted generation is retired permanently, never wrapped/reused.
        heap.roots[root.index].generation = u64::MAX;
        let exhausted = RootSlot {
            index: root.index,
            generation: u64::MAX,
        };
        heap.release_root(exhausted).unwrap();
        let next = heap.create_root(None).unwrap();
        assert_ne!(next.index, exhausted.index);
        assert!(heap.root_target(exhausted).is_err());
    }
    use crate::{rinzu_handle_release, rinzu_handle_retain};
    use rinzu_abi::STATUS_OK;
    use std::ptr;

    #[test]
    fn integer_streams_keep_the_generic_layout_and_reject_incorrect_counts() {
        struct Reported {
            values: std::vec::IntoIter<i64>,
            count: usize,
        }
        impl Iterator for Reported {
            type Item = i64;
            fn next(&mut self) -> Option<i64> {
                self.values.next()
            }
            fn size_hint(&self) -> (usize, Option<usize>) {
                (self.count, Some(self.count))
            }
        }
        impl ExactSizeIterator for Reported {}
        let mut heap = DomainHeap::new(760);
        for count in [0, 1, 2, 9, 8192] {
            let values: Vec<_> = (0..count).map(|n| i64::MIN.wrapping_add(n)).collect();
            let fields: Vec<_> = values.iter().copied().map(HeapValue::Integer).collect();
            let generic = heap.allocate(31, &fields, FinalizerPolicy::None).unwrap();
            let streamed = heap.allocate_integers(31, values.into_iter()).unwrap();
            assert_eq!(
                heap.fields(generic).unwrap(),
                heap.fields(streamed).unwrap()
            );
            assert_eq!(
                heap.header(generic).unwrap().size_bytes,
                heap.header(streamed).unwrap().size_bytes
            );
            let roots = [
                heap.create_root(Some(generic)).unwrap(),
                heap.create_root(Some(streamed)).unwrap(),
            ];
            heap.collect(CollectionKind::Full).unwrap();
            let moved = roots.map(|root| heap.root_target(root).unwrap().unwrap());
            assert_eq!(heap.fields(moved[0]).unwrap(), fields);
            assert_eq!(heap.fields(moved[1]).unwrap(), fields);
            for root in roots {
                heap.release_root(root).unwrap();
            }
        }
        for (values, count) in [(vec![1], 2), (vec![1, 2], 1)] {
            let words = heap.words.clone();
            let objects = heap.object_count();
            assert!(matches!(
                heap.allocate_integers(
                    31,
                    Reported {
                        values: values.into_iter(),
                        count
                    }
                ),
                Err(HeapError::InvalidObject(_))
            ));
            assert_eq!(heap.words, words);
            assert_eq!(heap.object_count(), objects);
        }
        assert_eq!(
            heap.allocate_integers(31, std::iter::repeat_n(0, usize::MAX)),
            Err(HeapError::AddressSpaceExhausted)
        );
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(heap.object_count(), 0);
    }

    #[test]
    fn native_input_snapshots_before_its_source_heap_can_grow() {
        let mut heap = DomainHeap::new(761);
        heap.allocate(
            32,
            &[HeapValue::Integer(i64::MIN); 256],
            FinalizerPolicy::None,
        )
        .unwrap();
        heap.words.shrink_to_fit();
        let pointer = heap.words.as_ptr().cast::<u8>();
        let length = heap.words.len() * size_of::<u64>();
        // SAFETY: these are initialized bytes in the current heap allocation.
        let expected = unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec();
        let mut data = crate::data::ManagedData::new(761);
        // SAFETY: the runtime must finish the native snapshot before growing
        // this same heap; it must never stream from this pointer during growth.
        let handle =
            unsafe { data.receive_native_bytes(&mut heap, length as u64, pointer) }.unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(heap.object_count(), 1);
        assert_eq!(data.bytes(&heap, handle, length as u64).unwrap(), expected);
    }

    #[test]
    fn borrowed_fields_preserve_decode_errors_owner_and_moving_epoch() {
        let mut heap = DomainHeap::new(71);
        let child = heap.allocate(1, &[], FinalizerPolicy::None).unwrap();
        let reference = heap
            .allocate(
                2,
                &[
                    HeapValue::Null,
                    HeapValue::Integer(i64::MIN),
                    HeapValue::Reference(child),
                ],
                FinalizerPolicy::None,
            )
            .unwrap();
        let root = heap.create_root(Some(reference)).unwrap();
        assert_eq!(
            heap.fields_iter(reference).unwrap().collect::<Vec<_>>(),
            heap.fields(reference).unwrap()
        );
        let mut selected = heap.fields_iter(reference).unwrap();
        assert_eq!(selected.nth(1), Some(HeapValue::Integer(i64::MIN)));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected.next(), Some(HeapValue::Reference(child)));
        assert_eq!(selected.nth(usize::MAX), None);
        assert_eq!(selected.next(), None);
        let mut backwards = heap.fields_iter(reference).unwrap();
        assert_eq!(backwards.next_back(), Some(HeapValue::Reference(child)));
        assert_eq!(backwards.next(), Some(HeapValue::Null));
        assert_eq!(backwards.next_back(), Some(HeapValue::Integer(i64::MIN)));
        assert_eq!(backwards.next_back(), None);
        assert_eq!(
            heap.fields_iter(reference)
                .unwrap()
                .take(2)
                .rev()
                .collect::<Vec<_>>(),
            [HeapValue::Integer(i64::MIN), HeapValue::Null],
        );
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    assert!(matches!(
                        heap.fields_iter(reference),
                        Err(HeapError::WrongThread { .. })
                    ))
                })
                .join()
                .unwrap();
        });
        heap.collect(CollectionKind::Full).unwrap();
        assert!(matches!(
            heap.fields_iter(reference),
            Err(HeapError::StaleReference(_))
        ));
        let moved = heap.root_target(root).unwrap().unwrap();
        assert_eq!(
            heap.fields_iter(moved).unwrap().collect::<Vec<_>>(),
            heap.fields(moved).unwrap()
        );
        let tag = moved.offset_words + HEADER_WORDS + FIELD_WORDS * 2;
        heap.words[tag] = 99;
        assert!(matches!(
            heap.fields(moved),
            Err(HeapError::InvalidObject(_))
        ));
        assert!(matches!(
            heap.fields_iter(moved),
            Err(HeapError::InvalidObject(_))
        ));
    }

    #[test]
    fn collection_moves_graph_and_updates_all_root_kinds() {
        let mut heap = DomainHeap::new(7);
        let leaf = heap
            .allocate(1, &[HeapValue::Integer(41)], FinalizerPolicy::None)
            .unwrap();
        let parent = heap
            .allocate(2, &[HeapValue::Reference(leaf)], FinalizerPolicy::None)
            .unwrap();
        let root = heap.create_root(Some(parent)).unwrap();
        let handle = heap.create_stable_handle(leaf).unwrap();
        let old_parent = parent;
        let old_leaf = leaf;

        let first = heap.collect(CollectionKind::Minor).unwrap();
        assert_eq!(first.moved_objects, 2);
        let new_parent = heap.root_target(root).unwrap().unwrap();
        // SAFETY: handle remains owned until the release at the end of the test.
        let new_leaf = unsafe { heap.resolve_stable_handle(handle) }
            .unwrap()
            .unwrap();
        assert_ne!(new_parent.epoch(), old_parent.epoch());
        assert_ne!(new_leaf.epoch(), old_leaf.epoch());
        assert_eq!(
            heap.fields(new_parent).unwrap(),
            vec![HeapValue::Reference(new_leaf)]
        );
        assert!(matches!(
            heap.fields(old_parent),
            Err(HeapError::StaleReference(_))
        ));

        let mut retained = ptr::null_mut();
        // SAFETY: both pointers follow the owned C stable-handle contract.
        unsafe {
            assert_eq!(rinzu_handle_retain(handle, &mut retained), STATUS_OK);
            assert_eq!(
                heap.stable_handle_generation(handle).unwrap(),
                heap.stable_handle_generation(retained).unwrap()
            );
            rinzu_handle_release(retained);
            rinzu_handle_release(handle);
        }
    }

    #[test]
    fn weak_references_clear_and_only_native_finalization_is_reported() {
        let mut heap = DomainHeap::new(9);
        let dead = heap.allocate(5, &[], FinalizerPolicy::NativeOnly).unwrap();
        let weak = heap.create_weak(dead).unwrap();
        let report = heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(report.reclaimed_objects, 1);
        assert_eq!(report.cleared_weak_handles, 1);
        assert_eq!(report.native_finalizers.len(), 1);
        assert_eq!(report.native_finalizers[0].type_id, 5);
        assert_eq!(heap.resolve_weak(weak), Ok(None));
    }

    #[test]
    fn surviving_objects_promote_after_two_collections_and_remain_aligned() {
        let mut heap = DomainHeap::new(11);
        let object = heap.allocate(3, &[], FinalizerPolicy::None).unwrap();
        let root = heap.create_root(Some(object)).unwrap();
        assert_eq!(object.offset_bytes() % OBJECT_ALIGNMENT, 0);
        heap.collect(CollectionKind::Minor).unwrap();
        let second = heap.collect(CollectionKind::Minor).unwrap();
        assert_eq!(second.promoted_objects, 1);
        let object = heap.root_target(root).unwrap().unwrap();
        assert!(heap.header(object).unwrap().is_old);
        assert_eq!(
            heap.header(object).unwrap().size_bytes as usize % OBJECT_ALIGNMENT,
            0
        );
    }

    #[test]
    fn final_stable_handle_release_removes_the_gc_root() {
        let mut heap = DomainHeap::new(13);
        let object = heap.allocate(8, &[], FinalizerPolicy::None).unwrap();
        let handle = heap.create_stable_handle(object).unwrap();
        assert_eq!(heap.collect(CollectionKind::Full).unwrap().moved_objects, 1);
        // SAFETY: this consumes the only owned stable-handle reference.
        unsafe { rinzu_handle_release(handle) };
        let report = heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(report.reclaimed_objects, 1);
        assert_eq!(heap.object_count(), 0);
    }
}

#[cfg(test)]
#[path = "../../../tests/fixtures/state/spike.rs"]
mod m2_state_spike;

#[cfg(test)]
#[path = "../../../tests/fixtures/functions/heap_spike.rs"]
mod m2_function_spike;

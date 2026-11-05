//! Target-validated C layouts, independent of managed heap and wire layouts.
use super::*;
use crate::scalar::{ScalarKind, ScalarValue};
use crate::value::{ValueSchema, ValueType};
use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::ptr::NonNull;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeRecordField {
    pub kind: ScalarKind,
    pub offset: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRecordLayout {
    pub ty: usize,
    allocation: Layout,
    fields: Vec<NativeRecordField>,
}

/// Runtime-only identity of a shared target, independent of root handles and
/// moving heap addresses. This does not expose language equality or an address.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NativeRecordIdentity {
    pub(super) domain: u64,
    pub(super) object: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeRecordError {
    InvalidLayout,
    Null,
    Conversion,
    Data(DataError),
}
impl From<DataError> for NativeRecordError {
    fn from(error: DataError) -> Self {
        match error {
            DataError::AddressSpaceExhausted
            | DataError::Heap(HeapError::AddressSpaceExhausted) => {
                // SAFETY: native resource failure neither allocates nor unwinds.
                crate::native_fatal::resource()
            }
            other => Self::Data(other),
        }
    }
}

impl NativeRecordLayout {
    /// Offsets, size and alignment must come from the validated target compiler.
    /// This only checks their consistency; it never calculates a C layout.
    pub fn new(
        ty: usize,
        size: usize,
        alignment: usize,
        fields: Vec<NativeRecordField>,
    ) -> Result<Self, NativeRecordError> {
        let allocation = Layout::from_size_align(size, alignment)
            .map_err(|_| NativeRecordError::InvalidLayout)?;
        if size == 0 || !size.is_multiple_of(alignment) || fields.is_empty() {
            return Err(NativeRecordError::InvalidLayout);
        }
        let mut end = 0;
        for field in &fields {
            let width = field.kind.width() as usize / 8;
            if alignment < width || !field.offset.is_multiple_of(width) || field.offset < end {
                return Err(NativeRecordError::InvalidLayout);
            }
            end = field
                .offset
                .checked_add(width)
                .filter(|end| *end <= size)
                .ok_or(NativeRecordError::InvalidLayout)?;
        }
        Ok(Self {
            ty,
            allocation,
            fields,
        })
    }
    pub fn size(&self) -> usize {
        self.allocation.size()
    }
    pub fn alignment(&self) -> usize {
        self.allocation.align()
    }
    pub fn fields(&self) -> &[NativeRecordField] {
        &self.fields
    }

    pub fn validate_schema(&self, schema: &ValueSchema) -> Result<(), NativeRecordError> {
        let Ok(ValueType::Record { fields, .. } | ValueType::State { fields, .. }) =
            schema.ty(self.ty)
        else {
            return Err(NativeRecordError::InvalidLayout);
        };
        if fields.len() != self.fields.len() || fields.iter().zip(&self.fields).any(|(ty, field)| {
            !matches!(schema.ty(*ty), Ok(ValueType::Scalar(kind)) if *kind == field.kind)
        }) {
            return Err(NativeRecordError::InvalidLayout);
        }
        Ok(())
    }

    /// # Safety
    /// Non-null input must provide the validated struct's readable fields and
    /// remain stable until reception completes. Native read faults are fatal.
    pub unsafe fn receive(
        &self,
        pointer: *const u8,
    ) -> Result<Vec<ScalarValue>, NativeRecordError> {
        if pointer.is_null() {
            return Err(NativeRecordError::Null);
        }
        let mut values = Vec::new();
        if values.try_reserve_exact(self.fields.len()).is_err() {
            // SAFETY: copy allocation failure is an immediate resource fatal.
            crate::native_fatal::resource()
        }
        // SAFETY: the caller supplies this layout; the vector was reserved above.
        unsafe {
            self.receive_into(pointer, &mut values)?;
        }
        Ok(values)
    }

    /// Append into preparation's reserved validation storage without allocating.
    /// # Safety
    /// The same readable native struct contract as `receive` applies.
    pub(super) unsafe fn receive_into(
        &self,
        pointer: *const u8,
        values: &mut Vec<ScalarValue>,
    ) -> Result<(), NativeRecordError> {
        if pointer.is_null() {
            return Err(NativeRecordError::Null);
        }
        if values.capacity() - values.len() < self.fields.len() {
            return Err(NativeRecordError::InvalidLayout);
        }
        for field in &self.fields {
            let mut bytes = [0; 8];
            let width = field.kind.width() as usize / 8;
            // SAFETY: only the declared field bytes are copied, including CBool
            // as raw bytes rather than reading an invalid Rust/C Boolean value.
            unsafe {
                libc::memcpy(
                    bytes.as_mut_ptr().cast(),
                    pointer.add(field.offset).cast(),
                    width,
                )
            };
            let bits = match width {
                1 => u64::from(bytes[0]),
                2 => u64::from(u16::from_ne_bytes(bytes[..2].try_into().unwrap())),
                4 => u64::from(u32::from_ne_bytes(bytes[..4].try_into().unwrap())),
                8 => u64::from_ne_bytes(bytes),
                _ => unreachable!(),
            };
            values.push(
                ScalarValue::from_physical_bits(field.kind, bits)
                    .map_err(|_| NativeRecordError::Conversion)?,
            );
        }
        Ok(())
    }
}

/// Initialized, aligned, nonmoving C copy. Its padding is always zero.
#[derive(Debug)]
pub struct NativeRecordBuffer {
    pointer: NonNull<u8>,
    allocation: Layout,
}
impl NativeRecordBuffer {
    pub(super) fn allocate(size: usize, alignment: usize) -> Self {
        let allocation = Layout::from_size_align(size, alignment)
            .unwrap_or_else(|_| crate::native_fatal::resource());
        // SAFETY: callers request nonzero checked native layouts.
        let pointer = NonNull::new(unsafe { alloc_zeroed(allocation) })
            .unwrap_or_else(|| crate::native_fatal::resource());
        Self {
            pointer,
            allocation,
        }
    }
    pub(super) fn capacity(&self) -> usize {
        self.allocation.size()
    }
    pub(super) fn alignment(&self) -> usize {
        self.allocation.align()
    }
    pub(super) fn write(
        &mut self,
        offset: usize,
        layout: &NativeRecordLayout,
        values: &[ScalarValue],
    ) {
        if offset > self.capacity()
            || layout.size() > self.capacity() - offset
            || values.len() != layout.fields.len()
        {
            crate::native_fatal::invariant();
        }
        // SAFETY: offsets are checked against the owned buffer and Clang layout.
        unsafe {
            std::ptr::write_bytes(self.pointer.as_ptr().add(offset), 0, layout.size());
        }
        for (value, field) in values.iter().zip(&layout.fields) {
            if value.kind() != field.kind {
                crate::native_fatal::invariant();
            }
            let bits = value.bits();
            let width = field.kind.width() as usize / 8;
            let mut bytes = [0u8; 8];
            match width {
                1 => bytes[0] = bits as u8,
                2 => bytes[..2].copy_from_slice(&(bits as u16).to_ne_bytes()),
                4 => bytes[..4].copy_from_slice(&(bits as u32).to_ne_bytes()),
                8 => bytes = bits.to_ne_bytes(),
                _ => unreachable!(),
            }
            // SAFETY: checked field widths and offsets remain inside the owned buffer.
            unsafe {
                libc::memcpy(
                    self.pointer.as_ptr().add(offset + field.offset).cast(),
                    bytes.as_ptr().cast(),
                    field.kind.width() as usize / 8,
                );
            }
        }
    }
    pub fn new(
        layout: &NativeRecordLayout,
        values: &[ScalarValue],
    ) -> Result<Self, NativeRecordError> {
        if values.len() != layout.fields.len()
            || values
                .iter()
                .zip(&layout.fields)
                .any(|(value, field)| value.kind() != field.kind)
        {
            return Err(NativeRecordError::Data(DataError::InvalidEncoding));
        }
        let mut buffer = Self::allocate(layout.size(), layout.alignment());
        buffer.write(0, layout, values);
        Ok(buffer)
    }
    pub fn as_ptr(&self) -> *const u8 {
        self.pointer.as_ptr()
    }
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.pointer.as_ptr()
    }
}
impl Drop for NativeRecordBuffer {
    fn drop(&mut self) {
        // SAFETY: this buffer uniquely owns the pointer with its original Layout.
        unsafe { dealloc(self.pointer.as_ptr(), self.allocation) };
    }
}

impl ManagedData {
    /// Resolve a shared native target without equating root handles. Only a
    /// reference schema is writable; ordinary immutable records stay immutable.
    pub fn native_record_identity(
        &self,
        heap: &DomainHeap,
        layout: &NativeRecordLayout,
        handle: ManagedHandle,
    ) -> Result<NativeRecordIdentity, NativeRecordError> {
        self.require_domain(heap)?;
        let reference = self.reference(heap, handle, DataKind::Composite(layout.ty))?;
        self.native_record_target_at_reference(heap, layout, reference)
    }

    /// The caller has checked the Domain and resolved a local reference. A
    /// returned proof is reusable only until GC, managed mutation or native entry.
    pub(super) fn native_record_target_at_reference(
        &self,
        heap: &DomainHeap,
        layout: &NativeRecordLayout,
        reference: HeapRef,
    ) -> Result<NativeRecordIdentity, NativeRecordError> {
        layout.validate_schema(self.value_schema()?)?;
        if !matches!(
            self.value_schema()?.ty(layout.ty),
            Ok(ValueType::State { .. })
        ) {
            return Err(NativeRecordError::InvalidLayout);
        }
        let view = heap.object_view(reference).map_err(DataError::from)?;
        let header = view.header;
        if header.type_id != COMPOSITE_TYPE {
            return Err(DataError::WrongKind.into());
        }
        let mut fields = view.fields().map_err(DataError::from)?;
        let ty = fields.next();
        let variant = fields.next();
        if integer(ty.as_ref())? != layout.ty as i64
            || integer(variant.as_ref())? != -1
            || fields.len() != layout.fields.len()
            || fields.any(|field| !matches!(field, HeapValue::Integer(_)))
        {
            return Err(DataError::InvalidEncoding.into());
        }
        Ok(NativeRecordIdentity {
            domain: self.domain,
            object: header.object_id,
        })
    }

    /// Internal native writeback, never a source field assignment. Validate all
    /// values before writing; no allocation, GC or managed call occurs here.
    pub fn update_native_record_fields(
        &mut self,
        heap: &mut DomainHeap,
        layout: &NativeRecordLayout,
        handle: ManagedHandle,
        values: &[ScalarValue],
    ) -> Result<(), NativeRecordError> {
        self.native_record_identity(heap, layout, handle)?;
        if values.len() != layout.fields.len()
            || values
                .iter()
                .zip(&layout.fields)
                .any(|(value, field)| value.kind() != field.kind)
        {
            return Err(NativeRecordError::Conversion);
        }
        let reference = self.reference(heap, handle, DataKind::Composite(layout.ty))?;
        for (index, value) in values.iter().enumerate() {
            heap.write_field(
                reference,
                index + 2,
                HeapValue::Integer(value.bits() as i64),
            )
            .map_err(DataError::from)?;
        }
        Ok(())
    }

    pub fn copy_native_record(
        &mut self,
        heap: &mut DomainHeap,
        layout: &NativeRecordLayout,
        handle: ManagedHandle,
    ) -> Result<NativeRecordBuffer, NativeRecordError> {
        self.require_domain(heap)?;
        layout.validate_schema(self.value_schema()?)?;
        let mut values = Vec::new();
        if values.try_reserve_exact(layout.fields.len()).is_err() {
            // SAFETY: temporary preparation failure is process-fatal.
            crate::native_fatal::resource()
        }
        for index in 0..layout.fields.len() {
            let ManagedSlot::Scalar(value) = self.project(heap, handle, layout.ty, None, index)?
            else {
                return Err(NativeRecordError::Data(DataError::InvalidEncoding));
            };
            values.push(value);
        }
        NativeRecordBuffer::new(layout, &values)
    }
    /// # Safety
    /// Same readable, stable native struct contract as NativeRecordLayout::receive.
    pub unsafe fn receive_native_record(
        &mut self,
        heap: &mut DomainHeap,
        layout: &NativeRecordLayout,
        pointer: *const u8,
    ) -> Result<ManagedHandle, NativeRecordError> {
        self.require_domain(heap)?;
        layout.validate_schema(self.value_schema()?)?;
        if let Some(handle) = self.restore_native_record_pointer(heap, layout, pointer)? {
            return Ok(handle);
        }
        // SAFETY: the caller supplies this exact validated struct contract.
        let values = unsafe { layout.receive(pointer) }?;
        let mut slots = Vec::new();
        if slots.try_reserve_exact(values.len()).is_err() {
            // SAFETY: managed result preparation cannot recover from resource failure.
            crate::native_fatal::resource()
        }
        slots.extend(values.into_iter().map(ManagedSlot::Scalar));
        Ok(self.create_composite(heap, layout.ty, None, &slots)?)
    }
}

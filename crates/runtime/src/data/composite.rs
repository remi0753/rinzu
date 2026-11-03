use super::*;
use crate::scalar::ScalarValue;
use crate::value::{Value, ValueError, ValueSchema, ValueType};
use std::collections::HashMap;
use std::sync::Arc;

const BYTES_TYPE: u32 = 0xffff_ff03;
pub(super) const COMPOSITE_TYPE: u32 = 0xffff_ff04;

/// The tag comes from the typed frame, never from a scalar's numeric bit pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ManagedSlot {
    Scalar(ScalarValue),
    Unit,
    Reference(ManagedHandle),
    Opaque(u64),
}

/// A checked field for one operation, invalid after moving GC. Original inputs
/// stay rooted; the operation cannot collect or invoke managed code.
#[derive(Clone, Copy)]
pub(super) enum DecodedSlot {
    Value(ManagedSlot),
    Reference(HeapRef, DataKind),
}

impl From<ValueError> for DataError {
    fn from(error: ValueError) -> Self {
        Self::Value(error)
    }
}

impl ManagedData {
    /// One immutable schema describes all composite slots in this Domain.
    pub fn configure_values(
        &mut self,
        heap: &DomainHeap,
        schema: Arc<ValueSchema>,
    ) -> Result<(), DataError> {
        self.require_domain(heap)?;
        if self.schema.is_some() {
            return Err(ValueError::InvalidSchema.into());
        }
        self.schema = Some(schema);
        Ok(())
    }

    pub(super) fn value_schema(&self) -> Result<&ValueSchema, DataError> {
        self.schema
            .as_deref()
            .ok_or_else(|| ValueError::InvalidSchema.into())
    }

    pub(super) fn any_reference(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
    ) -> Result<(HeapRef, DataKind), DataError> {
        self.require_domain(heap)?;
        let (root, kind) = self.roots.get(&handle).ok_or(DataError::UnknownHandle)?;
        Ok((
            heap.root_target(*root)?.ok_or(DataError::InvalidEncoding)?,
            *kind,
        ))
    }

    pub fn create_bytes(
        &mut self,
        heap: &mut DomainHeap,
        length: u64,
        bytes: &[u8],
    ) -> Result<ManagedHandle, DataError> {
        self.require_domain(heap)?;
        if length != bytes.len() as u64 {
            return Err(ValueError::TypeMismatch.into());
        }
        let reference = heap.allocate_integers(BYTES_TYPE, PackedWords::new(bytes)?)?;
        self.root(heap, reference, DataKind::Bytes(length))
    }

    pub(super) fn bytes_at(
        heap: &DomainHeap,
        reference: HeapRef,
        length: u64,
    ) -> Result<Vec<u8>, DataError> {
        if heap.header(reference)?.type_id != BYTES_TYPE {
            return Err(DataError::WrongKind);
        }
        let mut fields = heap.fields_iter(reference)?;
        let first = fields.next();
        let actual =
            usize::try_from(integer(first.as_ref())?).map_err(|_| DataError::InvalidEncoding)?;
        if actual as u64 != length || fields.len() != actual.div_ceil(8) {
            return Err(DataError::InvalidEncoding);
        }
        let mut bytes = Vec::with_capacity(actual);
        for field in fields {
            let word = (integer(Some(&field))? as u64).to_le_bytes();
            let count = (actual - bytes.len()).min(8);
            if word[count..].iter().any(|byte| *byte != 0) {
                return Err(DataError::InvalidEncoding);
            }
            bytes.extend_from_slice(&word[..count]);
        }
        Ok(bytes)
    }

    pub fn bytes(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
        length: u64,
    ) -> Result<Vec<u8>, DataError> {
        Self::bytes_at(
            heap,
            self.reference(heap, handle, DataKind::Bytes(length))?,
            length,
        )
    }

    pub fn dynamic_byte_length(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
    ) -> Result<u64, DataError> {
        let (_, kind) = self.any_reference(heap, handle)?;
        match kind {
            DataKind::Bytes(n) => Ok(n),
            _ => Err(DataError::WrongKind),
        }
    }
    pub fn dynamic_bytes(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
    ) -> Result<Vec<u8>, DataError> {
        self.bytes(heap, handle, self.dynamic_byte_length(heap, handle)?)
    }

    /// Validate the complete packed encoding before an early-returning query.
    /// The heap loan keeps these words stable until the query has finished.
    pub(super) fn checked_byte_fields<'a>(
        &self,
        heap: &'a DomainHeap,
        handle: ManagedHandle,
    ) -> Result<(usize, crate::heap::HeapFields<'a>), DataError> {
        let (reference, kind) = self.any_reference(heap, handle)?;
        self.checked_byte_reference_fields(heap, reference, kind)
    }

    pub(super) fn checked_byte_reference_fields<'a>(
        &self,
        heap: &'a DomainHeap,
        reference: HeapRef,
        kind: DataKind,
    ) -> Result<(usize, crate::heap::HeapFields<'a>), DataError> {
        let DataKind::Bytes(length) = kind else {
            return Err(DataError::WrongKind);
        };
        if heap.header(reference)?.type_id != BYTES_TYPE {
            return Err(DataError::WrongKind);
        }
        let mut fields = heap.fields_iter(reference)?;
        let actual = usize::try_from(integer(fields.next().as_ref())?)
            .map_err(|_| DataError::InvalidEncoding)?;
        if actual as u64 != length || fields.len() != actual.div_ceil(8) {
            return Err(DataError::InvalidEncoding);
        }
        for field in fields.clone() {
            integer(Some(&field))?;
        }
        let tail = actual % 8;
        if tail != 0 && (integer(fields.clone().next_back().as_ref())? as u64) >> (tail * 8) != 0 {
            return Err(DataError::InvalidEncoding);
        }
        Ok((actual, fields))
    }

    pub fn byte_length(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
        expected: u64,
    ) -> Result<u64, DataError> {
        let expected = if expected == u64::MAX {
            self.dynamic_byte_length(heap, handle)?
        } else {
            expected
        };
        let reference = self.reference(heap, handle, DataKind::Bytes(expected))?;
        let mut fields = heap.fields_iter(reference)?;
        let first = fields.next();
        if heap.header(reference)?.type_id != BYTES_TYPE
            || integer(first.as_ref())? as u64 != expected
            || fields.len() as u64 != expected.div_ceil(8)
        {
            return Err(DataError::InvalidEncoding);
        }
        Ok(expected)
    }

    pub(super) fn slot_field(
        &self,
        heap: &DomainHeap,
        ty: usize,
        slot: ManagedSlot,
    ) -> Result<HeapValue, DataError> {
        self.require_domain(heap)?;
        let expected = self.value_schema()?.ty(ty)?;
        match (expected, slot) {
            (ValueType::Scalar(kind), ManagedSlot::Scalar(value)) if *kind == value.kind() => {
                Ok(HeapValue::Integer(value.bits() as i64))
            }
            (ValueType::Unit, ManagedSlot::Unit) => Ok(HeapValue::Null),
            (ValueType::Other, ManagedSlot::Opaque(bits)) => Ok(HeapValue::Integer(bits as i64)),
            (_, ManagedSlot::Reference(handle)) => {
                let (reference, kind) = self.any_reference(heap, handle)?;
                self.reference_slot_field(ty, expected, reference, kind)
            }
            _ => Err(ValueError::TypeMismatch.into()),
        }
    }

    fn reference_slot_field(
        &self,
        ty: usize,
        expected: &ValueType,
        reference: HeapRef,
        kind: DataKind,
    ) -> Result<HeapValue, DataError> {
        let valid = match (expected, kind) {
            (ValueType::String, DataKind::String) => true,
            (ValueType::Bytes(length), DataKind::Bytes(actual)) => *length == actual,
            (ValueType::DynamicBytes, DataKind::Bytes(_)) => true,
            (ValueType::Error, DataKind::Error | DataKind::RichError(_)) => true,
            (ValueType::Error, DataKind::Composite(actual)) => matches!(
                self.value_schema()?.ty(actual)?,
                ValueType::ErrorRecord { .. }
            ),
            (
                ValueType::Record { .. }
                | ValueType::State { .. }
                | ValueType::ErrorRecord { .. }
                | ValueType::Enum { .. }
                | ValueType::List(_)
                | ValueType::Map { .. },
                DataKind::Composite(actual),
            ) => ty == actual,
            (ValueType::Other, DataKind::Error) => true,
            (ValueType::Other, DataKind::NativeOpaque(actual)) => ty == actual,
            _ => false,
        };
        if valid {
            Ok(HeapValue::Reference(reference))
        } else {
            Err(ValueError::TypeMismatch.into())
        }
    }

    pub(super) fn decoded_slot_field(
        &self,
        heap: &DomainHeap,
        ty: usize,
        slot: DecodedSlot,
    ) -> Result<HeapValue, DataError> {
        match slot {
            DecodedSlot::Value(value) => self.slot_field(heap, ty, value),
            DecodedSlot::Reference(reference, kind) => {
                self.reference_slot_field(ty, self.value_schema()?.ty(ty)?, reference, kind)
            }
        }
    }

    /// Original inputs stay rooted throughout preparation. All fields are checked
    /// before allocating; a later Map entry cannot hide an invalid earlier value.
    pub fn create_composite(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        variant: Option<u32>,
        slots: &[ManagedSlot],
    ) -> Result<ManagedHandle, DataError> {
        self.create_composite_hashed(heap, ty, variant, slots, |hash| hash)
    }

    pub(super) fn create_composite_hashed(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        variant: Option<u32>,
        slots: &[ManagedSlot],
        remap_hash: fn(u64) -> u64,
    ) -> Result<ManagedHandle, DataError> {
        self.require_domain(heap)?;
        let schema = self.value_schema()?;
        let types = child_types(schema.ty(ty)?, variant, slots.len())?;
        let mut fields = Vec::with_capacity(slots.len() + 2);
        fields.push(HeapValue::Integer(
            i64::try_from(ty).map_err(|_| DataError::AddressSpaceExhausted)?,
        ));
        fields.push(HeapValue::Integer(variant.map_or(-1, i64::from)));
        for (child, &slot) in types.iter().zip(slots) {
            fields.push(self.slot_field(heap, child, slot)?);
        }
        self.finish_composite_fields(heap, ty, fields, remap_hash)
    }

    /// Original source containers/arguments stay rooted; allocation never
    /// collects or invokes managed code. The owned decoded slots retain kind
    /// and nominal identity without publishing temporary child roots.
    pub(super) fn create_composite_decoded_hashed(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        variant: Option<u32>,
        slots: &[DecodedSlot],
        remap_hash: fn(u64) -> u64,
    ) -> Result<ManagedHandle, DataError> {
        self.require_domain(heap)?;
        let types = child_types(self.value_schema()?.ty(ty)?, variant, slots.len())?;
        let mut fields = Vec::with_capacity(slots.len() + 2);
        fields.push(HeapValue::Integer(
            i64::try_from(ty).map_err(|_| DataError::AddressSpaceExhausted)?,
        ));
        fields.push(HeapValue::Integer(variant.map_or(-1, i64::from)));
        for (child, &slot) in types.iter().zip(slots) {
            fields.push(self.decoded_slot_field(heap, child, slot)?);
        }
        self.finish_composite_fields(heap, ty, fields, remap_hash)
    }

    fn finish_composite_fields(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        mut fields: Vec<HeapValue>,
        remap_hash: fn(u64) -> u64,
    ) -> Result<ManagedHandle, DataError> {
        let count = fields.len() - 2;
        let schema = self.value_schema()?;
        if matches!(schema.ty(ty)?, ValueType::ErrorRecord { .. }) {
            // Private first-failure position. These scalar words are never
            // application fields and require no extra managed allocation.
            fields.extend([HeapValue::Integer(0); 4]);
        }
        if let ValueType::Map { key, .. } = schema.ty(ty)? {
            let mut pairs: Vec<usize> = Vec::new();
            let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
            let mut keys: Vec<Value> = Vec::new();
            for index in (0..count).step_by(2) {
                let key_value = self.snapshot_field(heap, *key, fields[index + 2])?;
                let hash = remap_hash(schema.key_hash(*key, &key_value)?);
                let bucket = buckets.entry(hash).or_default();
                let mut previous = None;
                for &candidate in bucket.iter() {
                    if schema.equal(*key, &keys[candidate], &key_value)? {
                        previous = Some(candidate);
                        break;
                    }
                }
                if let Some(previous) = previous {
                    pairs[previous] = index;
                } else {
                    bucket.push(pairs.len());
                    pairs.push(index);
                    keys.push(key_value);
                }
            }
            let length = 2 + pairs.len() * 2;
            for (output, index) in pairs.into_iter().enumerate() {
                let destination = 2 + output * 2;
                let source = index + 2;
                // An entry's last occurrence is never before its first unique
                // position, so compaction cannot overwrite an unread pair.
                debug_assert!(source >= destination);
                if source != destination {
                    fields.copy_within(source..source + 2, destination);
                }
            }
            fields.truncate(length);
        }
        let reference = heap.allocate(COMPOSITE_TYPE, &fields, FinalizerPolicy::None)?;
        self.root(heap, reference, DataKind::Composite(ty))
    }

    pub(super) fn composite_fields(
        &self,
        heap: &DomainHeap,
        reference: HeapRef,
        ty: usize,
    ) -> Result<(Option<u32>, Vec<HeapValue>), DataError> {
        let (variant, fields) = self.composite_fields_iter(heap, reference, ty)?;
        Ok((variant, fields.collect()))
    }

    pub(super) fn composite_fields_iter<'a>(
        &self,
        heap: &'a DomainHeap,
        reference: HeapRef,
        ty: usize,
    ) -> Result<(Option<u32>, std::iter::Take<crate::heap::HeapFields<'a>>), DataError> {
        if heap.header(reference)?.type_id != COMPOSITE_TYPE {
            return Err(DataError::WrongKind);
        }
        let mut fields = heap.fields_iter(reference)?;
        let first = fields.next();
        if usize::try_from(integer(first.as_ref())?).ok() != Some(ty) {
            return Err(ValueError::TypeMismatch.into());
        }
        let encoded_variant = fields.next();
        let variant = match integer(encoded_variant.as_ref())? {
            -1 => None,
            value => Some(u32::try_from(value).map_err(|_| DataError::InvalidEncoding)?),
        };
        let descriptor = self.value_schema()?.ty(ty)?;
        let count = fields
            .len()
            .checked_sub(if matches!(descriptor, ValueType::ErrorRecord { .. }) {
                4
            } else {
                0
            })
            .ok_or(DataError::InvalidEncoding)?;
        child_types(descriptor, variant, count)?;
        Ok((variant, fields.take(count)))
    }

    /// Read only the validated collection header; never decode/copy its children.
    pub fn collection_length(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
        ty: usize,
    ) -> Result<usize, DataError> {
        let reference = self.reference(heap, handle, DataKind::Composite(ty))?;
        let (variant, fields) = self.composite_fields_iter(heap, reference, ty)?;
        if variant.is_some() {
            return Err(DataError::WrongKind);
        }
        match self.value_schema()?.ty(ty)? {
            ValueType::List(_) => Ok(fields.len()),
            ValueType::Map { .. } => Ok(fields.len() / 2),
            _ => Err(DataError::WrongKind),
        }
    }

    pub fn enum_variant(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
        ty: usize,
    ) -> Result<u32, DataError> {
        let reference = self.reference(heap, handle, DataKind::Composite(ty))?;
        let (variant, _) = self.composite_fields_iter(heap, reference, ty)?;
        if !matches!(self.value_schema()?.ty(ty)?, ValueType::Enum { .. }) {
            return Err(DataError::WrongKind);
        }
        variant.ok_or(DataError::InvalidEncoding)
    }

    /// Typed Domain-local mutation. There is no allocation or GC between resolving
    /// the receiver and the slot; the heap owns reference validation/barrier policy.
    pub fn set_state_field(
        &mut self,
        heap: &mut DomainHeap,
        handle: ManagedHandle,
        ty: usize,
        index: usize,
        slot: ManagedSlot,
    ) -> Result<(), DataError> {
        self.require_domain(heap)?;
        let ValueType::State { fields, .. } = self.value_schema()?.ty(ty)? else {
            return Err(DataError::WrongKind);
        };
        let child = *fields.get(index).ok_or(DataError::InvalidEncoding)?;
        let field = self.slot_field(heap, child, slot)?;
        let reference = self.reference(heap, handle, DataKind::Composite(ty))?;
        let _ = self.composite_fields_iter(heap, reference, ty)?;
        heap.write_field(reference, index + 2, field)?;
        Ok(())
    }

    /// A projected reference gets its own root before the parent can be released.
    pub fn project(
        &mut self,
        heap: &mut DomainHeap,
        handle: ManagedHandle,
        ty: usize,
        variant: Option<u32>,
        index: usize,
    ) -> Result<ManagedSlot, DataError> {
        let reference = self.reference(heap, handle, DataKind::Composite(ty))?;
        self.require_native_record_readable(heap, reference)?;
        if let ValueType::List(child) = self.value_schema()?.ty(ty)? {
            let child = *child;
            let header = heap.header(reference)?;
            if header.type_id != COMPOSITE_TYPE
                || variant.is_some()
                || usize::try_from(heap.integer_field(reference, 0)?).ok() != Some(ty)
                || heap.integer_field(reference, 1)? != -1
            {
                return Err(DataError::WrongKind);
            }
            let field = index
                .checked_add(2)
                .ok_or(DataError::AddressSpaceExhausted)?;
            return self.slot_from_field(heap, child, heap.field_at(reference, field)?);
        }
        let (actual, mut fields) = self.composite_fields_iter(heap, reference, ty)?;
        if variant != actual {
            return Err(ValueError::TypeMismatch.into());
        }
        let children = child_types(self.value_schema()?.ty(ty)?, actual, fields.len())?;
        let child = children.get(index).ok_or(DataError::InvalidEncoding)?;
        let field = fields.nth(index).ok_or(DataError::InvalidEncoding)?;
        self.slot_from_field(heap, child, field)
    }

    pub(super) fn slot_from_field(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        field: HeapValue,
    ) -> Result<ManagedSlot, DataError> {
        let decoded = self.decode_slot_field(heap, ty, field)?;
        self.root_decoded_slot(heap, decoded)
    }

    pub(super) fn root_decoded_slot(
        &mut self,
        heap: &mut DomainHeap,
        decoded: DecodedSlot,
    ) -> Result<ManagedSlot, DataError> {
        match decoded {
            DecodedSlot::Value(slot) => Ok(slot),
            DecodedSlot::Reference(reference, kind) => {
                self.root(heap, reference, kind).map(ManagedSlot::Reference)
            }
        }
    }

    pub(super) fn decode_rooted_slot(
        &self,
        heap: &DomainHeap,
        slot: ManagedSlot,
    ) -> Result<DecodedSlot, DataError> {
        match slot {
            ManagedSlot::Reference(handle) => {
                let (reference, kind) = self.any_reference(heap, handle)?;
                Ok(DecodedSlot::Reference(reference, kind))
            }
            value => Ok(DecodedSlot::Value(value)),
        }
    }

    pub(super) fn decode_slot_field(
        &self,
        heap: &DomainHeap,
        ty: usize,
        field: HeapValue,
    ) -> Result<DecodedSlot, DataError> {
        let descriptor = self.value_schema()?.ty(ty)?;
        match (descriptor, field) {
            (ValueType::Scalar(kind), HeapValue::Integer(bits)) => {
                ScalarValue::from_bits(*kind, bits as u64)
                    .map(|value| DecodedSlot::Value(ManagedSlot::Scalar(value)))
                    .map_err(|_| DataError::InvalidEncoding)
            }
            (ValueType::Unit, HeapValue::Null) => Ok(DecodedSlot::Value(ManagedSlot::Unit)),
            (ValueType::Other, HeapValue::Integer(bits)) => {
                Ok(DecodedSlot::Value(ManagedSlot::Opaque(bits as u64)))
            }
            (_, HeapValue::Reference(reference)) => {
                let kind = match descriptor {
                    ValueType::String => DataKind::String,
                    ValueType::Error => {
                        if heap.header(reference)?.type_id == ERROR_TYPE {
                            DataKind::Error
                        } else if heap.header(reference)?.type_id == COMPOSITE_TYPE {
                            DataKind::Composite(self.error_record_type_at(heap, reference)?)
                        } else {
                            DataKind::RichError(heap.integer_field(reference, 0)? as u64)
                        }
                    }
                    ValueType::Bytes(length) => DataKind::Bytes(*length),
                    ValueType::DynamicBytes => {
                        DataKind::Bytes(heap.integer_field(reference, 0)? as u64)
                    }
                    ValueType::Record { .. }
                    | ValueType::State { .. }
                    | ValueType::ErrorRecord { .. }
                    | ValueType::Enum { .. }
                    | ValueType::List(_)
                    | ValueType::Map { .. } => DataKind::Composite(ty),
                    ValueType::Other => match heap.header(reference)?.type_id {
                        STRING_TYPE => DataKind::String,
                        ERROR_TYPE => DataKind::Error,
                        native_opaque::OPAQUE_TYPE
                            if heap.integer_field(reference, 0)? as usize == ty
                                && heap.integer_field(reference, 1)? != 0 =>
                        {
                            DataKind::NativeOpaque(ty)
                        }
                        _ => return Err(DataError::WrongKind),
                    },
                    _ => return Err(DataError::WrongKind),
                };
                Ok(DecodedSlot::Reference(reference, kind))
            }
            _ => Err(DataError::InvalidEncoding),
        }
    }

    pub fn snapshot(
        &self,
        heap: &DomainHeap,
        ty: usize,
        slot: ManagedSlot,
    ) -> Result<Value, DataError> {
        let field = self.slot_field(heap, ty, slot)?;
        let value = self.snapshot_field(heap, ty, field)?;
        self.value_schema()?.validate(ty, &value)?;
        Ok(value)
    }

    /// Deep-copy a complete transport argument/result vector into this Domain.
    /// Validate every value and all variants before allocating any heap object.
    pub fn materialize_values(
        &mut self,
        heap: &mut DomainHeap,
        types: &[usize],
        values: &[Value],
    ) -> Result<Vec<ManagedSlot>, DataError> {
        self.require_domain(heap)?;
        if types.len() != values.len() {
            return Err(ValueError::TypeMismatch.into());
        }
        let schema = self.schema.clone().ok_or(ValueError::InvalidSchema)?;
        for (&ty, value) in types.iter().zip(values) {
            if !schema.has_transferable(ty)? {
                return Err(ValueError::NoEquality.into());
            }
            schema.validate(ty, value)?;
        }
        enum Walk<'a> {
            Visit(usize, &'a Value),
            Build(usize, Option<u32>, usize),
        }
        // root() issues globally increasing, never-reused identities. This
        // Owner-only construction cannot remove old roots or reenter managed
        // code, so every new root sorts after the last preexisting identity.
        let before = self.roots.last_key_value().map(|(handle, _)| *handle);
        let result = (|| {
            let mut work = types
                .iter()
                .copied()
                .zip(values)
                .rev()
                .map(|(ty, value)| Walk::Visit(ty, value))
                .collect::<Vec<_>>();
            let mut output = Vec::new();
            while let Some(step) = work.pop() {
                match step {
                    Walk::Visit(ty, value) => match (schema.ty(ty)?, value) {
                        (ValueType::Scalar(_), Value::Scalar(value)) => {
                            output.push(ManagedSlot::Scalar(*value))
                        }
                        (ValueType::Unit, Value::Unit) => output.push(ManagedSlot::Unit),
                        (ValueType::Error, Value::Error(value)) => output.push(
                            ManagedSlot::Reference(self.create_error_value(heap, value)?),
                        ),
                        (ValueType::String, Value::String(text)) => {
                            output.push(ManagedSlot::Reference(self.create_string(heap, text)?))
                        }
                        (ValueType::DynamicBytes, Value::Bytes(bytes)) => {
                            output.push(ManagedSlot::Reference(self.create_bytes(
                                heap,
                                bytes.len() as u64,
                                bytes,
                            )?))
                        }
                        (ValueType::Bytes(length), Value::Bytes(bytes)) => output.push(
                            ManagedSlot::Reference(self.create_bytes(heap, *length, bytes)?),
                        ),
                        (
                            ValueType::Map {
                                key,
                                value: value_type,
                            },
                            Value::Map(entries),
                        ) => {
                            let count = entries
                                .len()
                                .checked_mul(2)
                                .ok_or(DataError::AddressSpaceExhausted)?;
                            work.push(Walk::Build(ty, None, count));
                            for (k, v) in entries.iter().rev() {
                                work.push(Walk::Visit(*value_type, v));
                                work.push(Walk::Visit(*key, k));
                            }
                        }
                        (
                            descriptor,
                            Value::Record { fields, .. }
                            | Value::Enum { fields, .. }
                            | Value::List(fields),
                        ) => {
                            let variant = if let Value::Enum { variant, .. } = value {
                                Some(*variant)
                            } else {
                                None
                            };
                            let children = child_types(descriptor, variant, fields.len())?;
                            work.push(Walk::Build(ty, variant, fields.len()));
                            work.extend(
                                children
                                    .iter()
                                    .zip(fields)
                                    .rev()
                                    .map(|(ty, value)| Walk::Visit(ty, value)),
                            );
                        }
                        _ => return Err(DataError::InvalidEncoding),
                    },
                    Walk::Build(ty, variant, count) => {
                        let start = output
                            .len()
                            .checked_sub(count)
                            .ok_or(DataError::InvalidEncoding)?;
                        let fields = output.split_off(start);
                        output.push(ManagedSlot::Reference(
                            self.create_composite(heap, ty, variant, &fields)?,
                        ));
                    }
                }
            }
            if output.len() != types.len() {
                return Err(DataError::InvalidEncoding);
            }
            Ok(output)
        })();
        if result.is_err() {
            self.rollback_roots_after(heap, before)?;
        }
        result
    }

    fn rollback_roots_after(
        &mut self,
        heap: &mut DomainHeap,
        before: Option<ManagedHandle>,
    ) -> Result<(), DataError> {
        while let Some((&handle, _)) = self.roots.last_key_value() {
            if before.is_some_and(|previous| handle <= previous) {
                break;
            }
            self.release(heap, handle)?;
        }
        Ok(())
    }

    pub(super) fn snapshot_field(
        &self,
        heap: &DomainHeap,
        ty: usize,
        field: HeapValue,
    ) -> Result<Value, DataError> {
        enum Walk {
            Visit(usize, HeapValue),
            Build(usize, Option<u32>, usize),
        }
        let schema = self.value_schema()?;
        if let Some(value) = self.snapshot_leaf_field(heap, ty, schema.ty(ty)?, field)? {
            return Ok(value);
        }
        let mut work = vec![Walk::Visit(ty, field)];
        let mut output = Vec::new();
        while let Some(step) = work.pop() {
            match step {
                Walk::Visit(ty, field) => {
                    let descriptor = schema.ty(ty)?;
                    if let Some(value) = self.snapshot_leaf_field(heap, ty, descriptor, field)? {
                        output.push(value);
                    } else {
                        let HeapValue::Reference(reference) = field else {
                            return Err(DataError::InvalidEncoding);
                        };
                        let (variant, fields) = self.composite_fields_iter(heap, reference, ty)?;
                        let children = child_types(schema.ty(ty)?, variant, fields.len())?;
                        work.push(Walk::Build(ty, variant, fields.len()));
                        work.extend(
                            children
                                .iter()
                                .zip(fields)
                                .rev()
                                .map(|(ty, field)| Walk::Visit(ty, field)),
                        );
                    }
                }
                Walk::Build(ty, variant, count) => {
                    let fields = output.split_off(
                        output
                            .len()
                            .checked_sub(count)
                            .ok_or(DataError::InvalidEncoding)?,
                    );
                    output.push(match schema.ty(ty)? {
                        ValueType::Record { identity, .. } => Value::Record {
                            identity: identity.clone(),
                            fields,
                        },
                        ValueType::Enum { identity, .. } => Value::Enum {
                            identity: identity.clone(),
                            variant: variant.ok_or(DataError::InvalidEncoding)?,
                            fields,
                        },
                        ValueType::List(_) => Value::List(fields),
                        ValueType::Map { .. } => {
                            let mut fields = fields.into_iter();
                            let mut entries = Vec::with_capacity(count / 2);
                            while let Some(key) = fields.next() {
                                entries
                                    .push((key, fields.next().ok_or(DataError::InvalidEncoding)?));
                            }
                            Value::Map(entries)
                        }
                        _ => return Err(DataError::InvalidEncoding),
                    });
                }
            }
        }
        if output.len() != 1 {
            return Err(DataError::InvalidEncoding);
        }
        output.pop().ok_or(DataError::InvalidEncoding)
    }

    fn snapshot_leaf_field(
        &self,
        heap: &DomainHeap,
        ty: usize,
        descriptor: &ValueType,
        field: HeapValue,
    ) -> Result<Option<Value>, DataError> {
        Ok(Some(match (descriptor, field) {
            (ValueType::Scalar(kind), HeapValue::Integer(bits)) => Value::Scalar(
                ScalarValue::from_bits(*kind, bits as u64)
                    .map_err(|_| DataError::InvalidEncoding)?,
            ),
            (ValueType::Unit, HeapValue::Null) => Value::Unit,
            (
                ValueType::State { .. } | ValueType::ErrorRecord { .. },
                HeapValue::Reference(reference),
            ) => Value::Other {
                ty,
                slot: heap.header(reference)?.object_id,
            },
            (ValueType::Other, HeapValue::Integer(slot)) => Value::Other {
                ty,
                slot: slot as u64,
            },
            (ValueType::Other, HeapValue::Reference(_)) => return Err(DataError::WrongKind),
            (ValueType::Error, HeapValue::Reference(reference)) => {
                if heap.header(reference)?.type_id == COMPOSITE_TYPE {
                    Value::Other {
                        ty: self.error_record_type_at(heap, reference)?,
                        slot: heap.header(reference)?.object_id,
                    }
                } else {
                    Value::Error(Self::error_value_at(heap, reference)?)
                }
            }
            (ValueType::String, HeapValue::Reference(reference)) => {
                Value::String(Self::string_at(heap, reference)?)
            }
            (ValueType::DynamicBytes, HeapValue::Reference(reference)) => Value::Bytes(
                Self::bytes_at(heap, reference, heap.integer_field(reference, 0)? as u64)?,
            ),
            (ValueType::Bytes(length), HeapValue::Reference(reference)) => {
                Value::Bytes(Self::bytes_at(heap, reference, *length)?)
            }
            (
                ValueType::Record { .. }
                | ValueType::Enum { .. }
                | ValueType::List(_)
                | ValueType::Map { .. },
                HeapValue::Reference(_),
            ) => return Ok(None),
            _ => return Err(DataError::InvalidEncoding),
        }))
    }

    pub fn values_equal(
        &self,
        heap: &DomainHeap,
        ty: usize,
        left: ManagedSlot,
        right: ManagedSlot,
    ) -> Result<bool, DataError> {
        self.require_domain(heap)?;
        let schema = self.value_schema()?;
        if !schema.has_transferable(ty)? {
            return Err(ValueError::NoEquality.into());
        }
        Ok(schema.equal(
            ty,
            &self.snapshot(heap, ty, left)?,
            &self.snapshot(heap, ty, right)?,
        )?)
    }

    pub fn value_hash(
        &self,
        heap: &DomainHeap,
        ty: usize,
        value: ManagedSlot,
    ) -> Result<u64, DataError> {
        self.require_domain(heap)?;
        let schema = self.value_schema()?;
        if !schema.is_key(ty)? {
            return Err(ValueError::NotKey.into());
        }
        Ok(schema.key_hash(ty, &self.snapshot(heap, ty, value)?)?)
    }
}

pub(super) enum ChildTypes<'a> {
    Fields(&'a [usize]),
    List(usize, usize),
    Map(usize, usize, usize),
}
impl ChildTypes<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Fields(fields) => fields.len(),
            Self::List(_, count) | Self::Map(_, _, count) => *count,
        }
    }
    fn get(&self, index: usize) -> Option<usize> {
        if index >= self.len() {
            return None;
        }
        Some(match self {
            Self::Fields(fields) => fields[index],
            Self::List(element, _) => *element,
            Self::Map(key, value, _) => {
                if index.is_multiple_of(2) {
                    *key
                } else {
                    *value
                }
            }
        })
    }
    pub(super) fn iter(&self) -> impl ExactSizeIterator<Item = usize> + DoubleEndedIterator + '_ {
        (0..self.len()).map(|index| self.get(index).expect("validated child index"))
    }
}

pub(super) fn child_types(
    descriptor: &ValueType,
    variant: Option<u32>,
    count: usize,
) -> Result<ChildTypes<'_>, DataError> {
    let fields = match (descriptor, variant) {
        (
            ValueType::Record { fields, .. }
            | ValueType::State { fields, .. }
            | ValueType::ErrorRecord { fields, .. },
            None,
        ) if fields.len() == count => ChildTypes::Fields(fields),
        (ValueType::Enum { variants, .. }, Some(variant)) => ChildTypes::Fields(
            variants
                .get(variant as usize)
                .filter(|fields| fields.len() == count)
                .ok_or(ValueError::TypeMismatch)?,
        ),
        (ValueType::List(element), None) => ChildTypes::List(*element, count),
        (ValueType::Map { key, value }, None) if count.is_multiple_of(2) => {
            ChildTypes::Map(*key, *value, count)
        }
        _ => return Err(ValueError::TypeMismatch.into()),
    };
    Ok(fields)
}

#[cfg(test)]
mod rollback_tests {
    use super::*;

    #[test]
    fn root_checkpoint_keeps_existing_aliases_and_ignores_other_domains() {
        for resident in [0, 1, 1024] {
            let mut heap = DomainHeap::new(740);
            let mut data = ManagedData::new(740);
            let originals: Vec<_> = (0..resident)
                .map(|_| data.create_string(&mut heap, "keep🙂").unwrap())
                .collect();
            let retired = data.create_string(&mut heap, "retired").unwrap();
            data.release(&mut heap, retired).unwrap();
            let before = data.roots.last_key_value().map(|(handle, _)| *handle);
            let mut other_heap = DomainHeap::new(741);
            let mut other = ManagedData::new(741);
            let foreign = other.create_string(&mut other_heap, "foreign").unwrap();
            let temporary = data.create_string(&mut heap, "rollback").unwrap();
            if let Some(&original) = originals.first() {
                let reference = data.reference(&heap, original, DataKind::String).unwrap();
                data.root(&mut heap, reference, DataKind::String).unwrap();
            }
            data.rollback_roots_after(&mut heap, before).unwrap();
            assert_eq!(data.root_count(), resident);
            assert_eq!(data.string(&heap, temporary), Err(DataError::UnknownHandle));
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert_eq!(heap.object_count(), resident);
            for original in originals {
                assert_eq!(data.string(&heap, original).unwrap(), "keep🙂");
            }
            assert_eq!(other.string(&other_heap, foreign).unwrap(), "foreign");
        }
    }
}

//! Error references retained by pending failures and Future outcomes. A lease
//! keeps only the Error's managed graph alive, never the producing stack frame.
use super::*;
use crate::value::{ValueError, ValueType};
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedError {
    domain: u64,
    handle: ManagedHandle,
    live: Arc<()>,
}

impl ManagedData {
    pub fn first_error_trace(
        &mut self,
        heap: &mut DomainHeap,
        handle: ManagedHandle,
        trace: Arc<crate::error_trace::ErrorTrace>,
    ) -> Result<Arc<crate::error_trace::ErrorTrace>, DataError> {
        self.error_record_type(heap, handle)?;
        Ok(heap.first_error_trace(self.any_reference(heap, handle)?.0, trace)?)
    }
    pub fn error_trace(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
    ) -> Result<Option<Arc<crate::error_trace::ErrorTrace>>, DataError> {
        self.error_record_type(heap, handle)?;
        Ok(heap.error_trace(self.any_reference(heap, handle)?.0)?)
    }
    pub fn error_is(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
        target: usize,
    ) -> Result<bool, DataError> {
        if !matches!(
            self.value_schema()?.ty(target)?,
            ValueType::ErrorRecord { .. }
        ) {
            return Err(DataError::WrongKind);
        }
        Ok(self.error_record_type(heap, handle)? == Some(target))
    }
    pub fn error_list_type(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
    ) -> Result<usize, DataError> {
        let DataKind::Composite(ty) = self.any_reference(heap, handle)?.1 else {
            return Err(DataError::WrongKind);
        };
        self.error_list_fields(heap, handle, ty)?;
        Ok(ty)
    }
    pub fn error_metadata(
        &mut self,
        heap: &mut DomainHeap,
        handle: ManagedHandle,
        field: usize,
        result: usize,
    ) -> Result<ManagedSlot, DataError> {
        if let Some(ty) = self.error_record_type(heap, handle)? {
            let cached = self.project(heap, handle, ty, None, field)?;
            if field != 2 {
                return Ok(cached);
            }
            let ManagedSlot::Reference(cached_handle) = cached else {
                return Err(DataError::InvalidEncoding);
            };
            if self.enum_variant(heap, cached_handle, result)? != 0 {
                return Ok(cached);
            }
            let source = self.error_source(heap, handle)?;
            if source.is_none() {
                return Ok(cached);
            }
            let option = self.source_option(heap, source, result)?;
            let error = self.any_reference(heap, handle)?.0;
            let value = self.any_reference(heap, option)?.0;
            heap.write_field(error, 4, HeapValue::Reference(value))?;
            return Ok(ManagedSlot::Reference(option));
        }
        // Legacy built-in Errors are still migrated separately from managed
        // Error records. Their immutable diagnostic data never contains a
        // managed payload, so this adapter cannot lose reference identity.
        match field {
            0 => Ok(ManagedSlot::Reference(self.create_string(heap, "Error")?)),
            1 => Ok(ManagedSlot::Reference(self.error_message(heap, handle)?)),
            2 => {
                let source = self.error_source(heap, handle)?;
                Ok(ManagedSlot::Reference(
                    self.source_option(heap, source, result)?,
                ))
            }
            3 => {
                let value = self.error_value(heap, handle)?;
                let cause = value.root().cause.map(|index| value.prefix(index));
                let slots = if let Some(cause) = cause {
                    vec![ManagedSlot::Reference(
                        self.create_error_value(heap, &cause)?,
                    )]
                } else {
                    Vec::new()
                };
                Ok(ManagedSlot::Reference(self.create_composite(
                    heap,
                    result,
                    Some(u32::from(!slots.is_empty())),
                    &slots,
                )?))
            }
            _ => Err(DataError::WrongKind),
        }
    }
    fn error_list_fields(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
        ty: usize,
    ) -> Result<Vec<HeapValue>, DataError> {
        let ValueType::List(element) = self.value_schema()?.ty(ty)? else {
            return Err(DataError::WrongKind);
        };
        if !matches!(self.value_schema()?.ty(*element)?, ValueType::Error) {
            return Err(DataError::WrongKind);
        }
        let reference = self.reference(heap, handle, DataKind::Composite(ty))?;
        Ok(self.composite_fields(heap, reference, ty)?.1)
    }

    /// The private list roots the materialized Errors. Its diagnostic snapshot
    /// contains no leases and is retired with the list's scoped handle.
    pub fn catch_error_list(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        errors: &[Failure],
        preserve: bool,
    ) -> Result<ManagedHandle, DataError> {
        let mut values = Vec::new();
        for failure in errors {
            for index in 0..=failure.cleanup.len() {
                let diagnostic = failure.diagnostic_at(index).expect("failure element");
                values.push(ManagedSlot::Reference(
                    self.create_error(heap, &diagnostic)?,
                ));
            }
        }
        let list = self.create_composite(heap, ty, None, &values)?;
        if preserve
            && let Some(snapshot) =
                crate::error_groups::aggregate(errors.iter().map(Failure::diagnostics_only))
        {
            self.caught_errors.insert(list, snapshot);
        }
        Ok(list)
    }

    pub fn rethrow_caught(
        &mut self,
        heap: &mut DomainHeap,
        list: ManagedHandle,
        elements: &[u32],
        tail: Option<u32>,
    ) -> Result<Failure, DataError> {
        let snapshot = self
            .caught_errors
            .get(&list)
            .cloned()
            .ok_or(DataError::WrongKind)?;
        let ty = self.error_list_type(heap, list)?;
        let ValueType::List(element) = *self.value_schema()?.ty(ty)? else {
            return Err(DataError::WrongKind);
        };
        let fields = self.error_list_fields(heap, list, ty)?;
        if elements.windows(2).any(|pair| pair[0] >= pair[1])
            || tail.is_some_and(|start| {
                start as usize > fields.len()
                    || elements.last().is_some_and(|index| *index >= start)
            })
        {
            return Err(DataError::InvalidEncoding);
        }
        let indices = elements.iter().map(|index| *index as usize).chain(
            tail.into_iter()
                .flat_map(|start| start as usize..fields.len()),
        );
        let mut result: Option<Failure> = None;
        for index in indices {
            let field = *fields.get(index).ok_or(DataError::InvalidEncoding)?;
            let ManagedSlot::Reference(handle) = self.slot_from_field(heap, element, field)? else {
                return Err(DataError::WrongKind);
            };
            let mut failure = snapshot
                .diagnostic_at(index)
                .ok_or(DataError::InvalidEncoding)?;
            failure.managed.insert(0, self.lease_error(heap, handle)?);
            if let Some(result) = &mut result {
                result.append_cleanup(failure);
            } else {
                result = Some(failure);
            }
        }
        result.ok_or(DataError::InvalidEncoding)
    }

    pub fn match_error_list(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
        ty: usize,
        expected: &[usize],
        rest: bool,
    ) -> Result<bool, DataError> {
        let fields = self.error_list_fields(heap, handle, ty)?;
        if fields.is_empty()
            || if rest {
                fields.len() < expected.len()
            } else {
                fields.len() != expected.len()
            }
        {
            return Ok(false);
        }
        for (&field, &expected) in fields.iter().zip(expected) {
            let HeapValue::Reference(reference) = field else {
                return Err(DataError::InvalidEncoding);
            };
            match self.value_schema()?.ty(expected)? {
                ValueType::Error => {}
                ValueType::ErrorRecord { .. } => {
                    if heap.header(reference)?.type_id != COMPOSITE_TYPE
                        || self.error_record_type_at(heap, reference)? != expected
                    {
                        return Ok(false);
                    }
                }
                _ => return Err(DataError::WrongKind),
            }
        }
        Ok(true)
    }

    pub fn error_list_rest(
        &mut self,
        heap: &mut DomainHeap,
        handle: ManagedHandle,
        ty: usize,
        start: usize,
    ) -> Result<ManagedHandle, DataError> {
        let fields = self.error_list_fields(heap, handle, ty)?;
        let fields = fields.get(start..).ok_or(DataError::InvalidEncoding)?;
        let ValueType::List(element) = *self.value_schema()?.ty(ty)? else {
            unreachable!()
        };
        let slots = fields
            .iter()
            .map(|field| self.slot_from_field(heap, element, *field))
            .collect::<Result<Vec<_>, _>>()?;
        self.create_composite(heap, ty, None, &slots)
    }
    pub fn error_record_type(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
    ) -> Result<Option<usize>, DataError> {
        match self.any_reference(heap, handle)?.1 {
            DataKind::Error | DataKind::RichError(_) => Ok(None),
            DataKind::Composite(ty)
                if matches!(self.value_schema()?.ty(ty)?, ValueType::ErrorRecord { .. }) =>
            {
                Ok(Some(ty))
            }
            _ => Err(DataError::WrongKind),
        }
    }

    pub fn lease_error(
        &mut self,
        heap: &DomainHeap,
        handle: ManagedHandle,
    ) -> Result<ManagedError, DataError> {
        self.error_record_type(heap, handle)?;
        let live = self
            .error_leases
            .get(&handle)
            .and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| {
                let live = Arc::new(());
                self.error_leases.insert(handle, Arc::downgrade(&live));
                live
            });
        Ok(ManagedError {
            domain: self.domain,
            handle,
            live,
        })
    }

    pub fn leased_error(
        &self,
        heap: &DomainHeap,
        error: &ManagedError,
    ) -> Result<ManagedHandle, DataError> {
        self.require_domain(heap)?;
        if error.domain != self.domain {
            return Err(DataError::WrongDomain);
        }
        self.error_record_type(heap, error.handle)?;
        Ok(error.handle)
    }

    /// The first four fields have a fixed layout across nominal Error types.
    /// Payload access uses the concrete schema, after a checked type match.
    pub fn error_record_field(
        &mut self,
        heap: &mut DomainHeap,
        handle: ManagedHandle,
        field: usize,
    ) -> Result<ManagedSlot, DataError> {
        let ty = self
            .error_record_type(heap, handle)?
            .ok_or(DataError::WrongKind)?;
        if field == 2 {
            let ValueType::ErrorRecord { fields, .. } = self.value_schema()?.ty(ty)? else {
                return Err(DataError::WrongKind);
            };
            return self.error_metadata(heap, handle, field, fields[2]);
        }
        self.project(heap, handle, ty, None, field)
    }

    pub(super) fn error_record_type_at(
        &self,
        heap: &DomainHeap,
        reference: HeapRef,
    ) -> Result<usize, DataError> {
        if heap.header(reference)?.type_id != COMPOSITE_TYPE {
            return Err(DataError::WrongKind);
        }
        let ty = usize::try_from(heap.integer_field(reference, 0)?)
            .map_err(|_| DataError::InvalidEncoding)?;
        if !matches!(self.value_schema()?.ty(ty)?, ValueType::ErrorRecord { .. }) {
            return Err(ValueError::TypeMismatch.into());
        }
        Ok(ty)
    }

    pub(super) fn error_message_at(
        &self,
        heap: &DomainHeap,
        reference: HeapRef,
    ) -> Result<String, DataError> {
        if heap.header(reference)?.type_id == COMPOSITE_TYPE {
            self.error_record_type_at(heap, reference)?;
            let Some(HeapValue::Reference(message)) = heap.fields(reference)?.get(3).copied()
            else {
                return Err(DataError::InvalidEncoding);
            };
            Self::string_at(heap, message)
        } else {
            Ok(Self::error_value_at(heap, reference)?
                .root()
                .message
                .clone())
        }
    }
}

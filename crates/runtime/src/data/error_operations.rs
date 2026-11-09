use super::*;
use crate::{
    error_value::ErrorValue,
    scalar::{ScalarKind, ScalarValue},
    value::Value,
};
use rinzu_abi::data_operation::DataOperation;

impl ManagedData {
    pub fn create_error_value(
        &mut self,
        heap: &mut DomainHeap,
        value: &ErrorValue,
    ) -> Result<ManagedHandle, DataError> {
        if !value.validate() {
            return Err(DataError::InvalidEncoding);
        }
        let bytes = value.encode();
        let handle = self.create_bytes(heap, bytes.len() as u64, &bytes)?;
        self.roots.get_mut(&handle).unwrap().1 = DataKind::RichError(bytes.len() as u64);
        Ok(handle)
    }
    pub(super) fn error_value_at(
        heap: &DomainHeap,
        reference: HeapRef,
    ) -> Result<ErrorValue, DataError> {
        if heap.header(reference)?.type_id == ERROR_TYPE {
            return Ok(ErrorValue::from_failure(&Self::failure_at(
                heap, reference,
            )?));
        }
        let bytes = Self::bytes_at(heap, reference, heap.integer_field(reference, 0)? as u64)?;
        ErrorValue::decode(&bytes).ok_or(DataError::InvalidEncoding)
    }
    pub fn error_value(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
    ) -> Result<ErrorValue, DataError> {
        let (reference, kind) = self.any_reference(heap, handle)?;
        if !matches!(kind, DataKind::Error | DataKind::RichError(_)) {
            return Err(DataError::WrongKind);
        }
        Self::error_value_at(heap, reference)
    }
    pub(super) fn error_operation(
        &mut self,
        heap: &mut DomainHeap,
        op: DataOperation,
        result: usize,
        types: &[usize],
        slots: &[ManagedSlot],
    ) -> Result<ManagedSlot, DataError> {
        use DataOperation::*;
        let scalar = |kind, bits| {
            ManagedSlot::Scalar(ScalarValue::from_bits(kind, bits).expect("validated scalar"))
        };
        let scalar_value = |slot| match slot {
            ManagedSlot::Scalar(value) => Ok(value.bits()),
            _ => Err(DataError::WrongKind),
        };
        let text = |data: &Self, index| match data.snapshot(heap, types[index], slots[index])? {
            Value::String(s) => Ok(s),
            _ => Err(DataError::WrongKind),
        };
        if matches!(op, ErrorCreate | NativeStatus) {
            let code = scalar_value(slots[0])? as i64;
            let message = text(self, 1)?;
            if op == ErrorCreate {
                return Ok(ManagedSlot::Reference(
                    self.create_error_value(heap, &ErrorValue::new(code, message))?,
                ));
            }
            if code == 0 {
                return Ok(scalar(ScalarKind::I64, 0));
            }
            let mut value = ErrorValue::new(100, "native operation failed").with_context(message);
            value.nodes.last_mut().unwrap().native_status = Some(code);
            return Err(DataError::Thrown(value));
        }
        let ManagedSlot::Reference(handle) = slots[0] else {
            return Err(DataError::WrongKind);
        };
        if op == ErrorMessage {
            return self.error_message(heap, handle).map(ManagedSlot::Reference);
        }
        if op == ErrorCause {
            return self.error_metadata(heap, handle, 3, result);
        }
        if matches!(op, ErrorSourceFile | ErrorSourceStart | ErrorSourceEnd) {
            let position = self.error_source(heap, handle)?;
            let child = position.map(|position| match op {
                ErrorSourceFile => scalar(ScalarKind::U64, position.file),
                ErrorSourceStart => scalar(ScalarKind::I64, u64::from(position.start_byte)),
                _ => scalar(ScalarKind::I64, u64::from(position.end_byte)),
            });
            return self.error_option(heap, result, child);
        }
        let named = self.error_record_type(heap, handle)?.is_some();
        if named && op == ErrorCleanup {
            return Ok(ManagedSlot::Reference(self.create_composite(
                heap,
                result,
                None,
                &[],
            )?));
        }
        let cause = if matches!(op, ErrorWithCause | ErrorWithCauseOption) {
            let ManagedSlot::Reference(cause) = slots[1] else {
                return Err(DataError::WrongKind);
            };
            if op == ErrorWithCauseOption && self.error_record_type(heap, cause)?.is_some() {
                return self.error_option(heap, result, None);
            }
            Some(cause)
        } else {
            None
        };
        if named && (op.is_optional_error_metadata() || op == ErrorNativeStatus) {
            return self.error_option(heap, result, None);
        }
        let value = self.error_value(heap, handle)?;
        if op == ErrorCode {
            return Ok(scalar(ScalarKind::I64, value.root().code as u64));
        }
        if op == ErrorContext {
            return Ok(ManagedSlot::Reference(
                self.create_string(heap, &value.root().context)?,
            ));
        }
        if matches!(
            op,
            ErrorWithContext | ErrorWithCause | ErrorWithContextOption | ErrorWithCauseOption
        ) {
            let value = if let Some(cause) = cause {
                value
                    .with_cause(&self.error_value(heap, cause)?)
                    .map_err(DataError::Thrown)?
            } else {
                value.with_context(text(self, 1)?)
            };
            let updated = self.create_error_value(heap, &value)?;
            if op.is_optional_error_metadata() {
                if let Some(trace) = self.error_trace(heap, handle)? {
                    self.first_error_trace(heap, updated, trace)?;
                }
                return self.error_option(heap, result, Some(ManagedSlot::Reference(updated)));
            }
            return Ok(ManagedSlot::Reference(updated));
        }
        if op == ErrorCleanup {
            let children = value
                .root()
                .cleanup
                .iter()
                .map(|i| {
                    self.create_error_value(heap, &value.prefix(*i))
                        .map(ManagedSlot::Reference)
                })
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(ManagedSlot::Reference(
                self.create_composite(heap, result, None, &children)?,
            ));
        }
        let child = match op {
            ErrorCodeOption => Some(scalar(ScalarKind::I64, value.root().code as u64)),
            ErrorContextOption => Some(ManagedSlot::Reference(
                self.create_string(heap, &value.root().context)?,
            )),
            ErrorNativeStatus => value
                .root()
                .native_status
                .map(|v| scalar(ScalarKind::I64, v as u64)),
            _ => return Err(DataError::WrongKind),
        };
        self.error_option(heap, result, child)
    }

    fn error_option(
        &mut self,
        heap: &mut DomainHeap,
        result: usize,
        child: Option<ManagedSlot>,
    ) -> Result<ManagedSlot, DataError> {
        Ok(ManagedSlot::Reference(self.create_composite(
            heap,
            result,
            Some(u32::from(child.is_some())),
            child.as_slice(),
        )?))
    }
}

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
        let value = self.error_value(heap, handle)?;
        if op == ErrorCode {
            return Ok(scalar(ScalarKind::I64, value.root().code as u64));
        }
        if matches!(op, ErrorMessage | ErrorContext) {
            let text = if op == ErrorMessage {
                &value.root().message
            } else {
                &value.root().context
            };
            return Ok(ManagedSlot::Reference(self.create_string(heap, text)?));
        }
        if op == ErrorWithContext {
            let value = value.with_context(text(self, 1)?);
            return Ok(ManagedSlot::Reference(
                self.create_error_value(heap, &value)?,
            ));
        }
        if op == ErrorWithCause {
            let ManagedSlot::Reference(cause) = slots[1] else {
                return Err(DataError::WrongKind);
            };
            let value = value
                .with_cause(&self.error_value(heap, cause)?)
                .map_err(DataError::Thrown)?;
            return Ok(ManagedSlot::Reference(
                self.create_error_value(heap, &value)?,
            ));
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
            ErrorCause => {
                if let Some(i) = value.root().cause {
                    Some(ManagedSlot::Reference(
                        self.create_error_value(heap, &value.prefix(i))?,
                    ))
                } else {
                    None
                }
            }
            ErrorNativeStatus => value
                .root()
                .native_status
                .map(|v| scalar(ScalarKind::I64, v as u64)),
            ErrorSourceFile => value.root().source.map(|v| scalar(ScalarKind::U64, v.file)),
            ErrorSourceStart => value
                .root()
                .source
                .map(|v| scalar(ScalarKind::I64, v.start_byte as u64)),
            ErrorSourceEnd => value
                .root()
                .source
                .map(|v| scalar(ScalarKind::I64, v.end_byte as u64)),
            _ => return Err(DataError::WrongKind),
        };
        Ok(ManagedSlot::Reference(self.create_composite(
            heap,
            result,
            Some(u32::from(child.is_some())),
            &child.into_iter().collect::<Vec<_>>(),
        )?))
    }
}

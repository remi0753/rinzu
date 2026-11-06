use super::{DataError, DomainHeap, ManagedData, ManagedHandle, ManagedSlot};
use crate::scalar::{ScalarKind, ScalarValue};
use crate::value::ValueType;

/// Input and cached managed snapshot owned by a single Main invocation.
#[derive(Default)]
pub struct ProgramArguments {
    raw: Vec<Vec<u8>>,
    cached: Option<ManagedHandle>,
}

impl ProgramArguments {
    pub fn new(raw: Vec<Vec<u8>>) -> Self {
        Self { raw, cached: None }
    }

    pub fn root(&self) -> Option<ManagedHandle> {
        self.cached
    }

    pub fn snapshot(
        &mut self,
        data: &mut ManagedData,
        heap: &mut DomainHeap,
        result: usize,
    ) -> Result<ManagedSlot, DataError> {
        if let Some(handle) = self.cached {
            return Ok(ManagedSlot::Reference(handle));
        }
        let schema = data.value_schema()?;
        let ValueType::Enum { variants, .. } = schema.ty(result)? else {
            return Err(DataError::WrongKind);
        };
        let [ready, invalid] = variants.as_slice() else {
            return Err(DataError::InvalidEncoding);
        };
        let [list] = ready.as_slice() else {
            return Err(DataError::InvalidEncoding);
        };
        let ValueType::List(element) = schema.ty(*list)? else {
            return Err(DataError::WrongKind);
        };
        if !matches!(schema.ty(*element)?, ValueType::String)
            || invalid.len() != 2
            || invalid
                .iter()
                .any(|ty| !matches!(schema.ty(*ty), Ok(ValueType::Scalar(ScalarKind::I64))))
        {
            return Err(DataError::InvalidEncoding);
        }
        let list = *list;
        let mut strings = Vec::with_capacity(self.raw.len());
        for (index, bytes) in self.raw.iter().enumerate() {
            match std::str::from_utf8(bytes) {
                Ok(text) => strings.push(text),
                Err(error) => {
                    let int = |value: usize| -> Result<ManagedSlot, DataError> {
                        let value =
                            i64::try_from(value).map_err(|_| DataError::AddressSpaceExhausted)?;
                        Ok(ManagedSlot::Scalar(
                            ScalarValue::from_bits(ScalarKind::I64, value as u64).unwrap(),
                        ))
                    };
                    return data
                        .create_composite(
                            heap,
                            result,
                            Some(1),
                            &[int(index)?, int(error.valid_up_to())?],
                        )
                        .map(ManagedSlot::Reference);
                }
            }
        }

        let mut slots = Vec::with_capacity(strings.len());
        let snapshot = (|| {
            for text in strings {
                slots.push(ManagedSlot::Reference(data.create_string(heap, text)?));
            }
            let values = data.create_composite(heap, list, None, &slots)?;
            let snapshot =
                data.create_composite(heap, result, Some(0), &[ManagedSlot::Reference(values)]);
            data.release(heap, values)?;
            snapshot
        })();
        for slot in slots {
            if let ManagedSlot::Reference(handle) = slot {
                data.release(heap, handle)?;
            }
        }
        let handle = snapshot?;
        self.cached = Some(handle);
        Ok(ManagedSlot::Reference(handle))
    }
}

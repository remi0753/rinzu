//! First-failure positions live with Errors, without retaining execution frames.
use super::*;
use crate::value::ValueType;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct SourceCatalog {
    pub files: BTreeMap<u64, String>,
    pub positions: BTreeMap<u64, BTreeMap<u32, (u32, u32)>>,
}

impl ManagedData {
    pub fn configure_sources(
        &mut self,
        heap: &DomainHeap,
        catalog: Arc<SourceCatalog>,
    ) -> Result<(), DataError> {
        self.require_domain(heap)?;
        if catalog.positions.iter().any(|(file, positions)| {
            !catalog.files.contains_key(file)
                || positions
                    .values()
                    .any(|&(line, column)| line == 0 || column == 0)
        }) {
            return Err(DataError::InvalidEncoding);
        }
        self.source_catalog = catalog;
        Ok(())
    }

    pub fn error_source(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
    ) -> Result<Option<SourcePosition>, DataError> {
        let (reference, kind) = self.any_reference(heap, handle)?;
        if let DataKind::Composite(ty) = kind {
            let ValueType::ErrorRecord { fields, .. } = self.value_schema()?.ty(ty)? else {
                return Err(DataError::WrongKind);
            };
            let offset = fields.len() + 2;
            if heap.header(reference)?.field_count as usize != offset + 4 {
                return Err(DataError::InvalidEncoding);
            }
            return match heap.integer_field(reference, offset)? {
                0 => Ok(None),
                1 => Ok(Some(SourcePosition {
                    file: heap.integer_field(reference, offset + 1)? as u64,
                    start_byte: u32::try_from(heap.integer_field(reference, offset + 2)?)
                        .map_err(|_| DataError::InvalidEncoding)?,
                    end_byte: u32::try_from(heap.integer_field(reference, offset + 3)?)
                        .map_err(|_| DataError::InvalidEncoding)?,
                })),
                _ => Err(DataError::InvalidEncoding),
            };
        }
        Ok(self.error_value(heap, handle)?.root().source)
    }

    pub fn first_error_source(
        &mut self,
        heap: &mut DomainHeap,
        handle: ManagedHandle,
        position: Option<SourcePosition>,
    ) -> Result<Option<SourcePosition>, DataError> {
        if let Some(original) = self.error_source(heap, handle)? {
            return Ok(Some(original));
        }
        let Some(position) = position else {
            return Ok(None);
        };
        let (reference, kind) = self.any_reference(heap, handle)?;
        match kind {
            DataKind::Composite(ty) => {
                let ValueType::ErrorRecord { fields, .. } = self.value_schema()?.ty(ty)? else {
                    return Err(DataError::WrongKind);
                };
                let offset = fields.len() + 2;
                for (index, value) in [
                    1,
                    position.file as i64,
                    i64::from(position.start_byte),
                    i64::from(position.end_byte),
                ]
                .into_iter()
                .enumerate()
                {
                    heap.write_field(reference, offset + index, HeapValue::Integer(value))?;
                }
            }
            DataKind::Error => {
                for (index, value) in [
                    1,
                    position.file as i64,
                    i64::from(position.start_byte),
                    i64::from(position.end_byte),
                ]
                .into_iter()
                .enumerate()
                {
                    heap.write_field(reference, 3 + index, HeapValue::Integer(value))?;
                }
            }
            DataKind::RichError(length) => {
                let mut value = self.error_value(heap, handle)?;
                value
                    .nodes
                    .last_mut()
                    .ok_or(DataError::InvalidEncoding)?
                    .source = Some(position);
                let bytes = value.encode();
                if bytes.len() as u64 != length {
                    return Err(DataError::InvalidEncoding);
                }
                // Legacy metadata has fixed-width position fields, including
                // absent positions. Preserve the original handle and graph.
                let words = bytes.chunks(8).map(|chunk| {
                    let mut word = [0; 8];
                    word[..chunk.len()].copy_from_slice(chunk);
                    u64::from_le_bytes(word)
                });
                heap.write_integer_fields(reference, 1, words)?;
            }
            _ => return Err(DataError::WrongKind),
        }
        Ok(Some(position))
    }

    pub(super) fn source_option(
        &mut self,
        heap: &mut DomainHeap,
        source: Option<SourcePosition>,
        option: usize,
    ) -> Result<ManagedHandle, DataError> {
        let catalog = Arc::clone(&self.source_catalog);
        let resolved = source.and_then(|source| {
            let &(line, column) = catalog
                .positions
                .get(&source.file)?
                .get(&source.start_byte)?;
            Some((catalog.files.get(&source.file)?.as_str(), line, column))
        });
        let Some((file, line, column)) = resolved else {
            return self.create_composite(heap, option, Some(0), &[]);
        };
        let ValueType::Enum { variants, .. } = self.value_schema()?.ty(option)? else {
            return Err(DataError::WrongKind);
        };
        let &[location] = variants
            .get(1)
            .ok_or(DataError::InvalidEncoding)?
            .as_slice()
        else {
            return Err(DataError::InvalidEncoding);
        };
        let file = self.create_string(heap, file)?;
        let int = |value: u32| {
            ManagedSlot::Scalar(
                crate::scalar::ScalarValue::from_bits(
                    crate::scalar::ScalarKind::I64,
                    u64::from(value),
                )
                .unwrap(),
            )
        };
        let value = self.create_composite(
            heap,
            location,
            None,
            &[ManagedSlot::Reference(file), int(line), int(column)],
        )?;
        self.create_composite(heap, option, Some(1), &[ManagedSlot::Reference(value)])
    }
}

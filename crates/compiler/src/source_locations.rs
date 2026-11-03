//! Compact coordinates from the exact source snapshot used for compilation.
use crate::{IrModule, SourceFile};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceCoordinates {
    pub line: u32,
    pub column: u32,
}

impl IrModule {
    pub(crate) fn embed_source_locations<'a>(
        &mut self,
        sources: impl IntoIterator<Item = &'a SourceFile>,
    ) {
        let mut used = BTreeMap::<u32, BTreeSet<u32>>::new();
        let mut record = |range: crate::SourceRange| {
            used.entry(range.file_id.0)
                .or_default()
                .insert(range.start_byte);
        };
        for function in &self.functions {
            record(function.range);
            for block in &function.blocks {
                record(block.range);
                for operation in &block.operations {
                    record(operation.range);
                }
            }
        }
        for file in sources {
            if !self.source_files.contains_key(&file.id().0) {
                continue;
            }
            let Some(offsets) = used.get(&file.id().0) else {
                continue;
            };
            // Offsets are sorted. Walk each consumed character once, keeping
            // the same LF/Unicode scalar convention as SourceFile::line_column.
            let mut previous = 0;
            let mut line = 1;
            let mut column = 1;
            let positions = self.source_locations.entry(file.id().0).or_default();
            for &byte in offsets {
                let Some(offset) = (byte as usize).checked_sub(file.original_offset(0)) else {
                    continue;
                };
                if !file.text().is_char_boundary(offset) {
                    continue;
                }
                for character in file.text()[previous..offset].chars() {
                    if character == '\n' {
                        line += 1;
                        column = 1;
                    } else {
                        column += 1;
                    }
                }
                positions.insert(byte, SourceCoordinates { line, column });
                previous = offset;
            }
        }
    }
}

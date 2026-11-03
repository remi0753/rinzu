use std::path::{Path, PathBuf};

use crate::{Diagnostic, DiagnosticCode};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileId(pub u32);

impl FileId {
    pub const MANIFEST: Self = Self(0);
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceRange {
    pub file_id: FileId,
    pub start_byte: u32,
    pub end_byte: u32,
    /// Internal concrete instance; physical file/byte identity is unchanged.
    pub instance: u32,
}

/// Zero-based editor line and UTF-16 code-unit column, excluding a file BOM.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Utf16Position {
    pub line: u32,
    pub character: u32,
}

impl SourceRange {
    #[must_use]
    pub const fn new(file_id: FileId, start_byte: usize, end_byte: usize) -> Self {
        Self {
            file_id,
            start_byte: start_byte as u32,
            end_byte: end_byte as u32,
            instance: 0,
        }
    }

    #[must_use]
    pub const fn empty(file_id: FileId, byte: usize) -> Self {
        Self::new(file_id, byte, byte)
    }

    #[must_use]
    pub const fn join(self, other: Self) -> Self {
        Self {
            file_id: self.file_id,
            start_byte: self.start_byte,
            end_byte: other.end_byte,
            instance: self.instance,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceFile {
    id: FileId,
    path: PathBuf,
    text: String,
    bom_bytes: usize,
}

impl SourceFile {
    #[must_use]
    pub fn new(id: FileId, path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        Self {
            id,
            path: path.into(),
            text: text.into(),
            bom_bytes: 0,
        }
    }

    pub fn from_utf8(
        id: FileId,
        path: impl Into<PathBuf>,
        bytes: Vec<u8>,
    ) -> Result<Self, Vec<Diagnostic>> {
        let has_bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
        let text_bytes = if has_bom { &bytes[3..] } else { &bytes };
        let text = std::str::from_utf8(text_bytes).map_err(|error| {
            vec![Diagnostic::error(
                DiagnosticCode::InvalidUtf8,
                SourceRange::new(
                    id,
                    usize::from(has_bom) * 3 + error.valid_up_to(),
                    usize::from(has_bom) * 3 + error.valid_up_to() + 1,
                ),
                "source file is not valid UTF-8",
                "save the file as UTF-8",
            )]
        })?;
        Ok(Self {
            id,
            path: path.into(),
            text: text.to_owned(),
            bom_bytes: usize::from(has_bom) * 3,
        })
    }

    #[must_use]
    pub const fn id(&self) -> FileId {
        self.id
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn original_offset(&self, text_offset: usize) -> usize {
        self.bom_bytes + text_offset
    }

    #[must_use]
    /// One-based scalar coordinates, clamping interior byte offsets to the character start.
    pub fn line_column(&self, original_offset: usize) -> (usize, usize) {
        let offset = original_offset
            .saturating_sub(self.bom_bytes)
            .min(self.text.len());
        let offset = self.text.floor_char_boundary(offset);
        let prefix = &self.text[..offset];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
        let column = prefix[line_start..].chars().count() + 1;
        (line, column)
    }

    pub fn utf16_position(&self, original_offset: usize) -> Result<Utf16Position, &'static str> {
        let offset = original_offset
            .checked_sub(self.bom_bytes)
            .ok_or("position is inside the BOM")?;
        if !self.text.is_char_boundary(offset) {
            return Err("position is outside UTF-8 character boundaries");
        }
        let prefix = &self.text[..offset];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
        let start = prefix.rfind('\n').map_or(0, |index| index + 1);
        let mut content = &prefix[start..];
        if content.ends_with('\r') && self.text.as_bytes().get(offset) == Some(&b'\n') {
            content = &content[..content.len() - 1];
        }
        Ok(Utf16Position {
            line: line as u32,
            character: content.encode_utf16().count() as u32,
        })
    }

    pub fn original_byte_at_utf16(&self, position: Utf16Position) -> Result<usize, &'static str> {
        let mut start = 0;
        for (line, raw) in self.text.split('\n').enumerate() {
            if line != position.line as usize {
                start += raw.len() + 1;
                continue;
            }
            let content = if start + raw.len() < self.text.len() {
                raw.strip_suffix('\r').unwrap_or(raw)
            } else {
                raw
            };
            let mut units = 0;
            for (byte, character) in content.char_indices() {
                if units == position.character as usize {
                    return Ok(self.original_offset(start + byte));
                }
                units += character.len_utf16();
                if units > position.character as usize {
                    return Err("position is inside a UTF-16 surrogate pair");
                }
            }
            return if units == position.character as usize {
                Ok(self.original_offset(start + content.len()))
            } else {
                Err("UTF-16 column is outside its line")
            };
        }
        Err("UTF-16 line is outside the source")
    }
}

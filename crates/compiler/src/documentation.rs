//! Source documentation is separate from semantic/API compatibility metadata.
use crate::ast::{Declaration, FieldDecl, NativeMember, RecordDecl};
use crate::lexer::{Token, TokenKind, lex};
use crate::{Diagnostic, SourceFile, SourceRange, parse_source};

#[derive(Clone, Debug)]
pub struct SourceMemberDocumentation {
    pub name: String,
    pub is_private: bool,
    pub kind: &'static str,
    pub signature: String,
    pub documentation: String,
    pub range: SourceRange,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct SourceDeclarationDocumentation {
    /// Original source name. A package linker may give the same range another identity.
    pub name: String,
    pub kind: &'static str,
    pub signature: String,
    pub documentation: String,
    pub range: SourceRange,
    pub line: usize,
    pub members: Vec<SourceMemberDocumentation>,
}

struct Collector<'a> {
    source: &'a SourceFile,
    tokens: Vec<Token>,
}

impl Collector<'_> {
    fn slice(&self, start: u32, end: u32) -> &str {
        let bom = self.source.original_offset(0);
        &self.source.text()[start as usize - bom..end as usize - bom]
    }

    fn before(&self, start: u32) -> String {
        let index = self
            .tokens
            .partition_point(|token| token.range.end_byte <= start);
        let previous = index
            .checked_sub(1)
            .map_or(self.source.original_offset(0) as u32, |index| {
                self.tokens[index].range.end_byte
            });
        let gap = self.slice(previous, start);
        let mut lines = gap.split('\n').rev();
        // The last segment is the declaration's indentation, not a doc line.
        if lines.next().is_none_or(|line| !line.trim().is_empty()) {
            return String::new();
        }
        let mut docs = Vec::new();
        for line in lines {
            let text = line.trim_end_matches('\r').trim_start();
            let Some(doc) = text.strip_prefix("///") else {
                break;
            };
            // Four slashes remain an ordinary comment; a blank separates groups.
            if doc.starts_with('/') {
                break;
            }
            docs.push(doc.strip_prefix(' ').unwrap_or(doc));
        }
        docs.reverse();
        docs.join("\n")
    }

    fn declaration(
        &self,
        name: &str,
        kind: &'static str,
        range: SourceRange,
        header_end: u32,
        members: Vec<SourceMemberDocumentation>,
    ) -> SourceDeclarationDocumentation {
        SourceDeclarationDocumentation {
            name: name.to_owned(),
            kind,
            signature: self.slice(range.start_byte, header_end).trim().to_owned(),
            documentation: self.before(range.start_byte),
            range,
            line: self.source.line_column(range.start_byte as usize).0,
            members,
        }
    }

    fn fields(&self, fields: &[FieldDecl]) -> Vec<SourceMemberDocumentation> {
        fields
            .iter()
            .map(|field| {
                // FieldDecl starts at the name; class `const` is a preceding token.
                let index = self
                    .tokens
                    .partition_point(|token| token.range.start_byte < field.range.start_byte);
                let start = index
                    .checked_sub(1)
                    .filter(|previous| {
                        field.is_const
                            && matches!(
                                &self.tokens[*previous].kind,
                                TokenKind::Identifier(name) if name == "const"
                            )
                    })
                    .map_or(field.range.start_byte, |previous| {
                        self.tokens[previous].range.start_byte
                    });
                let range = SourceRange {
                    start_byte: start,
                    ..field.range
                };
                SourceMemberDocumentation {
                    name: field.name.clone(),
                    is_private: field.is_private,
                    kind: "field",
                    signature: self.slice(start, field.range.end_byte).trim().to_owned(),
                    documentation: self.before(start),
                    range,
                    line: self.source.line_column(start as usize).0,
                }
            })
            .collect()
    }

    fn header_end(&self, range: SourceRange) -> u32 {
        self.tokens
            .iter()
            .find(|token| {
                token.range.start_byte >= range.start_byte
                    && token.range.end_byte <= range.end_byte
                    && token.kind == TokenKind::LeftBrace
            })
            .expect("parsed aggregate has an opening brace")
            .range
            .start_byte
    }

    fn record(&self, record: &RecordDecl, kind: &'static str) -> SourceDeclarationDocumentation {
        self.declaration(
            &record.name,
            kind,
            record.range,
            self.header_end(record.range),
            self.fields(&record.fields),
        )
    }
}

/// Read adjacent `///` lines and original headers using the ordinary parser/lexer.
/// This includes private declarations; the package command filters with checked exports.
pub fn source_documentation(
    source: &SourceFile,
) -> Result<Vec<SourceDeclarationDocumentation>, Vec<Diagnostic>> {
    let module = parse_source(source)?;
    let collector = Collector {
        source,
        tokens: lex(source)?,
    };
    let mut docs = Vec::new();
    for declaration in &module.declarations {
        let entry = match declaration {
            Declaration::Function(function) => collector.declaration(
                &function.name,
                if function.name.contains('.') {
                    "method"
                } else {
                    "function"
                },
                function.range,
                function.body.range.start_byte,
                Vec::new(),
            ),
            Declaration::Record(record) => collector.record(record, "class"),
            Declaration::Error(error) => collector.declaration(
                &error.name,
                "error",
                error.range,
                collector.header_end(error.range),
                error
                    .payload
                    .as_ref()
                    .map(|payload| SourceMemberDocumentation {
                        is_private: false,
                        name: "payload".to_owned(),
                        kind: "field",
                        signature: collector
                            .slice(payload.range.start_byte, payload.range.end_byte)
                            .trim()
                            .to_owned(),
                        documentation: collector.before(payload.range.start_byte),
                        range: payload.range,
                        line: source.line_column(payload.range.start_byte as usize).0,
                    })
                    .into_iter()
                    .collect(),
            ),
            Declaration::Enum(value) => collector.declaration(
                &value.name,
                "enum",
                value.range,
                collector.header_end(value.range),
                value
                    .variants
                    .iter()
                    .map(|variant| SourceMemberDocumentation {
                        is_private: false,
                        name: variant.name.clone(),
                        kind: "variant",
                        signature: collector
                            .slice(variant.range.start_byte, variant.range.end_byte)
                            .trim()
                            .to_owned(),
                        documentation: collector.before(variant.range.start_byte),
                        range: variant.range,
                        line: source.line_column(variant.range.start_byte as usize).0,
                    })
                    .collect(),
            ),
            Declaration::TypeAlias(value) => collector.declaration(
                &value.name,
                "type-alias",
                value.range,
                value.range.end_byte,
                Vec::new(),
            ),
            Declaration::Constant(value) => collector.declaration(
                &value.name,
                "constant",
                value.range,
                value.range.end_byte,
                Vec::new(),
            ),
        };
        docs.push(entry);
    }
    for block in &module.native_blocks {
        for member in &block.members {
            let entry = match member {
                NativeMember::Opaque { name, range } => {
                    collector.declaration(name, "opaque", *range, range.end_byte, Vec::new())
                }
                NativeMember::Record(record) => collector.record(record, "native-record"),
                NativeMember::Function(function) => collector.declaration(
                    &function.name,
                    "native-function",
                    function.range,
                    function.range.end_byte,
                    Vec::new(),
                ),
            };
            docs.push(entry);
        }
    }
    docs.sort_by_key(|entry| entry.range.start_byte);
    Ok(docs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileId;

    #[test]
    fn docs_keep_bom_crlf_unicode_code_indent_and_original_positions() {
        let text = "\u{feff}/// 日本語🙂\r\n/// ```rinzu-test\r\n/// fn main() {\r\n///     let answer = 42;\r\n/// }\r\n/// ```\r\nfn answer<T>(value: T) -> T { return value; }\r\n";
        let source =
            SourceFile::from_utf8(FileId(17), "docs.rinzu", text.as_bytes().to_vec()).unwrap();
        let docs = source_documentation(&source).unwrap();
        assert_eq!(
            docs[0].documentation,
            "日本語🙂\n```rinzu-test\nfn main() {\n    let answer = 42;\n}\n```"
        );
        assert_eq!(docs[0].signature, "fn answer<T>(value: T) -> T");
        assert_eq!(docs[0].line, 7);
        assert_eq!(
            docs[0].range.start_byte as usize,
            text.find("fn answer").unwrap()
        );
    }

    #[test]
    fn literals_ordinary_comments_and_blank_gaps_do_not_create_docs() {
        let text = "let text = r\"/// not a doc\n/// still a literal\";\nfn plain() {}\n/// detached\n\nfn detached() {}\n//// ordinary\nfn ordinary() {}\n// older comment\n/// adjacent\nfn documented() {}";
        let docs = source_documentation(&SourceFile::new(FileId(0), "docs.rinzu", text)).unwrap();
        assert!(docs[..4].iter().all(|entry| entry.documentation.is_empty()));
        assert_eq!(docs[4].documentation, "adjacent");
    }

    #[test]
    fn class_fields_methods_enum_variants_aliases_and_native_headers_are_separate() {
        let text = "/// counter\nclass Counter {\n/// total\nvalue: Int\n/// read count\nfn read(self: Counter) -> Int { return self.value; }\n}\n/// state\nenum State {\n/// ready value\nReady(value: Int),\n/// no value\nEmpty\n}\n/// alias\ntype Counts<T> = List<T>;\nnative {\n/// opaque\nopaque Handle;\n/// layout\nrecord Pair {\n/// first\nx: CInt\n}\n/// native answer\nfn answer() -> CInt symbol \"answer\";\n}";
        let docs = source_documentation(&SourceFile::new(FileId(0), "docs.rinzu", text)).unwrap();
        let find = |name: &str| docs.iter().find(|entry| entry.name == name).unwrap();
        assert_eq!(find("Counter").documentation, "counter");
        assert_eq!(find("Counter").members[0].documentation, "total");
        assert_eq!(find("Counter.read").documentation, "read count");
        assert_eq!(find("Counter.read").kind, "method");
        assert!(!find("Counter.read").signature.contains("return"));
        assert_eq!(find("State").members[0].documentation, "ready value");
        assert_eq!(find("State").members[1].signature, "Empty");
        assert_eq!(find("Counts").kind, "type-alias");
        assert_eq!(find("Handle").documentation, "opaque");
        assert_eq!(find("Handle").signature, "opaque Handle;");
        assert_eq!(find("Pair").members[0].documentation, "first");
        assert_eq!(
            find("answer").signature,
            "fn answer() -> CInt symbol \"answer\";"
        );
    }

    #[test]
    fn const_field_docs_include_the_modifier_without_changing_parser_ranges() {
        let text = "class Values<T> {\n/// stored value\nconst value: T,\n/// mutable count\ncount: Int\n}\n/// upper bound\nlet LIMIT = 42;";
        let docs = source_documentation(&SourceFile::new(FileId(0), "docs.rinzu", text)).unwrap();
        assert_eq!(docs[0].members[0].signature, "const value: T");
        assert_eq!(docs[0].members[0].documentation, "stored value");
        assert_eq!(
            docs[0].members[0].range.start_byte as usize,
            text.find("const value").unwrap()
        );
        assert_eq!(docs[0].members[1].documentation, "mutable count");
        assert_eq!(docs[1].kind, "constant");
        assert_eq!(docs[1].documentation, "upper bound");
    }
}

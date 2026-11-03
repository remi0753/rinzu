use crate::{Diagnostic, DiagnosticCode, SourceFile, SourceRange};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub range: SourceRange,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> SourceFile {
        SourceFile::new(crate::FileId(1), "strings.rinzu", text)
    }

    #[test]
    fn formatted_escapes_are_text_and_preserve_unicode_and_nul() {
        let tokens = lex(&source(r#"f"日本語 \{name\} \u{7B}\u{7D}\u{0}\n\"\\""#)).unwrap();
        let TokenKind::FormattedString(parts) = &tokens[0].kind else {
            panic!("formatted String");
        };
        let [FormattedPart::Text { value, .. }] = parts.as_slice() else {
            panic!("escapes must not become interpolation expressions");
        };
        assert_eq!(value, "日本語 {name} {}\0\n\"\\");
    }

    #[test]
    fn expression_braces_ignore_strings_bytes_comments_and_nested_literals() {
        let text = r#"f"前{lookup("}", b"{") /* } */}中{{"a": 1}}後{f"内{name}"}""#;
        let tokens = lex(&source(text)).unwrap();
        let TokenKind::FormattedString(parts) = &tokens[0].kind else {
            panic!("formatted String");
        };
        assert_eq!(parts.len(), 6);
        let FormattedPart::Expression { tokens, range } = &parts[1] else {
            panic!("first expression");
        };
        assert_eq!(
            &text[range.start_byte as usize..range.end_byte as usize],
            r#"{lookup("}", b"{") /* } */}"#
        );
        assert!(
            tokens
                .iter()
                .any(|token| token.kind == TokenKind::String("}".into()))
        );
        assert!(
            tokens
                .iter()
                .any(|token| token.kind == TokenKind::Bytes(b"{".to_vec()))
        );
        let FormattedPart::Expression { tokens, .. } = &parts[3] else {
            panic!("map expression");
        };
        assert_eq!(tokens[0].kind, TokenKind::LeftBrace);
        assert_eq!(tokens[tokens.len() - 2].kind, TokenKind::RightBrace);
        let FormattedPart::Expression { tokens, .. } = &parts[5] else {
            panic!("nested literal");
        };
        assert!(matches!(tokens[0].kind, TokenKind::FormattedString(_)));
    }

    #[test]
    fn malformed_braces_and_multiline_literals_are_diagnosed_at_original_spans() {
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes.extend_from_slice("f\"前}\"".as_bytes());
        let file = SourceFile::from_utf8(crate::FileId(7), "bom.rinzu", bytes).unwrap();
        let errors = lex(&file).unwrap_err();
        assert_eq!(errors[0].code, DiagnosticCode::UnexpectedToken);
        assert_eq!(errors[0].primary, SourceRange::new(file.id(), 8, 9));
        assert!(errors[0].suggestions[0].message.contains("\\}"));
        for text in [
            "f\"{value",
            "f\"text\nnext\"",
            "f\"{value\n}\"",
            "f\"{1 /*\n*/}\"",
        ] {
            let errors = lex(&source(text)).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.code == DiagnosticCode::UnterminatedLiteral),
                "{text:?}: {errors:?}"
            );
        }
    }

    #[test]
    fn ordinary_strings_and_bytes_keep_their_existing_rules() {
        let tokens = lex(&source(r#""{name}" b"{}" f "{}""#)).unwrap();
        assert_eq!(tokens[0].kind, TokenKind::String("{name}".into()));
        assert_eq!(tokens[1].kind, TokenKind::Bytes(b"{}".to_vec()));
        assert_eq!(tokens[2].kind, TokenKind::Identifier("f".into()));
        assert_eq!(tokens[3].kind, TokenKind::String("{}".into()));
        assert_eq!(
            lex(&source(r#""\{""#)).unwrap_err()[0].code,
            DiagnosticCode::InvalidEscape
        );
    }

    #[test]
    fn raw_content_preserves_nul_crlf_tabs_and_backslashes_for_every_delimiter() {
        let content = "日本語\0\r\n\t\\u{0}{name}";
        for count in 0..=16 {
            let hashes = "#".repeat(count);
            let text = format!("r{hashes}\"{content}\"{hashes}");
            let tokens = lex(&source(&text)).unwrap();
            assert_eq!(tokens[0].kind, TokenKind::String(content.into()));
            assert_eq!(tokens[0].range.end_byte as usize, text.len());
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FormattedPart {
    Text {
        value: String,
        range: SourceRange,
    },
    Expression {
        tokens: Vec<Token>,
        range: SourceRange,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TokenKind {
    Identifier(String),
    IntMagnitude(u64),
    DecimalInteger(String),
    Float { value: f64, spelling: String },
    String(String),
    FormattedString(Vec<FormattedPart>),
    Bytes(Vec<u8>),
    Import,
    Export,
    Native,
    Type,
    Supplant,
    Catch,
    Fallback,
    Fn,
    Async,
    Record,
    Enum,
    Let,
    Var,
    If,
    Else,
    Switch,
    Case,
    While,
    With,
    Break,
    Continue,
    Return,
    Throw,
    Await,
    Spawn,
    Try,
    TaskScope,
    Finally,
    True,
    False,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Colon,
    Question,
    Semicolon,
    Dot,
    Ellipsis,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    EqualEqual,
    Bang,
    BangEqual,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    AndAnd,
    OrOr,
    Arrow,
    Eof,
}

impl TokenKind {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Identifier(_) => "identifier",
            Self::IntMagnitude(_) | Self::DecimalInteger(_) => "integer literal",
            Self::Float { .. } => "floating literal",
            Self::String(_) => "String literal",
            Self::FormattedString(_) => "formatted String literal",
            Self::Bytes(_) => "Bytes literal",
            Self::Import => "`import`",
            Self::Native => "`native`",
            Self::Type => "`type`",
            Self::Supplant => "`supplant`",
            Self::Catch => "`catch`",
            Self::Fallback => "`fallback`",
            Self::Export => "`export`",
            Self::Fn => "`fn`",
            Self::Async => "`async`",
            Self::Record => "`record`",
            Self::Enum => "`enum`",
            Self::Let => "`let`",
            Self::Var => "`var`",
            Self::If => "`if`",
            Self::Else => "`else`",
            Self::Switch => "`switch`",
            Self::Case => "`case`",
            Self::While => "`while`",
            Self::With => "`with`",
            Self::Break => "`break`",
            Self::Continue => "`continue`",
            Self::Return => "`return`",
            Self::Throw => "`throw`",
            Self::Await => "`await`",
            Self::Spawn => "`spawn`",
            Self::Try => "`try`",
            Self::TaskScope => "`taskScope`",
            Self::Finally => "`finally`",
            Self::True => "`true`",
            Self::False => "`false`",
            Self::LeftParen => "`(`",
            Self::RightParen => "`)`",
            Self::LeftBrace => "`{`",
            Self::RightBrace => "`}`",
            Self::LeftBracket => "`[`",
            Self::RightBracket => "`]`",
            Self::Comma => "`,`",
            Self::Colon => "`:`",
            Self::Question => "`?`",
            Self::Semicolon => "`;`",
            Self::Dot => "`.`",
            Self::Ellipsis => "`...`",
            Self::Less => "`<`",
            Self::LessEqual => "`<=`",
            Self::Greater => "`>`",
            Self::GreaterEqual => "`>=`",
            Self::Equal => "`=`",
            Self::EqualEqual => "`==`",
            Self::Bang => "`!`",
            Self::BangEqual => "`!=`",
            Self::Plus => "`+`",
            Self::Minus => "`-`",
            Self::Star => "`*`",
            Self::Slash => "`/`",
            Self::Percent => "`%`",
            Self::AndAnd => "`&&`",
            Self::OrOr => "`||`",
            Self::Arrow => "`->`",
            Self::Eof => "end of file",
        }
    }
}

pub(crate) fn lex(file: &SourceFile) -> Result<Vec<Token>, Vec<Diagnostic>> {
    Lexer {
        file,
        bytes: file.text().as_bytes(),
        offset: 0,
        tokens: Vec::new(),
        diagnostics: Vec::new(),
    }
    .run()
}

struct Lexer<'a> {
    file: &'a SourceFile,
    bytes: &'a [u8],
    offset: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl Lexer<'_> {
    fn run(mut self) -> Result<Vec<Token>, Vec<Diagnostic>> {
        while self.offset < self.bytes.len() {
            self.token();
        }
        let eof = self.file.original_offset(self.offset);
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            range: SourceRange::empty(self.file.id(), eof),
        });
        if self.diagnostics.is_empty() {
            Ok(self.tokens)
        } else {
            Err(self.diagnostics)
        }
    }

    fn token(&mut self) {
        match self.bytes[self.offset] {
            b' ' | b'\t' | b'\r' | b'\n' => self.offset += 1,
            b'/' if self.peek(1) == Some(b'/') => self.line_comment(),
            b'/' if self.peek(1) == Some(b'*') => self.block_comment(),
            b'b' if self.peek(1) == Some(b'"') => self.bytes_literal(),
            b'f' if self.peek(1) == Some(b'"') => self.formatted_string(),
            b'r' if matches!(self.peek(1), Some(b'"' | b'#')) => self.raw_string(),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.word(),
            b'0'..=b'9' => self.number(),
            b'"' => self.string_literal(),
            byte => self.symbol(byte),
        }
    }

    fn peek(&self, distance: usize) -> Option<u8> {
        self.bytes.get(self.offset + distance).copied()
    }

    fn range(&self, start: usize, end: usize) -> SourceRange {
        SourceRange::new(
            self.file.id(),
            self.file.original_offset(start),
            self.file.original_offset(end),
        )
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            range: self.range(start, self.offset),
        });
    }

    fn line_comment(&mut self) {
        self.offset += 2;
        while self.offset < self.bytes.len() && self.bytes[self.offset] != b'\n' {
            self.offset += 1;
        }
    }

    fn block_comment(&mut self) {
        let start = self.offset;
        self.offset += 2;
        while self.offset + 1 < self.bytes.len() {
            if self.bytes[self.offset] == b'*' && self.bytes[self.offset + 1] == b'/' {
                self.offset += 2;
                return;
            }
            self.offset += 1;
        }
        self.offset = self.bytes.len();
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnterminatedComment,
            self.range(start, self.offset),
            "unterminated block comment",
            "add `*/` before the end of the file",
        ));
    }

    fn word(&mut self) {
        let start = self.offset;
        self.offset += 1;
        while matches!(
            self.peek(0),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        ) {
            self.offset += 1;
        }
        let word = &self.file.text()[start..self.offset];
        let kind = match word {
            "import" => TokenKind::Import,
            "native" => TokenKind::Native,
            "type" => TokenKind::Type,
            "supplant" => TokenKind::Supplant,
            "catch" => TokenKind::Catch,
            "fallback" => TokenKind::Fallback,
            "export" => TokenKind::Export,
            "fn" => TokenKind::Fn,
            "async" => TokenKind::Async,
            "record" => TokenKind::Record,
            "enum" => TokenKind::Enum,
            "let" => TokenKind::Let,
            "var" => TokenKind::Var,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "switch" => TokenKind::Switch,
            "case" => TokenKind::Case,
            "while" => TokenKind::While,
            "with" => TokenKind::With,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "return" => TokenKind::Return,
            "throw" => TokenKind::Throw,
            "await" => TokenKind::Await,
            "spawn" => TokenKind::Spawn,
            "try" => TokenKind::Try,
            "taskScope" => TokenKind::TaskScope,
            "finally" => TokenKind::Finally,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            _ => TokenKind::Identifier(word.to_owned()),
        };
        self.push(kind, start);
    }

    fn number(&mut self) {
        let start = self.offset;
        if self.peek(0) == Some(b'0') && matches!(self.peek(1), Some(b'x' | b'X' | b'b' | b'B')) {
            let radix = if matches!(self.peek(1), Some(b'x' | b'X')) {
                16
            } else {
                2
            };
            self.offset += 2;
            let digit_start = self.offset;
            while self
                .peek(0)
                .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                self.offset += 1;
            }
            let digits = &self.file.text()[digit_start..self.offset];
            let valid = !digits.is_empty()
                && digits.bytes().enumerate().all(|(i, b)| {
                    if b == b'_' {
                        i > 0
                            && digits.as_bytes()[i - 1] != b'_'
                            && digits.as_bytes().get(i + 1).is_some_and(|b| *b != b'_')
                    } else {
                        (b as char).is_digit(radix)
                    }
                });
            if let Some(value) = valid
                .then(|| u64::from_str_radix(&digits.replace('_', ""), radix).ok())
                .flatten()
            {
                self.push(TokenKind::IntMagnitude(value), start);
            } else {
                self.diagnostics.push(Diagnostic::error(DiagnosticCode::InvalidToken,self.range(start,self.offset),
                    "invalid or oversized radix integer literal","use hexadecimal/binary digits with separators between digits and a u64 magnitude"));
            }
            return;
        }
        while matches!(self.peek(0), Some(b'0'..=b'9' | b'_')) {
            self.offset += 1;
        }
        let mut is_float = false;
        if self.peek(0) == Some(b'.') && matches!(self.peek(1), Some(b'0'..=b'9')) {
            is_float = true;
            self.offset += 1;
            while matches!(self.peek(0), Some(b'0'..=b'9' | b'_')) {
                self.offset += 1;
            }
        }
        if matches!(self.peek(0), Some(b'e' | b'E')) {
            is_float = true;
            self.offset += 1;
            if matches!(self.peek(0), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            while matches!(self.peek(0), Some(b'0'..=b'9' | b'_')) {
                self.offset += 1;
            }
        }
        let spelling = &self.file.text()[start..self.offset];
        if spelling.bytes().enumerate().any(|(index, byte)| {
            byte == b'_'
                && (index == 0
                    || !spelling.as_bytes()[index - 1].is_ascii_digit()
                    || !spelling
                        .as_bytes()
                        .get(index + 1)
                        .is_some_and(u8::is_ascii_digit))
        }) {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidToken,
                self.range(start, self.offset),
                "numeric separators must occur between digits",
                "place each underscore between two decimal digits",
            ));
            return;
        }
        let raw = spelling.replace('_', "");
        if is_float {
            match raw.parse() {
                Ok(value) => self.push(
                    TokenKind::Float {
                        value,
                        spelling: raw,
                    },
                    start,
                ),
                Err(_) => self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidToken,
                    self.range(start, self.offset),
                    "invalid floating literal",
                    "use a decimal floating literal such as `1.0` or `1e3`",
                )),
            }
        } else {
            match raw.parse::<u64>() {
                Ok(value) => {
                    self.push(TokenKind::IntMagnitude(value), start);
                }
                Err(_) => self.push(TokenKind::DecimalInteger(raw), start),
            }
        }
    }

    fn formatted_string(&mut self) {
        let start = self.offset;
        self.offset += 2;
        let mut text_start = self.offset;
        let mut value = String::new();
        let mut parts = Vec::new();
        while self.offset < self.bytes.len() {
            match self.bytes[self.offset] {
                b'"' => {
                    if !value.is_empty() {
                        parts.push(FormattedPart::Text {
                            value,
                            range: self.range(text_start, self.offset),
                        });
                    }
                    self.offset += 1;
                    self.push(TokenKind::FormattedString(parts), start);
                    return;
                }
                b'\n' | b'\r' => break,
                b'{' => {
                    if !value.is_empty() {
                        parts.push(FormattedPart::Text {
                            value: std::mem::take(&mut value),
                            range: self.range(text_start, self.offset),
                        });
                    }
                    let Some(part) = self.formatted_expression() else {
                        return;
                    };
                    parts.push(part);
                    text_start = self.offset;
                }
                b'}' => {
                    let brace = self.offset;
                    self.offset += 1;
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::UnexpectedToken,
                        self.range(brace, self.offset),
                        "unmatched `}` in formatted String",
                        "write `\\}` for a literal closing brace",
                    ));
                }
                b'\\' => {
                    self.offset += 1;
                    if matches!(self.peek(0), Some(b'{' | b'}')) {
                        value.push(self.bytes[self.offset] as char);
                        self.offset += 1;
                    } else if !self.string_escape(&mut value, start) {
                        return;
                    }
                }
                _ => {
                    let character = self.file.text()[self.offset..]
                        .chars()
                        .next()
                        .expect("offset is valid");
                    value.push(character);
                    self.offset += character.len_utf8();
                }
            }
        }
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnterminatedLiteral,
            self.range(start, self.offset),
            "unterminated formatted String literal",
            "add a closing double quote on the same line",
        ));
    }

    fn formatted_expression(&mut self) -> Option<FormattedPart> {
        let start = self.offset;
        self.offset += 1;
        let first_token = self.tokens.len();
        let mut braces = 0_usize;
        while self.offset < self.bytes.len() {
            match self.bytes[self.offset] {
                b'}' if braces == 0 => {
                    let mut tokens = self.tokens.split_off(first_token);
                    tokens.push(Token {
                        kind: TokenKind::Eof,
                        range: self.range(self.offset, self.offset),
                    });
                    self.offset += 1;
                    return Some(FormattedPart::Expression {
                        tokens,
                        range: self.range(start, self.offset),
                    });
                }
                b'{' => braces += 1,
                b'}' => braces -= 1,
                b'\n' | b'\r' => break,
                _ => {}
            }
            let token_start = self.offset;
            self.token();
            if self.bytes[token_start..self.offset]
                .iter()
                .any(|byte| matches!(byte, b'\n' | b'\r'))
            {
                break;
            }
        }
        self.tokens.truncate(first_token);
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnterminatedLiteral,
            self.range(start, start + 1),
            "unterminated formatted String expression",
            "add the matching `}` and keep the literal on one line; write `\\{` for a literal brace",
        ));
        None
    }

    fn raw_string(&mut self) {
        let start = self.offset;
        self.offset += 1;
        let hashes_start = self.offset;
        while self.peek(0) == Some(b'#') {
            self.offset += 1;
        }
        let hashes = self.offset - hashes_start;
        if hashes > 16 || self.peek(0) != Some(b'"') {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidToken,
                self.range(start, self.offset),
                "invalid raw String delimiter",
                "use r, zero to sixteen hashes, then a double quote",
            ));
            return;
        }
        self.offset += 1;
        let content_start = self.offset;
        while self.offset < self.bytes.len() {
            if self.bytes[self.offset] == b'"'
                && self
                    .bytes
                    .get(self.offset + 1..self.offset + 1 + hashes)
                    .is_some_and(|suffix| suffix.iter().all(|b| *b == b'#'))
            {
                let value = self.file.text()[content_start..self.offset].to_owned();
                self.offset += 1 + hashes;
                self.push(TokenKind::String(value), start);
                return;
            }
            self.offset += 1;
        }
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnterminatedLiteral,
            self.range(start, self.offset),
            "unterminated raw String literal",
            "close the double quote with the same number of hashes",
        ));
    }

    fn string_literal(&mut self) {
        let start = self.offset;
        self.offset += 1;
        let mut value = String::new();
        while self.offset < self.bytes.len() {
            match self.bytes[self.offset] {
                b'"' => {
                    self.offset += 1;
                    self.push(TokenKind::String(value), start);
                    return;
                }
                b'\n' | b'\r' => break,
                b'\\' => {
                    self.offset += 1;
                    if !self.string_escape(&mut value, start) {
                        return;
                    }
                }
                _ => {
                    let character = self.file.text()[self.offset..]
                        .chars()
                        .next()
                        .expect("offset is valid");
                    value.push(character);
                    self.offset += character.len_utf8();
                }
            }
        }
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnterminatedLiteral,
            self.range(start, self.offset),
            "unterminated String literal",
            "add a closing double quote on the same line",
        ));
    }

    fn string_escape(&mut self, output: &mut String, start: usize) -> bool {
        let Some(byte) = self.peek(0) else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::UnterminatedLiteral,
                self.range(start, self.offset),
                "unterminated literal after escape",
                "complete the escape and add a closing double quote on the same line",
            ));
            return false;
        };
        match byte {
            b'\\' => output.push('\\'),
            b'"' => output.push('"'),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'u' if self.peek(1) == Some(b'{') => {
                self.offset += 2;
                let hex_start = self.offset;
                while matches!(self.peek(0), Some(b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F'))
                    && self.offset - hex_start < 6
                {
                    self.offset += 1;
                }
                if self.offset == hex_start || self.peek(0) != Some(b'}') {
                    self.invalid_escape(
                        start,
                        "use `\\u{1F600}` with one to six hexadecimal digits",
                    );
                    return false;
                }
                let raw = &self.file.text()[hex_start..self.offset];
                self.offset += 1;
                let scalar = u32::from_str_radix(raw, 16).ok().and_then(char::from_u32);
                let Some(character) = scalar else {
                    self.invalid_escape(start, "use a valid Unicode scalar value");
                    return false;
                };
                output.push(character);
                return true;
            }
            _ => {
                self.invalid_escape(
                    start,
                    "use one of `\\\\`, `\\\"`, `\\n`, `\\r`, `\\t`, or `\\u{...}`",
                );
                return false;
            }
        }
        self.offset += 1;
        true
    }

    fn bytes_literal(&mut self) {
        let start = self.offset;
        self.offset += 2;
        let mut value = Vec::new();
        while self.offset < self.bytes.len() {
            match self.bytes[self.offset] {
                b'"' => {
                    self.offset += 1;
                    self.push(TokenKind::Bytes(value), start);
                    return;
                }
                b'\n' | b'\r' => break,
                b'\\' => {
                    self.offset += 1;
                    let Some(escape) = self.peek(0) else { break };
                    match escape {
                        b'\\' | b'"' => value.push(escape),
                        b'n' => value.push(b'\n'),
                        b'r' => value.push(b'\r'),
                        b't' => value.push(b'\t'),
                        b'x' => {
                            let Some(hex) = self.bytes.get(self.offset + 1..self.offset + 3) else {
                                self.invalid_escape(
                                    start,
                                    "use `\\xNN` with two hexadecimal digits",
                                );
                                return;
                            };
                            let Ok(raw) = std::str::from_utf8(hex) else {
                                self.invalid_escape(
                                    start,
                                    "use `\\xNN` with two hexadecimal digits",
                                );
                                return;
                            };
                            let Ok(byte) = u8::from_str_radix(raw, 16) else {
                                self.invalid_escape(
                                    start,
                                    "use `\\xNN` with two hexadecimal digits",
                                );
                                return;
                            };
                            value.push(byte);
                            self.offset += 3;
                            continue;
                        }
                        _ => {
                            self.invalid_escape(
                                start,
                                "use an ASCII byte or a supported Bytes escape",
                            );
                            return;
                        }
                    }
                    self.offset += 1;
                }
                b' '..=b'~' => {
                    value.push(self.bytes[self.offset]);
                    self.offset += 1;
                }
                _ => {
                    self.invalid_escape(
                        start,
                        "Bytes literals contain printable ASCII or `\\xNN` escapes",
                    );
                    return;
                }
            }
        }
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnterminatedLiteral,
            self.range(start, self.offset),
            "unterminated Bytes literal",
            "add a closing double quote on the same line",
        ));
    }

    fn invalid_escape(&mut self, start: usize, suggestion: &str) {
        let end = self
            .file
            .text()
            .ceil_char_boundary(self.offset.saturating_add(1).min(self.bytes.len()));
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::InvalidEscape,
            self.range(start, end),
            "invalid literal escape",
            suggestion,
        ));
        while self.offset < self.bytes.len()
            && !matches!(self.bytes[self.offset], b'"' | b'\n' | b'\r')
        {
            self.offset += 1;
        }
        if self.peek(0) == Some(b'"') {
            self.offset += 1;
        }
    }

    fn symbol(&mut self, byte: u8) {
        let start = self.offset;
        self.offset += 1;
        let kind = match (byte, self.peek(0)) {
            (b'(', _) => TokenKind::LeftParen,
            (b')', _) => TokenKind::RightParen,
            (b'{', _) => TokenKind::LeftBrace,
            (b'}', _) => TokenKind::RightBrace,
            (b'[', _) => TokenKind::LeftBracket,
            (b']', _) => TokenKind::RightBracket,
            (b',', _) => TokenKind::Comma,
            (b':', _) => TokenKind::Colon,
            (b'?', _) => TokenKind::Question,
            (b';', _) => TokenKind::Semicolon,
            (b'.', Some(b'.')) if self.peek(1) == Some(b'.') => {
                self.offset += 2;
                TokenKind::Ellipsis
            }
            (b'.', _) => TokenKind::Dot,
            (b'+', _) => TokenKind::Plus,
            (b'*', _) => TokenKind::Star,
            (b'/', _) => TokenKind::Slash,
            (b'%', _) => TokenKind::Percent,
            (b'-', Some(b'>')) => {
                self.offset += 1;
                TokenKind::Arrow
            }
            (b'-', _) => TokenKind::Minus,
            (b'<', Some(b'=')) => {
                self.offset += 1;
                TokenKind::LessEqual
            }
            (b'<', _) => TokenKind::Less,
            (b'>', Some(b'=')) => {
                self.offset += 1;
                TokenKind::GreaterEqual
            }
            (b'>', _) => TokenKind::Greater,
            (b'=', Some(b'=')) => {
                self.offset += 1;
                TokenKind::EqualEqual
            }
            (b'=', _) => TokenKind::Equal,
            (b'!', Some(b'=')) => {
                self.offset += 1;
                TokenKind::BangEqual
            }
            (b'!', _) => TokenKind::Bang,
            (b'&', Some(b'&')) => {
                self.offset += 1;
                TokenKind::AndAnd
            }
            (b'|', Some(b'|')) => {
                self.offset += 1;
                TokenKind::OrOr
            }
            _ => {
                let character = self.file.text()[start..]
                    .chars()
                    .next()
                    .expect("token starts at a character boundary");
                self.offset = start + character.len_utf8();
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidToken,
                    self.range(start, self.offset),
                    format!("invalid token character {character:?}"),
                    "remove the character or replace it with a language 0.5 token",
                ));
                return;
            }
        };
        self.push(kind, start);
    }
}

//! Syntax-aware, token-preserving formatting; comments and literal spellings are retained.
mod layout;
#[cfg(test)]
mod tests;

use crate::ast::{
    Declaration, Expression, ExpressionKind, FunctionDecl, NativeMember, TypeRef, TypeRefKind,
};
use crate::ast_walk::{self, Visitor};
use crate::lexer::{Token, TokenKind, lex};
use crate::{Diagnostic, SourceFile, SourceRange, parse_source};
use layout::Doc;

/// Maximum code-point width, except indivisible tokens and preserved comments/literals.
const MAX_WIDTH: usize = 120;
const INDENT: usize = 4;

pub fn format_source(file: &SourceFile) -> Result<String, Vec<Diagnostic>> {
    let mut syntax = parse_source(file)?;
    let tokens = lex(file)?;
    let mut hints = Hints::new(&tokens);
    hints.declaration_spacing(&syntax);
    for import in &syntax.imports {
        if let Some(file) = &import.file {
            for byte in [file.path_range.start_byte, file.alias_range.start_byte] {
                if let Some(index) = hints.at_byte(byte) {
                    hints.break_before[index] = true;
                }
            }
            if let Some(alias) = hints.at_byte(file.alias_range.start_byte) {
                hints.break_before[alias - 1] = true; // contextual `as`
            }
        }
    }
    for declaration in &syntax.declarations {
        match declaration {
            Declaration::Interface(interface) => {
                hints.parameters(&interface.type_parameters);
                for method in &interface.methods {
                    hints.member_end(method.range);
                }
            }
            Declaration::Record(record) => {
                hints.parameters(&record.type_parameters);
                for field in &record.fields {
                    hints.member_end(field.range);
                }
            }
            Declaration::Enum(enumeration) => {
                hints.parameters(&enumeration.type_parameters);
                for variant in &enumeration.variants {
                    hints.member_end(variant.range);
                }
            }
            Declaration::Error(error) => {
                hints.parameters(&error.type_parameters);
                if let Some(payload) = &error.payload {
                    hints.member_end(payload.range);
                }
            }
            Declaration::TypeAlias(alias) => hints.parameters(&alias.type_parameters),
            Declaration::Function(_) | Declaration::Constant(_) => {}
        }
    }
    ast_walk::module(&mut syntax, &mut hints);
    for block in &mut syntax.native_blocks {
        for member in &mut block.members {
            match member {
                NativeMember::Record(record) => {
                    hints.parameters(&record.type_parameters);
                    for field in &mut record.fields {
                        hints.member_end(field.range);
                        ast_walk::ty(&mut field.ty, &mut hints);
                    }
                }
                NativeMember::Function(function) => {
                    hints.binding(function.binding_range);
                    for parameter in &mut function.parameters {
                        ast_walk::ty(&mut parameter.ty, &mut hints);
                    }
                    ast_walk::ty(&mut function.result, &mut hints);
                }
                NativeMember::Opaque { .. } => {}
            }
        }
    }
    let mut builder = Builder {
        file,
        tokens: &tokens,
        hints: &hints,
        cursor: 0,
    };
    let document = builder.sequence(None, false);
    Ok(layout::render(&document, MAX_WIDTH))
}

struct Hints<'a> {
    tokens: &'a [Token],
    generic: Vec<bool>,
    break_after: Vec<bool>,
    binary: Vec<bool>,
    binding: Vec<bool>,
    break_before: Vec<bool>,
    blank_before: Vec<bool>,
}

impl<'a> Hints<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            generic: vec![false; tokens.len()],
            break_after: vec![false; tokens.len()],
            binary: vec![false; tokens.len()],
            binding: vec![false; tokens.len()],
            break_before: vec![false; tokens.len()],
            blank_before: vec![false; tokens.len()],
        }
    }

    fn at_byte(&self, byte: u32) -> Option<usize> {
        let index = self
            .tokens
            .partition_point(|token| token.range.start_byte < byte);
        self.tokens
            .get(index)
            .filter(|token| token.range.start_byte == byte)
            .map(|_| index)
    }

    fn declaration_spacing(&mut self, module: &crate::ast::Module) {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Group {
            Import,
            Export,
            Declaration,
        }

        let mut items: Vec<_> = module
            .imports
            .iter()
            .map(|item| (item.range, Group::Import))
            .chain(
                module
                    .exports
                    .iter()
                    .filter(|item| !item.implicit)
                    .map(|item| (item.range, Group::Export)),
            )
            .chain(
                module
                    .declarations
                    .iter()
                    .map(|item| (item.range(), Group::Declaration)),
            )
            .chain(
                module
                    .native_blocks
                    .iter()
                    .map(|item| (item.range, Group::Declaration)),
            )
            .collect();
        items.sort_by_key(|(range, _)| (range.start_byte, std::cmp::Reverse(range.end_byte)));
        let mut previous: Option<(SourceRange, Group)> = None;
        for (range, group) in items {
            // Class methods are flattened into module declarations, but belong
            // to the containing class's member sequence rather than this one.
            if previous.is_some_and(|(outer, _)| range.start_byte < outer.end_byte) {
                continue;
            }
            if previous.is_some_and(|(_, prior)| group != prior || group == Group::Declaration) {
                self.blank_line_before(range);
            }
            previous = Some((range, group));
        }

        for declaration in &module.declarations {
            let Declaration::Record(record) = declaration else {
                continue;
            };
            if !record.mutable {
                continue;
            }
            let mut members: Vec<_> = record
                .fields
                .iter()
                .map(|field| (field.range, false))
                .collect();
            members.extend(module.declarations.iter().filter_map(|item| match item {
                Declaration::Function(function)
                    if function.range.start_byte > record.range.start_byte
                        && function.range.end_byte < record.range.end_byte =>
                {
                    Some((function.range, true))
                }
                _ => None,
            }));
            members.sort_by_key(|(range, _)| range.start_byte);
            for pair in members.windows(2) {
                if pair[0].1 || pair[1].1 {
                    self.blank_line_before(pair[1].0);
                }
            }
        }

        for block in &module.native_blocks {
            for member in block.members.iter().skip(1) {
                let range = match member {
                    NativeMember::Opaque { range, .. } => *range,
                    NativeMember::Record(record) => record.range,
                    NativeMember::Function(function) => function.range,
                };
                self.blank_line_before(range);
            }
        }
    }

    fn blank_line_before(&mut self, range: SourceRange) {
        if let Some(index) = self.at_byte(range.start_byte) {
            self.blank_before[index] = true;
        }
    }

    fn parameters(&mut self, parameters: &[crate::ast::TypeParameter]) {
        let Some(first) = parameters
            .first()
            .and_then(|parameter| self.at_byte(parameter.range.start_byte))
        else {
            return;
        };
        let Some(open) = first
            .checked_sub(1)
            .filter(|&index| matches!(self.tokens[index].kind, TokenKind::Less))
        else {
            return;
        };
        let last = parameters.last().unwrap();
        let Some(close) = self
            .tokens
            .iter()
            .enumerate()
            .skip(first)
            .find(|(_, token)| {
                token.range.start_byte >= last.range.end_byte
                    && matches!(token.kind, TokenKind::Greater)
            })
            .map(|(index, _)| index)
        else {
            return;
        };
        self.generic[open] = true;
        self.generic[close] = true;
    }

    fn arguments(&mut self, arguments: &[TypeRef]) {
        let Some(first) = arguments
            .first()
            .and_then(|argument| self.at_byte(argument.range.start_byte))
        else {
            return;
        };
        let Some(open) = first
            .checked_sub(1)
            .filter(|&index| matches!(self.tokens[index].kind, TokenKind::Less))
        else {
            return;
        };
        let last = arguments.last().unwrap();
        let mut close = self
            .tokens
            .partition_point(|token| token.range.start_byte < last.range.end_byte);
        if self
            .tokens
            .get(close)
            .is_some_and(|token| matches!(token.kind, TokenKind::Comma))
        {
            close += 1;
        }
        if !self
            .tokens
            .get(close)
            .is_some_and(|token| matches!(token.kind, TokenKind::Greater))
        {
            return;
        }
        self.generic[open] = true;
        self.generic[close] = true;
    }

    fn member_end(&mut self, range: SourceRange) {
        let next = self
            .tokens
            .partition_point(|token| token.range.start_byte < range.end_byte);
        let Some(last) = next.checked_sub(1) else {
            return;
        };
        if self.tokens[last].range.end_byte != range.end_byte {
            return;
        }
        let index = if self
            .tokens
            .get(next)
            .is_some_and(|token| matches!(token.kind, TokenKind::Comma))
        {
            next
        } else {
            last
        };
        self.break_after[index] = true;
    }

    fn binding(&mut self, range: SourceRange) {
        if let Some(index) = self.at_byte(range.start_byte).filter(|&index| index > 0) {
            self.binding[index - 1] = true;
            self.binding[index] = true;
        }
    }
}

impl Visitor for Hints<'_> {
    fn enter_function(&mut self, function: &FunctionDecl) {
        self.parameters(&function.type_parameters[function.owner_type_parameter_count..]);
    }

    fn ty(&mut self, ty: &mut TypeRef) {
        if !matches!(
            ty.kind,
            TypeRefKind::Generic { .. }
                | TypeRefKind::List(_)
                | TypeRefKind::Map(_, _)
                | TypeRefKind::Future(_)
        ) {
            return;
        }
        // Interpolation expressions live inside one preserved literal token, not this token stream.
        let Some(start) = self.at_byte(ty.range.start_byte) else {
            return;
        };
        let end = self
            .tokens
            .partition_point(|token| token.range.end_byte <= ty.range.end_byte);
        let Some(close) = end
            .checked_sub(1)
            .filter(|&index| matches!(self.tokens[index].kind, TokenKind::Greater))
        else {
            return;
        };
        let Some(open) =
            (start..close).find(|&index| matches!(self.tokens[index].kind, TokenKind::Less))
        else {
            return;
        };
        self.generic[open] = true;
        self.generic[close] = true;
    }

    fn expression(&mut self, expression: &mut Expression) {
        match &expression.kind {
            ExpressionKind::Bind { type_arguments, .. } => self.arguments(type_arguments),
            ExpressionKind::GenericCall {
                type_arguments,
                member_type_arguments,
                ..
            }
            | ExpressionKind::GenericReference {
                type_arguments,
                member_type_arguments,
                ..
            } => {
                self.arguments(type_arguments);
                self.arguments(member_type_arguments);
            }
            ExpressionKind::Binary { left, right, .. } => {
                let index = self
                    .tokens
                    .partition_point(|token| token.range.start_byte < left.range.end_byte);
                if self
                    .tokens
                    .get(index)
                    .is_some_and(|token| token.range.end_byte <= right.range.start_byte)
                {
                    self.binary[index] = true;
                }
            }
            _ => {}
        }
    }
}

struct Builder<'a> {
    file: &'a SourceFile,
    tokens: &'a [Token],
    hints: &'a Hints<'a>,
    cursor: usize,
}

impl<'a> Builder<'a> {
    fn text(&self, index: usize) -> &'a str {
        let range = self.tokens[index].range;
        let bom = self.file.original_offset(0);
        &self.file.text()[range.start_byte as usize - bom..range.end_byte as usize - bom]
    }

    fn trivia(&self, index: usize) -> &'a str {
        let bom = self.file.original_offset(0);
        let start = if index == 0 {
            0
        } else {
            self.tokens[index - 1].range.end_byte as usize - bom
        };
        let end = self.tokens[index].range.start_byte as usize - bom;
        self.file.text()[start..end].trim()
    }

    fn protect_term(current: &mut Vec<Doc<'a>>, start: usize) {
        let term = current.split_off(start);
        if !term.is_empty() {
            current.push(Doc::group(term, false));
        }
    }

    fn flush(output: &mut Vec<Doc<'a>>, current: &mut Vec<Doc<'a>>, term_start: &mut usize) {
        let logical = *term_start > 0;
        if *term_start > 0 {
            Self::protect_term(current, *term_start);
            *term_start = 0;
        }
        if !current.is_empty() {
            let parts = std::mem::take(current);
            output.push(if logical {
                Doc::terms(parts)
            } else {
                Doc::group(parts, false)
            });
        }
    }

    fn space_before(&self, index: usize, current: &mut Vec<Doc<'a>>) {
        if index == 0 {
            return;
        }
        let kind = &self.tokens[index].kind;
        let previous = &self.tokens[index - 1].kind;
        let tight_before = matches!(
            kind,
            TokenKind::Comma
                | TokenKind::Semicolon
                | TokenKind::Colon
                | TokenKind::Question
                | TokenKind::Dot
                | TokenKind::RightParen
                | TokenKind::RightBracket
        ) || self.hints.generic[index]
            && matches!(kind, TokenKind::Less | TokenKind::Greater)
            || matches!(kind, TokenKind::LeftParen)
                && (matches!(previous, TokenKind::Identifier(_))
                    || self.hints.generic[index - 1] && matches!(previous, TokenKind::Greater));
        let tight_after = matches!(
            previous,
            TokenKind::Dot
                | TokenKind::Bang
                | TokenKind::Ellipsis
                | TokenKind::LeftParen
                | TokenKind::LeftBracket
                | TokenKind::Comma
        ) || self.hints.generic[index - 1] && matches!(previous, TokenKind::Less)
            || matches!(previous, TokenKind::Minus) && !self.hints.binary[index - 1];
        if matches!(kind, TokenKind::Equal) {
            // Keep the assignment operator with its left side. An indivisible
            // right-hand token may fit on the continuation line only without
            // the operator and its following space.
            if !current.is_empty() {
                current.push(Doc::Text(" "));
            }
        } else if matches!(kind, TokenKind::Dot) {
            current.push(Doc::line("", INDENT));
        } else if !tight_before && !tight_after && !current.is_empty() {
            if self.hints.binary[index]
                || self.hints.binding[index]
                || self.hints.break_before[index]
                || matches!(kind, TokenKind::Arrow | TokenKind::Plus)
                || matches!(
                    previous,
                    TokenKind::Colon | TokenKind::Return | TokenKind::Throw | TokenKind::Equal
                )
            {
                current.push(Doc::line(" ", INDENT));
            } else {
                current.push(Doc::Text(" "));
            }
        }
    }

    fn sequence(&mut self, closing: Option<TokenKind>, list: bool) -> Doc<'a> {
        let mut output = Vec::new();
        let mut current = Vec::new();
        let mut term_start = 0;
        loop {
            let index = self.cursor;
            let token = &self.tokens[index];
            let trivia = self.trivia(index);
            if self.hints.blank_before[index] {
                Self::flush(&mut output, &mut current, &mut term_start);
                output.push(Doc::BlankLine);
            }
            if !trivia.is_empty() {
                Self::flush(&mut output, &mut current, &mut term_start);
                output.push(Doc::HardLine);
                output.push(Doc::Text(trivia));
                output.push(Doc::HardLine);
            }
            if matches!(token.kind, TokenKind::Eof)
                || closing.as_ref().is_some_and(|kind| {
                    std::mem::discriminant(kind) == std::mem::discriminant(&token.kind)
                })
            {
                break;
            }
            let logical = self.hints.binary[index]
                && matches!(token.kind, TokenKind::AndAnd | TokenKind::OrOr);
            if logical {
                Self::protect_term(&mut current, term_start);
            }
            self.space_before(index, &mut current);
            self.cursor += 1;
            match token.kind {
                TokenKind::Identifier(_)
                    if self.hints.binding[index]
                        && self.hints.binding[index + 1]
                        && self.trivia(index + 1).is_empty() =>
                {
                    current.push(Doc::group(
                        vec![
                            Doc::Text(self.text(index)),
                            Doc::line(" ", INDENT),
                            Doc::Text(self.text(index + 1)),
                        ],
                        false,
                    ));
                    self.cursor += 1;
                }
                TokenKind::LeftBrace => {
                    current.push(Doc::Text(self.text(index)));
                    let empty = matches!(self.tokens[self.cursor].kind, TokenKind::RightBrace)
                        && self.trivia(self.cursor).is_empty();
                    if !empty {
                        Self::flush(&mut output, &mut current, &mut term_start);
                        let body = self.sequence(Some(TokenKind::RightBrace), false);
                        output.push(Doc::Nest(
                            INDENT,
                            Box::new(Doc::Concat(vec![Doc::HardLine, body])),
                        ));
                        output.push(Doc::HardLine);
                    }
                    current.push(Doc::Text(self.text(self.cursor)));
                    self.cursor += 1;
                    if !matches!(
                        self.tokens[self.cursor].kind,
                        TokenKind::Comma
                            | TokenKind::Semicolon
                            | TokenKind::Dot
                            | TokenKind::RightParen
                            | TokenKind::RightBracket
                            | TokenKind::Greater
                    ) {
                        Self::flush(&mut output, &mut current, &mut term_start);
                        output.push(Doc::HardLine);
                    }
                }
                TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::Less
                    if !matches!(token.kind, TokenKind::Less) || self.hints.generic[index] =>
                {
                    let close = match token.kind {
                        TokenKind::LeftParen => TokenKind::RightParen,
                        TokenKind::LeftBracket => TokenKind::RightBracket,
                        _ => TokenKind::Greater,
                    };
                    let body = self.sequence(Some(close), true);
                    let empty = matches!(&body, Doc::Concat(parts) if parts.is_empty());
                    let mut parts = vec![Doc::Text(self.text(index))];
                    if !empty {
                        parts.push(Doc::Nest(
                            INDENT,
                            Box::new(Doc::Concat(vec![Doc::line("", 0), body])),
                        ));
                        parts.push(Doc::line("", 0));
                    }
                    parts.push(Doc::Text(self.text(self.cursor)));
                    current.push(Doc::group(parts, true));
                    self.cursor += 1;
                }
                _ => current.push(Doc::Text(self.text(index))),
            }
            if logical {
                term_start = current.len();
            }
            let last = self.cursor - 1;
            if self.hints.break_after[last]
                || matches!(self.tokens[last].kind, TokenKind::Semicolon)
            {
                Self::flush(&mut output, &mut current, &mut term_start);
                output.push(Doc::HardLine);
            } else if matches!(self.tokens[last].kind, TokenKind::Comma) {
                let before_close = closing.as_ref().is_some_and(|kind| {
                    std::mem::discriminant(kind)
                        == std::mem::discriminant(&self.tokens[self.cursor].kind)
                });
                if list {
                    Self::flush(&mut output, &mut current, &mut term_start);
                    if !before_close {
                        output.push(Doc::line(" ", 0));
                    }
                } else {
                    current.push(Doc::line(" ", 0));
                }
            }
        }
        Self::flush(&mut output, &mut current, &mut term_start);
        Doc::Concat(output)
    }
}

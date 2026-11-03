use std::mem::discriminant;

use crate::ast::{
    AllCatch, Arguments, BinaryOperator, Block, CatchBinding, CatchClause, CatchElement,
    CatchPattern, ConstantDecl, Declaration, EnumDecl, EnumVariant, ErrorDecl, ErrorPayloadDecl,
    Export, Expression, ExpressionKind, FieldDecl, FunctionDecl, Import, Literal, Module,
    NativeBinding, NativeBlock, NativeFunctionDecl, NativeMember, Parameter, RecordDecl, Statement,
    StatementKind, SwitchCase, TypeRef, TypeRefKind, UnaryOperator,
};
use crate::lexer::{FormattedPart, Token, TokenKind};
use crate::{Diagnostic, DiagnosticCode, FileId, SourceRange};

pub(crate) fn parse(file_id: FileId, tokens: &[Token]) -> Result<Module, Vec<Diagnostic>> {
    Parser {
        file_id,
        tokens,
        cursor: 0,
    }
    .module()
    .map_err(|diagnostic| vec![diagnostic])
}

struct Parser<'a> {
    file_id: FileId,
    tokens: &'a [Token],
    cursor: usize,
}

impl Parser<'_> {
    fn module(mut self) -> Result<Module, Diagnostic> {
        let start = self.current().range;
        let mut imports = Vec::new();
        while self.at(&TokenKind::Import) {
            imports.push(self.import()?);
        }
        let mut exports = Vec::new();
        let mut declarations = Vec::new();
        let mut native_blocks = Vec::new();
        while !self.at(&TokenKind::Eof) {
            if self.at(&TokenKind::Export) {
                exports.push(self.export()?);
            } else if self.at(&TokenKind::Native) {
                native_blocks.push(self.native_block()?);
            } else if self.at_identifier("class") {
                declarations.extend(self.class()?);
            } else {
                declarations.push(self.declaration()?);
            }
        }
        let end = self.current().range;
        Ok(Module {
            file_id: self.file_id,
            imports,
            exports,
            declarations,
            native_blocks,
            range: start.join(end),
        })
    }

    fn import(&mut self) -> Result<Import, Diagnostic> {
        let start = self.take(&TokenKind::Import)?.range;
        if let TokenKind::String(path) = &self.current().kind {
            let path = path.clone();
            let path_range = self.advance().range;
            if path.is_empty()
                || path.chars().any(char::is_control)
                || std::path::Path::new(&path)
                    .extension()
                    .is_none_or(|value| value != "rinzu")
            {
                return Err(Diagnostic::error(
                    DiagnosticCode::InvalidModulePath,
                    path_range,
                    "file import must name a non-empty `.rinzu` path without control characters",
                    "use a regular String path such as \"./lib/math.rinzu\"",
                ));
            }
            if !self.at_identifier("as") {
                return Err(self.unexpected("`as` followed by a file import alias"));
            }
            self.advance();
            let (alias, alias_range) = self.identifier()?;
            if alias == "rinzu" {
                return Err(Diagnostic::error(
                    DiagnosticCode::InvalidModulePath,
                    alias_range,
                    "file import alias `rinzu` is reserved",
                    "choose another alias for the imported file",
                ));
            }
            let end = self.take(&TokenKind::Semicolon)?.range;
            return Ok(Import {
                path: vec![alias],
                file: Some(crate::ast::FileImport {
                    path,
                    path_range,
                    alias_range,
                }),
                range: start.join(end),
            });
        }
        let (_, path, _) = self.qualified_name()?;
        if self.at_identifier("as") {
            return Err(Diagnostic::error(
                DiagnosticCode::InvalidModulePath,
                self.current().range,
                "an explicit alias requires a file path literal",
                "write `import \"./module.rinzu\" as alias;` or keep the dotted import",
            ));
        }
        let end = self.take(&TokenKind::Semicolon)?.range;
        Ok(Import {
            path,
            file: None,
            range: start.join(end),
        })
    }

    fn export(&mut self) -> Result<Export, Diagnostic> {
        let start = self.take(&TokenKind::Export)?.range;
        let (_, path, _) = self.qualified_name()?;
        let end = self.take(&TokenKind::Semicolon)?.range;
        Ok(Export {
            path,
            implicit: false,
            range: start.join(end),
        })
    }

    fn declaration(&mut self) -> Result<Declaration, Diagnostic> {
        if self.at(&TokenKind::Async) || self.at(&TokenKind::Fn) || self.at_private_method() {
            self.function().map(Declaration::Function)
        } else if self.at(&TokenKind::Record)
            || matches!(&self.current().kind,TokenKind::Identifier(name) if name=="state")
        {
            Err(self.removed(
                "named record/state declarations",
                "use a structural type alias and anonymous record, or class with const fields",
            ))
        } else if self.at(&TokenKind::Enum) {
            self.enum_decl().map(Declaration::Enum)
        } else if self.at_identifier("error") {
            self.error_decl().map(Declaration::Error)
        } else if self.at(&TokenKind::Let) {
            self.constant().map(Declaration::Constant)
        } else if self.at(&TokenKind::Type) {
            self.type_alias().map(Declaration::TypeAlias)
        } else {
            Err(self.unexpected(
                "a top-level function, class, type alias, enum, error, native block, export, or let",
            ))
        }
    }

    fn function(&mut self) -> Result<FunctionDecl, Diagnostic> {
        let start = self.current().range;
        let is_private = self.at_identifier("private");
        if is_private {
            self.advance();
        }
        let is_async = self.consume(&TokenKind::Async).is_some();
        self.take(&TokenKind::Fn)?;
        let (_, path, name_range) = self.qualified_name()?;
        let name = path.join(".");
        if path.len() > 2 {
            return Err(Diagnostic::error(
                DiagnosticCode::InvalidModulePath,
                name_range,
                "method declarations use a local Owner.method name",
                "declare the method in the module that defines its owner type",
            ));
        }
        let type_parameters = self.type_parameters()?;
        self.take(&TokenKind::LeftParen)?;
        let parameters = self.comma_list(TokenKind::RightParen, Self::parameter)?;
        self.take(&TokenKind::RightParen)?;
        let result = if self.consume(&TokenKind::Arrow).is_some() {
            self.type_ref()?
        } else {
            TypeRef::unit(name_range)
        };
        let fallback = if self.consume(&TokenKind::Fallback).is_some() {
            Some(self.expression()?)
        } else {
            None
        };
        let body = self.block()?;
        let range = start.join(body.range);
        Ok(FunctionDecl {
            name,
            is_private,
            lexical_owner: (path.len() == 2).then(|| path[0].clone()),
            type_parameters,
            is_async,
            parameters,
            result,
            body,
            fallback,
            range,
        })
    }

    fn parameter(&mut self) -> Result<Parameter, Diagnostic> {
        let (name, start) = self.identifier()?;
        self.take(&TokenKind::Colon)?;
        let ty = self.type_ref()?;
        Ok(Parameter {
            name,
            range: start.join(ty.range),
            ty,
        })
    }

    fn native_record(&mut self) -> Result<RecordDecl, Diagnostic> {
        let start = self.take(&TokenKind::Record)?.range;
        let (name, _) = self.identifier()?;
        let type_parameters = self.type_parameters()?;
        self.take(&TokenKind::LeftBrace)?;
        let fields = self.comma_list(TokenKind::RightBrace, Self::field)?;
        let end = self.take(&TokenKind::RightBrace)?.range;
        Ok(RecordDecl {
            mutable: false,
            name,
            type_parameters,
            fields,
            range: start.join(end),
        })
    }

    fn removed(&self, surface: &str, alternative: &str) -> Diagnostic {
        Diagnostic::error(
            DiagnosticCode::UnexpectedToken,
            self.current().range,
            format!("{surface} were removed in language 0.5"),
            alternative,
        )
    }

    fn at_identifier(&self, name: &str) -> bool {
        matches!(&self.current().kind, TokenKind::Identifier(value) if value == name)
    }

    fn at_private_method(&self) -> bool {
        self.at_identifier("private")
            && self
                .tokens
                .get(self.cursor + 1)
                .is_some_and(|token| matches!(token.kind, TokenKind::Fn | TokenKind::Async))
    }

    fn class(&mut self) -> Result<Vec<Declaration>, Diagnostic> {
        let start = self.advance().range;
        let (name, _) = self.identifier()?;
        let type_parameters = self.type_parameters()?;
        self.take(&TokenKind::LeftBrace)?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        while !self.at(&TokenKind::RightBrace) {
            if self.at(&TokenKind::Fn) || self.at(&TokenKind::Async) || self.at_private_method() {
                let mut method = self.function()?;
                if method.name.contains('.') {
                    return Err(self.unexpected("an unqualified method name inside class"));
                }
                method.name = format!("{name}.{}", method.name);
                method.lexical_owner = Some(name.clone());
                methods.push(Declaration::Function(method));
            } else {
                let is_const = self.at_identifier("const");
                if is_const {
                    self.advance();
                }
                let mut field = self.field()?;
                field.is_const = is_const;
                fields.push(field);
                if self.consume(&TokenKind::Comma).is_none()
                    && !self.at(&TokenKind::RightBrace)
                    && !self.at(&TokenKind::Fn)
                    && !self.at(&TokenKind::Async)
                    && !self.at_private_method()
                {
                    return Err(self.unexpected("a comma, class method, or closing brace"));
                }
            }
        }
        let end = self.take(&TokenKind::RightBrace)?.range;
        let mut declarations = vec![Declaration::Record(RecordDecl {
            mutable: true,
            name,
            type_parameters,
            fields,
            range: start.join(end),
        })];
        declarations.extend(methods);
        Ok(declarations)
    }

    fn enum_decl(&mut self) -> Result<EnumDecl, Diagnostic> {
        let start = self.take(&TokenKind::Enum)?.range;
        let (name, _) = self.identifier()?;
        let type_parameters = self.type_parameters()?;
        self.take(&TokenKind::LeftBrace)?;
        if self.at(&TokenKind::RightBrace) {
            return Err(self.unexpected("at least one enum variant"));
        }
        let variants = self.comma_list(TokenKind::RightBrace, Self::variant)?;
        let end = self.take(&TokenKind::RightBrace)?.range;
        Ok(EnumDecl {
            name,
            type_parameters,
            variants,
            range: start.join(end),
        })
    }

    fn error_decl(&mut self) -> Result<ErrorDecl, Diagnostic> {
        let start = self.advance().range;
        let (name, _) = self.identifier()?;
        let type_parameters = self.type_parameters()?;
        self.take(&TokenKind::LeftBrace)?;
        let payload = if self.at_identifier("payload") {
            let start = self.advance().range;
            let optional = self.consume(&TokenKind::Question).is_some();
            self.take(&TokenKind::Colon)?;
            let ty = self.type_ref()?;
            let range = start.join(ty.range);
            self.consume(&TokenKind::Comma);
            Some(ErrorPayloadDecl {
                optional,
                ty,
                range,
            })
        } else {
            None
        };
        let end = self.take(&TokenKind::RightBrace)?.range;
        Ok(ErrorDecl {
            name,
            type_parameters,
            payload,
            range: start.join(end),
        })
    }

    fn type_parameters(&mut self) -> Result<Vec<crate::ast::TypeParameter>, Diagnostic> {
        if self.consume(&TokenKind::Less).is_none() {
            return Ok(Vec::new());
        }
        let parameters = self.comma_list(TokenKind::Greater, |parser| {
            let (name, range) = parser.identifier()?;
            let mut constraints = Vec::new();
            if parser.consume(&TokenKind::Colon).is_some() {
                constraints.push(parser.identifier()?.0);
                while parser.consume(&TokenKind::Plus).is_some() {
                    constraints.push(parser.identifier()?.0);
                }
            }
            Ok(crate::ast::TypeParameter {
                name,
                constraints,
                range,
            })
        })?;
        self.take(&TokenKind::Greater)?;
        if parameters.is_empty() {
            return Err(self.unexpected("at least one type parameter"));
        }
        Ok(parameters)
    }

    fn type_arguments(&mut self) -> Result<Vec<TypeRef>, Diagnostic> {
        self.take(&TokenKind::Less)?;
        let arguments = self.comma_list(TokenKind::Greater, Self::type_ref)?;
        self.take(&TokenKind::Greater)?;
        if arguments.is_empty() {
            return Err(self.unexpected("at least one type argument"));
        }
        Ok(arguments)
    }

    fn variant(&mut self) -> Result<EnumVariant, Diagnostic> {
        let (name, start) = self.identifier()?;
        let (fields, end) = if self.consume(&TokenKind::LeftParen).is_some() {
            let fields = self.comma_list(TokenKind::RightParen, Self::field)?;
            let end = self.take(&TokenKind::RightParen)?.range;
            (fields, end)
        } else {
            (Vec::new(), start)
        };
        Ok(EnumVariant {
            name,
            fields,
            range: start.join(end),
        })
    }

    fn field(&mut self) -> Result<FieldDecl, Diagnostic> {
        let (name, start) = self.identifier()?;
        self.take(&TokenKind::Colon)?;
        let ty = self.type_ref()?;
        Ok(FieldDecl {
            name,
            is_const: true,
            range: start.join(ty.range),
            ty,
        })
    }

    fn type_ref(&mut self) -> Result<TypeRef, Diagnostic> {
        let start = self.current().range;
        if self.consume(&TokenKind::LeftBrace).is_some() {
            let fields = self.comma_list(TokenKind::RightBrace, Self::field)?;
            let end = self.take(&TokenKind::RightBrace)?.range;
            if fields.is_empty() {
                return Err(self.unexpected("at least one record type field"));
            }
            return Ok(TypeRef {
                kind: TypeRefKind::Record(fields),
                range: start.join(end),
            });
        }
        let supplant = self.consume(&TokenKind::Supplant).is_some();
        if supplant || self.at(&TokenKind::Fn) {
            self.take(&TokenKind::Fn)?;
            self.take(&TokenKind::LeftParen)?;
            let parameters = self.comma_list(TokenKind::RightParen, Self::type_ref)?;
            self.take(&TokenKind::RightParen)?;
            self.take(&TokenKind::Arrow)?;
            let result = self.type_ref()?;
            let range = start.join(result.range);
            return Ok(TypeRef {
                kind: TypeRefKind::Function {
                    supplant,
                    parameters,
                    result: Box::new(result),
                },
                range,
            });
        }
        let (name, path, start) = self.qualified_name()?;
        if path.len() == 1 && name == "Bytes" && self.consume(&TokenKind::LeftParen).is_some() {
            let length = self.expression()?;
            let end = self.take(&TokenKind::RightParen)?.range;
            return Ok(TypeRef {
                kind: TypeRefKind::Bytes(Box::new(length)),
                range: start.join(end),
            });
        }
        if path.len() == 1 && matches!(name.as_str(), "List" | "Map" | "Future") {
            self.take(&TokenKind::Less)?;
            let first = self.type_ref()?;
            let kind = match name.as_str() {
                "List" => TypeRefKind::List(Box::new(first)),
                "Future" => TypeRefKind::Future(Box::new(first)),
                "Map" => {
                    self.take(&TokenKind::Comma)?;
                    TypeRefKind::Map(Box::new(first), Box::new(self.type_ref()?))
                }
                _ => unreachable!(),
            };
            let end = self.take(&TokenKind::Greater)?.range;
            Ok(TypeRef {
                kind,
                range: start.join(end),
            })
        } else if self.at(&TokenKind::Less) {
            let arguments = self.type_arguments()?;
            Ok(TypeRef {
                kind: TypeRefKind::Generic { path, arguments },
                range: start.join(self.previous().range),
            })
        } else {
            Ok(TypeRef {
                kind: TypeRefKind::Named(path),
                range: start.join(self.previous().range),
            })
        }
    }

    fn constant(&mut self) -> Result<ConstantDecl, Diagnostic> {
        let start = self.take(&TokenKind::Let)?.range;
        let (name, _) = self.identifier()?;
        let annotation = if self.consume(&TokenKind::Colon).is_some() {
            Some(self.type_ref()?)
        } else {
            None
        };
        self.take(&TokenKind::Equal)?;
        let initializer = self.expression()?;
        let end = self.take(&TokenKind::Semicolon)?.range;
        Ok(ConstantDecl {
            name,
            annotation,
            initializer,
            range: start.join(end),
        })
    }

    fn type_alias(&mut self) -> Result<crate::ast::TypeAliasDecl, Diagnostic> {
        let start = self.take(&TokenKind::Type)?.range;
        let (name, _) = self.identifier()?;
        let type_parameters = self.type_parameters()?;
        self.take(&TokenKind::Equal)?;
        let ty = self.type_ref()?;
        let end = self.take(&TokenKind::Semicolon)?.range;
        Ok(crate::ast::TypeAliasDecl {
            name,
            type_parameters,
            ty,
            range: start.join(end),
        })
    }

    fn native_block(&mut self) -> Result<NativeBlock, Diagnostic> {
        let start = self.take(&TokenKind::Native)?.range;
        self.take(&TokenKind::LeftBrace)?;
        let mut members = Vec::new();
        while !self.at(&TokenKind::RightBrace) && !self.at(&TokenKind::Eof) {
            if matches!(&self.current().kind, TokenKind::Identifier(word) if word == "opaque") {
                let start = self.advance().range;
                let (name, _) = self.identifier()?;
                let end = self.take(&TokenKind::Semicolon)?.range;
                members.push(NativeMember::Opaque {
                    name,
                    range: start.join(end),
                });
            } else if self.at(&TokenKind::Type) {
                return Err(Diagnostic::error(
                    DiagnosticCode::UnsupportedSafeFfi,
                    self.current().range,
                    "native `type T;` declarations have been removed",
                    "declare a memberless native handle with `opaque T;`",
                ));
            } else if self.at(&TokenKind::Record) {
                members.push(NativeMember::Record(self.native_record()?));
            } else if matches!(&self.current().kind, TokenKind::Identifier(word) if word == "userdata")
            {
                return Err(Diagnostic::error(
                    DiagnosticCode::UnsupportedSafeFfi,
                    self.current().range,
                    "native `userdata` declarations have been removed",
                    "pass an ordinary structural record directly; wrap a class or enum in a record",
                ));
            } else {
                members.push(NativeMember::Function(self.native_function()?));
            }
        }
        let end = self.take(&TokenKind::RightBrace)?.range;
        Ok(NativeBlock {
            members,
            range: start.join(end),
        })
    }

    fn native_function(&mut self) -> Result<NativeFunctionDecl, Diagnostic> {
        let start = self.current().range;
        let is_supplant = self.consume(&TokenKind::Supplant).is_some();
        if self.at(&TokenKind::Async) {
            return Err(Diagnostic::error(
                DiagnosticCode::UnsupportedSafeFfi,
                self.current().range,
                "async native declarations and service bindings have been removed",
                "use rinzu.timer.sleep; SDKs can await nativeCompleted or nativeTransferred",
            ));
        }
        self.take(&TokenKind::Fn)?;
        let (name, name_range) = self.identifier()?;
        self.take(&TokenKind::LeftParen)?;
        let parameters = self.comma_list(TokenKind::RightParen, Self::parameter)?;
        self.take(&TokenKind::RightParen)?;
        let result = if self.consume(&TokenKind::Arrow).is_some() {
            self.type_ref()?
        } else {
            TypeRef::unit(name_range)
        };
        let (word, _) = self.identifier()?;
        if word != "symbol" {
            return Err(self.unexpected("symbol"));
        }
        let token = self.advance().clone();
        let TokenKind::String(value) = token.kind else {
            return Err(self.unexpected("a String literal binding"));
        };
        let binding = NativeBinding::Symbol(value);
        let end = self.take(&TokenKind::Semicolon)?.range;
        Ok(NativeFunctionDecl {
            name,
            is_async: false,
            is_supplant,
            parameters,
            result,
            binding,
            binding_range: token.range,
            range: start.join(end),
        })
    }

    fn block(&mut self) -> Result<Block, Diagnostic> {
        let start = self.take(&TokenKind::LeftBrace)?.range;
        let mut statements = Vec::new();
        while !self.at(&TokenKind::RightBrace) && !self.at(&TokenKind::Eof) {
            statements.push(self.statement()?);
        }
        let end = self.take(&TokenKind::RightBrace)?.range;
        Ok(Block {
            statements,
            range: start.join(end),
        })
    }

    fn statement(&mut self) -> Result<Statement, Diagnostic> {
        if self.at(&TokenKind::LeftBrace) {
            let block = self.block()?;
            return Ok(Statement {
                range: block.range,
                kind: StatementKind::Block(block),
            });
        }
        if self.at(&TokenKind::Let) || self.at(&TokenKind::Var) {
            return self.local();
        }
        if self.at(&TokenKind::Return) {
            return self.return_statement();
        }
        if self.at(&TokenKind::Break) || self.at(&TokenKind::Continue) {
            let token = self.advance().clone();
            let kind = if same_kind(&token.kind, &TokenKind::Break) {
                StatementKind::Break
            } else {
                StatementKind::Continue
            };
            let end = self.take(&TokenKind::Semicolon)?.range;
            return Ok(Statement {
                kind,
                range: token.range.join(end),
            });
        }
        if self.at(&TokenKind::If) {
            return self.if_statement();
        }
        if self.at(&TokenKind::While) {
            return self.while_statement();
        }
        if self.at(&TokenKind::Switch) {
            return self.switch_statement();
        }
        if self.at(&TokenKind::TaskScope) {
            return Err(self.removed(
                "taskScope statements",
                "use try all { ... } catch errors { ... } finally { ... }",
            ));
        }
        if self.at(&TokenKind::Try) {
            return self.try_finally();
        }
        if self.at(&TokenKind::Throw) {
            let start = self.advance().range;
            let value = self.expression()?;
            let end = self.take(&TokenKind::Semicolon)?.range;
            return Ok(Statement {
                kind: StatementKind::Throw(value),
                range: start.join(end),
            });
        }
        if matches!(&self.current().kind, TokenKind::Identifier(_)) {
            let mut lookahead = 1;
            while self
                .peek_kind(lookahead)
                .is_some_and(|k| same_kind(k, &TokenKind::Dot))
                && matches!(
                    self.peek_kind(lookahead + 1),
                    Some(TokenKind::Identifier(_))
                )
            {
                lookahead += 2;
            }
            if self
                .peek_kind(lookahead)
                .is_some_and(|k| same_kind(k, &TokenKind::Equal))
            {
                return self.assignment();
            }
        }
        let expression = self.expression()?;
        let start = expression.range;
        let end = self.take(&TokenKind::Semicolon)?.range;
        Ok(Statement {
            kind: StatementKind::Expression(expression),
            range: start.join(end),
        })
    }

    fn local(&mut self) -> Result<Statement, Diagnostic> {
        let start_token = self.advance().clone();
        let mutable = same_kind(&start_token.kind, &TokenKind::Var);
        let (name, _) = self.identifier()?;
        let annotation = if self.consume(&TokenKind::Colon).is_some() {
            Some(self.type_ref()?)
        } else {
            None
        };
        self.take(&TokenKind::Equal)?;
        let initializer = self.expression()?;
        let end = self.take(&TokenKind::Semicolon)?.range;
        Ok(Statement {
            kind: StatementKind::Local {
                mutable,
                name,
                annotation,
                initializer,
            },
            range: start_token.range.join(end),
        })
    }

    fn assignment(&mut self) -> Result<Statement, Diagnostic> {
        let (_, mut path, start) = self.qualified_name()?;
        self.take(&TokenKind::Equal)?;
        let value = self.expression()?;
        let end = self.take(&TokenKind::Semicolon)?.range;
        Ok(Statement {
            kind: if path.len() == 1 {
                StatementKind::Assign {
                    name: path.remove(0),
                    value,
                }
            } else {
                let field = path.pop().unwrap();
                StatementKind::FieldAssign {
                    base: Expression {
                        kind: ExpressionKind::Name(path),
                        range: start,
                    },
                    field,
                    value,
                }
            },
            range: start.join(end),
        })
    }

    fn return_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.take(&TokenKind::Return)?.range;
        let value = if self.at(&TokenKind::Semicolon) {
            None
        } else {
            Some(self.expression()?)
        };
        let end = self.take(&TokenKind::Semicolon)?.range;
        Ok(Statement {
            kind: StatementKind::Return(value),
            range: start.join(end),
        })
    }

    fn if_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.take(&TokenKind::If)?.range;
        let condition = self.expression()?;
        let then_block = self.block()?;
        let else_branch = if self.consume(&TokenKind::Else).is_some() {
            if self.at(&TokenKind::If) {
                Some(Box::new(self.if_statement()?))
            } else {
                let block = self.block()?;
                let range = block.range;
                Some(Box::new(Statement {
                    kind: StatementKind::Block(block),
                    range,
                }))
            }
        } else {
            None
        };
        let end = else_branch
            .as_ref()
            .map_or(then_block.range, |branch| branch.range);
        Ok(Statement {
            kind: StatementKind::If {
                condition,
                then_block,
                else_branch,
            },
            range: start.join(end),
        })
    }

    fn while_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.take(&TokenKind::While)?.range;
        let condition = self.expression()?;
        let body = self.block()?;
        let range = start.join(body.range);
        Ok(Statement {
            kind: StatementKind::While { condition, body },
            range,
        })
    }

    fn switch_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.take(&TokenKind::Switch)?.range;
        let value = self.expression()?;
        self.take(&TokenKind::LeftBrace)?;
        let mut cases = Vec::new();
        while self.at(&TokenKind::Case) {
            let case_start = self.take(&TokenKind::Case)?.range;
            let (_, path, _) = self.qualified_name()?;
            let bindings = if self.consume(&TokenKind::LeftParen).is_some() {
                let values = self.comma_list(TokenKind::RightParen, |parser| {
                    parser.identifier().map(|(name, _)| name)
                })?;
                self.take(&TokenKind::RightParen)?;
                values
            } else {
                Vec::new()
            };
            self.take(&TokenKind::Colon)?;
            let body = self.block()?;
            cases.push(SwitchCase {
                path,
                bindings,
                range: case_start.join(body.range),
                body,
            });
        }
        let end = self.take(&TokenKind::RightBrace)?.range;
        Ok(Statement {
            kind: StatementKind::Switch { value, cases },
            range: start.join(end),
        })
    }

    fn try_finally(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.take(&TokenKind::Try)?.range;
        let all = self.at_identifier("all");
        if all {
            self.advance();
        }
        let body = self.block()?;
        let mut handlers = Vec::new();
        while let Some(token) = self.consume(&TokenKind::Catch) {
            let catch_start = token.range;
            let pattern = self.catch_pattern()?;
            let body = self.block()?;
            handlers.push(CatchClause {
                pattern,
                range: catch_start.join(body.range),
                body,
            });
        }
        let mut cleanup = if self.consume(&TokenKind::Finally).is_some() {
            Some(self.block()?)
        } else {
            None
        };
        if handlers.is_empty() && cleanup.is_none() {
            return Err(self.unexpected("at least one catch or finally block"));
        }
        let end = cleanup
            .as_ref()
            .map(|block| block.range)
            .or_else(|| handlers.last().map(|handler| handler.body.range))
            .unwrap_or(body.range);
        let range = start.join(end);
        // Preserve the existing executable representation until the new failure path is connected.
        if handlers.len() == 1
            && let CatchPattern::Binding(binding) = &handlers[0].pattern
        {
            let binding = binding.clone();
            let handler = handlers.remove(0).body;
            let kind = if all {
                StatementKind::TryAll {
                    body,
                    handler: Some(AllCatch {
                        binding: binding.name,
                        binding_range: binding.range,
                        body: handler,
                    }),
                    cleanup,
                }
            } else {
                StatementKind::TryCatch {
                    body,
                    binding: binding.name,
                    binding_range: binding.range,
                    handler,
                    cleanup,
                }
            };
            return Ok(Statement { kind, range });
        }
        if handlers.is_empty()
            && let Some(cleanup) = cleanup.take()
        {
            let kind = if all {
                StatementKind::TryAll {
                    body,
                    handler: None,
                    cleanup: Some(cleanup),
                }
            } else {
                StatementKind::TryFinally { body, cleanup }
            };
            return Ok(Statement { kind, range });
        }
        Ok(Statement {
            kind: StatementKind::TryErrors {
                all,
                body,
                handlers,
                cleanup,
            },
            range,
        })
    }

    fn catch_pattern(&mut self) -> Result<CatchPattern, Diagnostic> {
        if !self.at(&TokenKind::LeftBracket) {
            let (name, range) = self.identifier()?;
            return Ok(CatchPattern::Binding(CatchBinding { name, range }));
        }
        let start = self.advance().range;
        let mut elements = Vec::new();
        let mut rest = None;
        while !self.at(&TokenKind::RightBracket) {
            if self.consume(&TokenKind::Ellipsis).is_some() {
                let (name, range) = self.identifier()?;
                rest = Some(CatchBinding { name, range });
                self.consume(&TokenKind::Comma);
                if !self.at(&TokenKind::RightBracket) {
                    return Err(self.unexpected("the rest binding at the end of the catch pattern"));
                }
                break;
            }
            let element_start = self.current().range;
            let binding = if matches!(&self.current().kind, TokenKind::Identifier(_))
                && matches!(self.peek_kind(1), Some(TokenKind::Colon))
            {
                let (name, range) = self.identifier()?;
                self.take(&TokenKind::Colon)?;
                Some(CatchBinding { name, range })
            } else {
                None
            };
            let ty = self.type_ref()?;
            elements.push(CatchElement {
                binding,
                range: element_start.join(ty.range),
                ty,
            });
            if self.consume(&TokenKind::Comma).is_none() {
                break;
            }
        }
        let end = self.take(&TokenKind::RightBracket)?.range;
        let range = start.join(end);
        if elements.is_empty() && rest.is_none() {
            return Err(Diagnostic::error(
                DiagnosticCode::InvalidCatchPattern,
                range,
                "catch [] cannot match a nonempty failure list",
                "match a typed element or bind the whole failure list",
            ));
        }
        Ok(CatchPattern::List {
            elements,
            rest,
            range,
        })
    }

    fn expression(&mut self) -> Result<Expression, Diagnostic> {
        self.binary(0)
    }

    fn binary(&mut self, minimum_precedence: u8) -> Result<Expression, Diagnostic> {
        let mut left = self.unary()?;
        loop {
            // Contextual infix keyword: declarations and members named `is`
            // remain ordinary identifiers outside this expression position.
            if matches!(&self.current().kind, TokenKind::Identifier(name) if name == "is") {
                if 4 < minimum_precedence {
                    break;
                }
                self.advance();
                let ty = self.type_ref()?;
                let range = left.range.join(ty.range);
                left = Expression {
                    kind: ExpressionKind::Is {
                        value: Box::new(left),
                        ty,
                    },
                    range,
                };
                continue;
            }
            let Some((operator, precedence)) = binary_operator(&self.current().kind) else {
                break;
            };
            if precedence < minimum_precedence {
                break;
            }
            self.advance();
            let right = self.binary(precedence + 1)?;
            let range = left.range.join(right.range);
            left = Expression {
                kind: ExpressionKind::Binary {
                    operator,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                range,
            };
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expression, Diagnostic> {
        if matches!(&self.current().kind, TokenKind::Identifier(name) if name == "bind")
            && matches!(self.peek_kind(1), Some(TokenKind::Identifier(_)))
        {
            return Err(self.removed("bind expressions", "use fn(argument:Type)->Result { return target(captured, argument); } with all target arguments"));
        }
        if self.at(&TokenKind::Minus)
            && matches!(self.peek_kind(1), Some(TokenKind::IntMagnitude(value)) if *value == (i64::MAX as u64) + 1)
        {
            let start = self.advance().range;
            let end = self.advance().range;
            return Ok(Expression {
                kind: ExpressionKind::Literal(Literal::Int(i64::MIN)),
                range: start.join(end),
            });
        }
        if self.at(&TokenKind::Bang) || self.at(&TokenKind::Minus) {
            let token = self.advance().clone();
            let operator = if same_kind(&token.kind, &TokenKind::Bang) {
                UnaryOperator::Not
            } else {
                UnaryOperator::Negate
            };
            let operand = self.unary()?;
            let range = token.range.join(operand.range);
            return Ok(Expression {
                kind: ExpressionKind::Unary {
                    operator,
                    operand: Box::new(operand),
                },
                range,
            });
        }
        if self.at(&TokenKind::Await) {
            let start = self.advance().range;
            let operand = self.unary()?;
            let range = start.join(operand.range);
            return Ok(Expression {
                kind: ExpressionKind::Await(Box::new(operand)),
                range,
            });
        }
        if self.at(&TokenKind::Spawn) {
            let start = self.advance().range;
            let mut expression = self.primary()?;
            if let ExpressionKind::GenericCall { spawn, .. } = &mut expression.kind {
                *spawn = true;
                expression.range = start.join(expression.range);
                return Ok(expression);
            }
            let ExpressionKind::Call { target, arguments } = expression.kind else {
                return Err(Diagnostic::error(
                    DiagnosticCode::UnexpectedToken,
                    expression.range,
                    "`spawn` requires a direct function invocation",
                    "write `spawn function(arguments)`",
                ));
            };
            return Ok(Expression {
                range: start.join(expression.range),
                kind: ExpressionKind::Spawn { target, arguments },
            });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expression, Diagnostic> {
        let base = self.primary()?;
        if self.at(&TokenKind::With) {
            return Err(self.removed(
                "with expressions",
                "create an anonymous record or class value by enumerating every field",
            ));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<Expression, Diagnostic> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Fn | TokenKind::Async => self.function_expression(),
            TokenKind::IntMagnitude(value) if value <= i64::MAX as u64 => {
                self.literal(token.range, Literal::Int(value as i64))
            }
            TokenKind::IntMagnitude(value) => self.literal(token.range, Literal::Unsigned(value)),
            TokenKind::DecimalInteger(spelling) => {
                self.literal(token.range, Literal::DecimalInteger(spelling))
            }
            TokenKind::Float { value, spelling } => {
                self.literal(token.range, Literal::Float { value, spelling })
            }
            TokenKind::String(value) => self.literal(token.range, Literal::String(value)),
            TokenKind::FormattedString(parts) => self.formatted_string(token.range, parts),
            TokenKind::Bytes(value) => self.literal(token.range, Literal::Bytes(value)),
            TokenKind::True => self.literal(token.range, Literal::Bool(true)),
            TokenKind::False => self.literal(token.range, Literal::Bool(false)),
            TokenKind::Identifier(_) => self.name_or_call(),
            TokenKind::LeftParen => self.parenthesized(),
            TokenKind::LeftBracket => self.list(),
            TokenKind::LeftBrace => self.record_or_map(),
            _ => Err(self.unexpected("an expression")),
        }
    }

    fn function_expression(&mut self) -> Result<Expression, Diagnostic> {
        let start = self.current().range;
        let is_async = self.consume(&TokenKind::Async).is_some();
        self.take(&TokenKind::Fn)?;
        self.take(&TokenKind::LeftParen)?;
        let parameters = self.comma_list(TokenKind::RightParen, Self::parameter)?;
        self.take(&TokenKind::RightParen)?;
        let result = if self.consume(&TokenKind::Arrow).is_some() {
            self.type_ref()?
        } else {
            TypeRef::unit(start)
        };
        let fallback = if self.consume(&TokenKind::Fallback).is_some() {
            Some(self.expression()?)
        } else {
            None
        };
        let body = self.block()?;
        let range = start.join(body.range);
        Ok(Expression {
            range,
            kind: ExpressionKind::Function(Box::new(FunctionDecl {
                name: String::new(),
                is_private: false,
                lexical_owner: None,
                type_parameters: Vec::new(),
                is_async,
                parameters,
                result,
                body,
                fallback,
                range,
            })),
        })
    }

    fn formatted_string(
        &mut self,
        range: SourceRange,
        parts: Vec<FormattedPart>,
    ) -> Result<Expression, Diagnostic> {
        self.advance();
        let parts = parts
            .into_iter()
            .map(|part| match part {
                FormattedPart::Text { value, range } => Ok(Expression {
                    kind: ExpressionKind::Literal(Literal::String(value)),
                    range,
                }),
                FormattedPart::Expression { tokens, range } => {
                    let mut parser = Parser {
                        file_id: self.file_id,
                        tokens: &tokens,
                        cursor: 0,
                    };
                    if parser.at(&TokenKind::Eof) {
                        return Err(Diagnostic::error(
                            DiagnosticCode::UnexpectedToken,
                            range,
                            "empty formatted String expression",
                            "insert an expression between the braces, or escape literal braces",
                        ));
                    }
                    let expression = parser.expression()?;
                    parser.take(&TokenKind::Eof)?;
                    Ok(expression)
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(Expression {
            kind: ExpressionKind::FormatString(parts),
            range,
        })
    }

    fn literal(&mut self, range: SourceRange, literal: Literal) -> Result<Expression, Diagnostic> {
        self.advance();
        Ok(Expression {
            kind: ExpressionKind::Literal(literal),
            range,
        })
    }

    fn name_or_call(&mut self) -> Result<Expression, Diagnostic> {
        let (_, path, start) = self.qualified_name()?;
        if self.at(&TokenKind::Less) {
            let saved = self.cursor;
            if let Ok(type_arguments) = self.type_arguments() {
                let variant = if self.consume(&TokenKind::Dot).is_some() {
                    Some(self.identifier()?.0)
                } else {
                    None
                };
                if self.consume(&TokenKind::LeftParen).is_some() {
                    let arguments = self.arguments()?;
                    let close = self.take(&TokenKind::RightParen)?.range;
                    return Ok(Expression {
                        kind: ExpressionKind::GenericCall {
                            target: path,
                            type_arguments,
                            variant,
                            arguments,
                            spawn: false,
                        },
                        range: start.join(close),
                    });
                }
            }
            self.cursor = saved;
        }
        let end = self.previous().range;
        if self.consume(&TokenKind::LeftParen).is_none() {
            return Ok(Expression {
                kind: ExpressionKind::Name(path),
                range: start.join(end),
            });
        }
        let arguments = self.arguments()?;
        let close = self.take(&TokenKind::RightParen)?.range;
        Ok(Expression {
            kind: ExpressionKind::Call {
                target: path,
                arguments,
            },
            range: start.join(close),
        })
    }

    fn arguments(&mut self) -> Result<Arguments, Diagnostic> {
        if self.at(&TokenKind::RightParen) {
            return Ok(Arguments::Positional(Vec::new()));
        }
        let named = matches!(&self.current().kind, TokenKind::Identifier(_))
            && self
                .peek_kind(1)
                .is_some_and(|kind| same_kind(kind, &TokenKind::Colon));
        if named {
            let values = self.comma_list(TokenKind::RightParen, |parser| {
                let (name, start) = parser.identifier()?;
                parser.take(&TokenKind::Colon)?;
                let expression = parser.expression()?;
                Ok((name, expression.clone(), start.join(expression.range)))
            })?;
            Ok(Arguments::Named(values))
        } else {
            self.comma_list(TokenKind::RightParen, Self::expression)
                .map(Arguments::Positional)
        }
    }

    fn parenthesized(&mut self) -> Result<Expression, Diagnostic> {
        let start = self.take(&TokenKind::LeftParen)?.range;
        if self.at(&TokenKind::RightParen) {
            let end = self.advance().range;
            return Ok(Expression {
                kind: ExpressionKind::Unit,
                range: start.join(end),
            });
        }
        let mut expression = self.expression()?;
        let end = self.take(&TokenKind::RightParen)?.range;
        expression.range = start.join(end);
        Ok(expression)
    }

    fn list(&mut self) -> Result<Expression, Diagnostic> {
        let start = self.take(&TokenKind::LeftBracket)?.range;
        let values = self.comma_list(TokenKind::RightBracket, Self::expression)?;
        let end = self.take(&TokenKind::RightBracket)?.range;
        Ok(Expression {
            kind: ExpressionKind::List(values),
            range: start.join(end),
        })
    }

    fn map(&mut self) -> Result<Expression, Diagnostic> {
        let start = self.take(&TokenKind::LeftBrace)?.range;
        let entries = self.comma_list(TokenKind::RightBrace, |parser| {
            let key = parser.expression()?;
            parser.take(&TokenKind::Colon)?;
            let value = parser.expression()?;
            Ok((key, value))
        })?;
        let end = self.take(&TokenKind::RightBrace)?.range;
        Ok(Expression {
            kind: ExpressionKind::Map(entries),
            range: start.join(end),
        })
    }

    fn record_or_map(&mut self) -> Result<Expression, Diagnostic> {
        // A record starts with name[ : Type ] =. A Map keeps key : value.
        let save = self.cursor;
        self.advance();
        let is_record = if matches!(self.current().kind, TokenKind::Identifier(_)) {
            self.advance();
            if self.at(&TokenKind::Equal) {
                true
            } else if self.consume(&TokenKind::Colon).is_some() {
                self.type_ref().is_ok() && self.at(&TokenKind::Equal)
            } else {
                false
            }
        } else {
            false
        };
        self.cursor = save;
        if !is_record {
            return self.map();
        }
        let start = self.take(&TokenKind::LeftBrace)?.range;
        let fields = self.comma_list(TokenKind::RightBrace, |parser| {
            let (name, start) = parser.identifier()?;
            let annotation = if parser.consume(&TokenKind::Colon).is_some() {
                Some(parser.type_ref()?)
            } else {
                None
            };
            parser.take(&TokenKind::Equal)?;
            let value = parser.expression()?;
            Ok(crate::ast::RecordField {
                name,
                annotation,
                range: start.join(value.range),
                value,
            })
        })?;
        let end = self.take(&TokenKind::RightBrace)?.range;
        Ok(Expression {
            kind: ExpressionKind::Record(fields),
            range: start.join(end),
        })
    }

    fn qualified_name(&mut self) -> Result<(String, Vec<String>, SourceRange), Diagnostic> {
        let (first, start) = self.identifier()?;
        let mut path = vec![first.clone()];
        while self.consume(&TokenKind::Dot).is_some() {
            path.push(self.identifier()?.0);
        }
        Ok((first, path, start))
    }

    fn identifier(&mut self) -> Result<(String, SourceRange), Diagnostic> {
        let token = self.current().clone();
        if let TokenKind::Identifier(name) = token.kind {
            self.advance();
            Ok((name, token.range))
        } else {
            Err(self.unexpected("an identifier"))
        }
    }

    fn comma_list<T>(
        &mut self,
        end: TokenKind,
        mut parse_item: impl FnMut(&mut Self) -> Result<T, Diagnostic>,
    ) -> Result<Vec<T>, Diagnostic> {
        let mut items = Vec::new();
        while !self.at(&end) {
            items.push(parse_item(self)?);
            if self.consume(&TokenKind::Comma).is_none() {
                break;
            }
        }
        Ok(items)
    }

    fn current(&self) -> &Token {
        &self.tokens[self.cursor]
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.cursor.saturating_sub(1)]
    }

    fn peek_kind(&self, distance: usize) -> Option<&TokenKind> {
        self.tokens
            .get(self.cursor + distance)
            .map(|token| &token.kind)
    }

    fn advance(&mut self) -> &Token {
        let index = self.cursor;
        if !self.at(&TokenKind::Eof) {
            self.cursor += 1;
        }
        &self.tokens[index]
    }

    fn at(&self, kind: &TokenKind) -> bool {
        same_kind(&self.current().kind, kind)
    }

    fn consume(&mut self, kind: &TokenKind) -> Option<&Token> {
        if self.at(kind) {
            Some(self.advance())
        } else {
            None
        }
    }

    fn take(&mut self, kind: &TokenKind) -> Result<&Token, Diagnostic> {
        if self.at(kind) {
            Ok(self.advance())
        } else {
            Err(Diagnostic::error(
                DiagnosticCode::ExpectedToken,
                self.current().range,
                format!(
                    "expected {}, found {}",
                    kind.label(),
                    self.current().kind.label()
                ),
                format!("insert {} here", kind.label()),
            ))
        }
    }

    fn unexpected(&self, expected: &str) -> Diagnostic {
        Diagnostic::error(
            DiagnosticCode::UnexpectedToken,
            self.current().range,
            format!("expected {expected}, found {}", self.current().kind.label()),
            format!("replace this token with {expected}"),
        )
    }
}

fn same_kind(left: &TokenKind, right: &TokenKind) -> bool {
    discriminant(left) == discriminant(right)
}

fn binary_operator(kind: &TokenKind) -> Option<(BinaryOperator, u8)> {
    Some(match kind {
        TokenKind::OrOr => (BinaryOperator::Or, 1),
        TokenKind::AndAnd => (BinaryOperator::And, 2),
        TokenKind::EqualEqual => (BinaryOperator::Equal, 3),
        TokenKind::BangEqual => (BinaryOperator::NotEqual, 3),
        TokenKind::Less => (BinaryOperator::Less, 4),
        TokenKind::LessEqual => (BinaryOperator::LessEqual, 4),
        TokenKind::Greater => (BinaryOperator::Greater, 4),
        TokenKind::GreaterEqual => (BinaryOperator::GreaterEqual, 4),
        TokenKind::Plus => (BinaryOperator::Add, 5),
        TokenKind::Minus => (BinaryOperator::Subtract, 5),
        TokenKind::Star => (BinaryOperator::Multiply, 6),
        TokenKind::Slash => (BinaryOperator::Divide, 6),
        TokenKind::Percent => (BinaryOperator::Remainder, 6),
        _ => return None,
    })
}

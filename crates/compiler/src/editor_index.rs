//! Source navigation uses parsed lexical scopes and checked, linked identities.
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use crate::ast::*;
use crate::ast_walk::{self, Visitor};
use crate::lexer::{FormattedPart, Token, TokenKind, lex};
use crate::{SourceFile, SourceRange};

#[derive(Clone, Debug)]
pub struct EditorSymbol {
    pub name: String,
    pub kind: &'static str,
    pub definition: SourceRange,
    pub scope: SourceRange,
    pub visible_from: u32,
}
#[derive(Clone, Debug)]
pub struct EditorReference {
    pub range: SourceRange,
    pub definition: SourceRange,
}
#[derive(Default)]
pub struct EditorIndex {
    pub symbols: Vec<EditorSymbol>,
    pub references: Vec<EditorReference>,
    pub targets: BTreeMap<String, SourceRange>,
}

impl EditorIndex {
    #[must_use]
    pub fn definition_at(&self, range: SourceRange) -> Option<SourceRange> {
        self.references
            .iter()
            .filter(|reference| contains(reference.range, range))
            .min_by_key(|reference| reference.range.end_byte - reference.range.start_byte)
            .map(|reference| reference.definition)
            .or_else(|| {
                self.symbols
                    .iter()
                    .find(|symbol| contains(symbol.definition, range))
                    .map(|symbol| symbol.definition)
            })
    }
    #[must_use]
    pub fn visible_symbols(&self, position: SourceRange) -> Vec<EditorSymbol> {
        let mut symbols: Vec<_> = self
            .symbols
            .iter()
            .filter(|symbol| {
                symbol.scope.file_id == position.file_id
                    && symbol.scope.start_byte <= position.start_byte
                    && (position.start_byte < symbol.scope.end_byte
                        || position.start_byte == symbol.scope.end_byte
                            && matches!(
                                symbol.kind,
                                "function"
                                    | "type"
                                    | "class"
                                    | "enum"
                                    | "error"
                                    | "constant"
                                    | "import"
                            ))
                    && symbol.visible_from <= position.start_byte
            })
            .cloned()
            .collect();
        // Inner bindings win over a same-named declaration in an outer scope.
        symbols.sort_by_key(|symbol| {
            (
                symbol.scope.end_byte - symbol.scope.start_byte,
                std::cmp::Reverse(symbol.visible_from),
            )
        });
        let mut unique = BTreeMap::new();
        for symbol in symbols {
            unique.entry(symbol.name.clone()).or_insert(symbol);
        }
        unique.into_values().collect()
    }
    pub fn add_import_symbol(&mut self, name: String, target: &str, scope: SourceRange) {
        if let Some(definition) = self.targets.get(target) {
            self.symbols.push(EditorSymbol {
                name,
                kind: "import",
                definition: *definition,
                scope,
                visible_from: scope.start_byte,
            });
        }
    }
}

fn contains(outer: SourceRange, inner: SourceRange) -> bool {
    outer.file_id == inner.file_id
        && outer.start_byte <= inner.start_byte
        && inner.start_byte < outer.end_byte
}
fn base_name(name: &str) -> &str {
    let name = crate::methods::parts(name).map_or(name, |(_, member)| member);
    name.split('<')
        .next()
        .unwrap_or(name)
        .rsplit("::")
        .next()
        .unwrap_or(name)
        .rsplit('.')
        .next()
        .unwrap_or(name)
}
fn flatten(tokens: &[Token], output: &mut Vec<(String, SourceRange)>) {
    for token in tokens {
        if let TokenKind::Identifier(name) = &token.kind {
            output.push((name.clone(), token.range));
        }
        if let TokenKind::FormattedString(parts) = &token.kind {
            for part in parts {
                if let FormattedPart::Expression { tokens, .. } = part {
                    flatten(tokens, output);
                }
            }
        }
    }
}
struct Names(BTreeMap<u32, Vec<(String, SourceRange)>>);
impl Names {
    fn new(sources: &BTreeMap<PathBuf, SourceFile>) -> Self {
        let mut names = BTreeMap::new();
        for source in sources.values() {
            let mut identifiers = Vec::new();
            if let Ok(tokens) = lex(source) {
                flatten(&tokens, &mut identifiers);
            }
            names.insert(source.id().0, identifiers);
        }
        Self(names)
    }
    fn span(&self, range: SourceRange, name: &str) -> Option<SourceRange> {
        self.0
            .get(&range.file_id.0)?
            .iter()
            .find(|(identifier, token)| {
                identifier == base_name(name)
                    && token.start_byte >= range.start_byte
                    && token.end_byte <= range.end_byte
                    // A method named `private` can also have the contextual modifier.
                    && !(identifier == "private"
                        && crate::methods::parts(name).is_some()
                        && token.start_byte == range.start_byte)
            })
            .map(|(_, range)| *range)
    }
}

pub fn build_editor_index(
    sources: &BTreeMap<PathBuf, SourceFile>,
    modules: &[(SourceFile, Module)],
    linked: &[&Module],
) -> EditorIndex {
    let names = Names::new(sources);
    let mut index = EditorIndex::default();
    for (source, module) in modules {
        let file_scope = SourceRange::new(
            source.id(),
            source.original_offset(0),
            source.original_offset(source.text().len()),
        );
        let mut builder = Lexical {
            names: &names,
            index: &mut index,
            env: vec![HashMap::new()],
            scopes: vec![file_scope],
            pending: HashMap::new(),
        };
        for declaration in &module.declarations {
            if let Some(span) = names.span(declaration.range(), declaration.name()) {
                if let Declaration::Function(function) = declaration
                    && function.is_private
                {
                    for member in &module.declarations {
                        if let Declaration::Function(member) = member
                            && function.lexical_owner.is_some()
                            && function.lexical_owner == member.lexical_owner
                        {
                            builder.index.symbols.push(EditorSymbol {
                                name: function.name.clone(),
                                kind: "function",
                                definition: span,
                                scope: member.range,
                                visible_from: member.range.start_byte,
                            });
                        }
                    }
                } else {
                    builder.define(
                        declaration.name().to_owned(),
                        declaration_kind(declaration),
                        span,
                        file_scope,
                        file_scope.start_byte,
                    );
                }
            }
            let parameters = match declaration {
                Declaration::Record(record) => &record.type_parameters,
                Declaration::Enum(value) => &value.type_parameters,
                Declaration::Error(value) => &value.type_parameters,
                Declaration::TypeAlias(alias) => &alias.type_parameters,
                _ => continue,
            };
            for parameter in parameters {
                if let Some(span) = names.span(parameter.range, &parameter.name) {
                    builder.index.symbols.push(EditorSymbol {
                        name: parameter.name.clone(),
                        kind: "typeParameter",
                        definition: span,
                        scope: declaration.range(),
                        visible_from: declaration.range().start_byte,
                    });
                }
            }
        }
        ast_walk::module(&mut module.clone(), &mut builder);
    }
    for module in linked {
        for declaration in &module.declarations {
            if let Some(span) = names.span(declaration.range(), declaration.name()) {
                index.targets.insert(declaration.name().to_owned(), span);
            }
        }
        for block in &module.native_blocks {
            for member in &block.members {
                let (name, range) = match member {
                    NativeMember::Opaque { name, range } => (name.as_str(), *range),
                    NativeMember::Record(record) => (record.name.as_str(), record.range),
                    NativeMember::Function(function) => (function.name.as_str(), function.range),
                };
                if let Some(span) = names.span(range, name) {
                    index.targets.insert(name.to_owned(), span);
                }
            }
        }
    }
    let targets = index.targets.clone();
    for module in linked {
        ast_walk::module(
            &mut (*module).clone(),
            &mut Resolved {
                names: &names,
                index: &mut index,
                targets: &targets,
            },
        );
    }
    index
}
fn declaration_kind(declaration: &Declaration) -> &'static str {
    match declaration {
        Declaration::Function(_) => "function",
        Declaration::Record(_) => "class",
        Declaration::Enum(_) => "enum",
        Declaration::Error(_) => "error",
        Declaration::Constant(_) => "constant",
        Declaration::TypeAlias(_) => "type",
    }
}

struct Lexical<'a> {
    names: &'a Names,
    index: &'a mut EditorIndex,
    env: Vec<HashMap<String, SourceRange>>,
    scopes: Vec<SourceRange>,
    pending: HashMap<SourceRange, Vec<(String, SourceRange)>>,
}
impl Lexical<'_> {
    fn define(
        &mut self,
        name: String,
        kind: &'static str,
        definition: SourceRange,
        scope: SourceRange,
        visible_from: u32,
    ) {
        self.env
            .last_mut()
            .expect("lexical scope")
            .insert(name.clone(), definition);
        self.index.symbols.push(EditorSymbol {
            name,
            kind,
            definition,
            scope,
            visible_from,
        });
    }
    fn reference(&mut self, name: &str, range: SourceRange) {
        let key = name.split('.').next().unwrap_or(name);
        let definition = self
            .env
            .iter()
            .rev()
            .find_map(|scope| scope.get(key))
            .copied()
            .or_else(|| {
                self.index
                    .symbols
                    .iter()
                    .filter(|symbol| {
                        symbol.name == key
                            && contains(symbol.scope, range)
                            && symbol.visible_from <= range.start_byte
                    })
                    .min_by_key(|symbol| symbol.scope.end_byte - symbol.scope.start_byte)
                    .map(|symbol| symbol.definition)
            });
        if let (Some(definition), Some(span)) = (definition, self.names.span(range, key)) {
            self.index.references.push(EditorReference {
                range: span,
                definition,
            });
        }
    }
}
impl Visitor for Lexical<'_> {
    fn enter_function(&mut self, function: &FunctionDecl) {
        self.env.push(HashMap::new());
        self.scopes.push(function.range);
        for parameter in &function.parameters {
            if let Some(span) = self.names.span(parameter.range, &parameter.name) {
                self.define(
                    parameter.name.clone(),
                    "parameter",
                    span,
                    function.range,
                    function.range.start_byte,
                );
            }
        }
        for parameter in &function.type_parameters {
            if let Some(span) = self.names.span(parameter.range, &parameter.name) {
                self.define(
                    parameter.name.clone(),
                    "typeParameter",
                    span,
                    function.range,
                    function.range.start_byte,
                );
            }
        }
    }
    fn leave_function(&mut self, _: &FunctionDecl) {
        self.env.pop();
        self.scopes.pop();
    }
    fn enter_block(&mut self, block: &Block) {
        self.env.push(HashMap::new());
        self.scopes.push(block.range);
        for (name, range) in self.pending.remove(&block.range).unwrap_or_default() {
            if let Some(span) = self.names.span(range, &name) {
                self.define(name, "parameter", span, block.range, block.range.start_byte);
            }
        }
    }
    fn leave_block(&mut self, _: &Block) {
        self.env.pop();
        self.scopes.pop();
    }
    fn enter_statement(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::For {
                binding,
                binding_range,
                body,
                ..
            } if binding != "_" => {
                self.pending
                    .insert(body.range, vec![(binding.clone(), *binding_range)]);
            }
            StatementKind::TryErrors { handlers, .. } => {
                for handler in handlers {
                    self.pending.insert(
                        handler.body.range,
                        handler
                            .pattern
                            .bindings()
                            .map(|binding| (binding.name.clone(), binding.range))
                            .collect(),
                    );
                }
            }
            StatementKind::TryCatch {
                binding,
                binding_range,
                handler,
                ..
            } => {
                self.pending
                    .insert(handler.range, vec![(binding.clone(), *binding_range)]);
            }
            StatementKind::TryAll {
                handler: Some(handler),
                ..
            } => {
                self.pending.insert(
                    handler.body.range,
                    vec![(handler.binding.clone(), handler.binding_range)],
                );
            }
            StatementKind::Switch { cases, .. } => {
                for case in cases {
                    self.pending.insert(
                        case.body.range,
                        case.bindings
                            .iter()
                            .map(|name| (name.clone(), case.range))
                            .collect(),
                    );
                }
            }
            _ => {}
        }
    }
    fn leave_statement(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::Local { name, .. } => {
                if let Some(span) = self.names.span(statement.range, name) {
                    self.define(
                        name.clone(),
                        "variable",
                        span,
                        *self.scopes.last().expect("block scope"),
                        statement.range.end_byte,
                    );
                }
            }
            StatementKind::Assign { name, .. } => self.reference(name, statement.range),
            _ => {}
        }
    }
    fn expression(&mut self, expression: &mut Expression) {
        match &expression.kind {
            ExpressionKind::Name(path)
            | ExpressionKind::Call { target: path, .. }
            | ExpressionKind::Spawn { target: path, .. }
            | ExpressionKind::GenericCall { target: path, .. }
            | ExpressionKind::GenericReference { target: path, .. }
            | ExpressionKind::Bind { target: path, .. } => {
                self.reference(&path.join("."), expression.range)
            }
            _ => {}
        }
    }
    fn ty(&mut self, ty: &mut TypeRef) {
        if let TypeRefKind::Named(path) | TypeRefKind::Generic { path, .. } = &ty.kind {
            self.reference(&path.join("."), ty.range);
        }
    }
}

struct Resolved<'a> {
    names: &'a Names,
    index: &'a mut EditorIndex,
    targets: &'a BTreeMap<String, SourceRange>,
}
impl Resolved<'_> {
    fn reference(&mut self, path: &[String], range: SourceRange) {
        let name = path.join(".");
        if let (Some(definition), Some(span)) = (
            self.targets
                .get(&name)
                .or_else(|| self.targets.get(&crate::methods::template_name(&name))),
            self.names.span(range, &name),
        ) {
            // A lexical variable/function alias already resolved at this token
            // must not be replaced by a same-named global target.
            if !self
                .index
                .references
                .iter()
                .any(|reference| reference.range == span)
            {
                self.index.references.push(EditorReference {
                    range: span,
                    definition: *definition,
                });
            }
        }
    }
}
impl Visitor for Resolved<'_> {
    fn expression(&mut self, expression: &mut Expression) {
        match &expression.kind {
            ExpressionKind::GenericCall {
                target,
                variant: Some(member),
                ..
            }
            | ExpressionKind::GenericReference {
                target,
                member: Some(member),
                ..
            } => {
                self.reference(target, expression.range);
                let mut path = target.clone();
                path.push(member.clone());
                self.reference(&path, expression.range);
            }
            ExpressionKind::Name(path)
            | ExpressionKind::Call { target: path, .. }
            | ExpressionKind::Spawn { target: path, .. }
            | ExpressionKind::GenericCall { target: path, .. }
            | ExpressionKind::GenericReference { target: path, .. }
            | ExpressionKind::Bind { target: path, .. } => self.reference(path, expression.range),
            _ => {}
        }
    }
    fn ty(&mut self, ty: &mut TypeRef) {
        if let TypeRefKind::Named(path) | TypeRefKind::Generic { path, .. } = &ty.kind {
            self.reference(path, ty.range);
        }
    }
}

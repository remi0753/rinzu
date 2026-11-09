//! Checked types and ordinary Error bounds attached to source editor tokens.
use super::*;
use crate::{CheckedFunction, CheckedModule, FunctionReferenceType, ResolvedSourceModule, Type};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Bound {
    None,
    Finite { types: Vec<String> },
    Open,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct Effect {
    phase: &'static str,
    basis: &'static str,
    bound: Bound,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorHover {
    pub kind: &'static str,
    pub resolution: &'static str,
    #[serde(skip)]
    pub target: SourceRange,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operand_type: Option<String>,
    #[serde(skip)]
    pub definition: Option<SourceRange>,
    #[serde(skip)]
    pub contract: Option<SourceRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    effects: Vec<Effect>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    varies_by_specialization: bool,
}

impl EditorHover {
    fn new(kind: &'static str, target: SourceRange, label: String, checked: bool) -> Self {
        Self {
            kind,
            resolution: if checked { "checked" } else { "syntax" },
            target,
            label,
            signature: None,
            value_type: None,
            operand_type: None,
            definition: None,
            contract: None,
            public_name: None,
            effects: Vec::new(),
            varies_by_specialization: false,
        }
    }

    fn merge(&mut self, other: Self) {
        if *self == other {
            return;
        }
        self.varies_by_specialization = true;
        macro_rules! common {
            ($($field:ident),+) => {
                $(if self.$field != other.$field { self.$field = None; })+
            };
        }
        common!(
            signature,
            value_type,
            operand_type,
            definition,
            contract,
            public_name
        );
        self.effects.retain(|effect| other.effects.contains(effect));
    }
}

#[derive(Default)]
pub struct EditorHoverIndex(BTreeMap<u32, EditorHover>);

impl EditorHoverIndex {
    pub fn at(&self, point: SourceRange) -> Option<&EditorHover> {
        let (_, hover) = self.0.range(..=point.start_byte).next_back()?;
        contains(hover.target, point).then_some(hover)
    }

    pub fn entries(&self) -> impl Iterator<Item = &EditorHover> {
        self.0.values()
    }

    pub fn retained_bytes(&self) -> usize {
        self.entries()
            .map(|hover| {
                std::mem::size_of::<EditorHover>()
                    + hover.label.len()
                    + [
                        &hover.signature,
                        &hover.value_type,
                        &hover.operand_type,
                        &hover.public_name,
                    ]
                    .into_iter()
                    .filter_map(|value| value.as_ref())
                    .map(String::len)
                    .sum::<usize>()
                    + hover
                        .effects
                        .iter()
                        .map(|effect| {
                            std::mem::size_of::<Effect>()
                                + match &effect.bound {
                                    Bound::Finite { types } => types
                                        .iter()
                                        .map(|ty| ty.len() + std::mem::size_of::<String>())
                                        .sum(),
                                    _ => 0,
                                }
                        })
                        .sum::<usize>()
            })
            .sum()
    }

    fn insert(&mut self, hover: EditorHover) {
        match self.0.entry(hover.target.start_byte) {
            std::collections::btree_map::Entry::Occupied(mut entry) => entry.get_mut().merge(hover),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(hover);
            }
        }
    }
}

fn physical(range: SourceRange) -> SourceRange {
    SourceRange {
        instance: 0,
        ..range
    }
}

fn function_symbol(index: &EditorIndex, name: &str) -> Option<SourceRange> {
    index
        .targets
        .get(name)
        .or_else(|| index.targets.get(&crate::methods::template_name(name)))
        .copied()
}

fn snippet(source: &SourceFile, range: SourceRange) -> &str {
    let bom = source.original_offset(0);
    source
        .text()
        .get(
            (range.start_byte as usize).saturating_sub(bom)
                ..(range.end_byte as usize).saturating_sub(bom),
        )
        .unwrap_or("")
        .trim()
}

#[derive(Default)]
struct NamesInFile<'a> {
    visible: BTreeMap<String, String>,
    public: BTreeMap<String, String>,
    checked: Option<&'a CheckedModule>,
}

impl<'a> NamesInFile<'a> {
    fn new(
        graph: &[ResolvedSourceModule],
        file: crate::FileId,
        checked: Option<&'a CheckedModule>,
    ) -> Self {
        let mut names = Self {
            checked,
            ..Self::default()
        };
        for module in graph {
            for (public, id) in &module.exports {
                names.public.entry(id.clone()).or_insert_with(|| {
                    if id.contains("::") {
                        id.replace("::", ".")
                    } else {
                        format!("{}.{public}", module.key)
                    }
                });
            }
            if module.file_id == file {
                for import in &module.imports {
                    for (public, id) in &import.public_symbols {
                        let display = format!("{}.{public}", import.qualifier);
                        let best = names
                            .visible
                            .entry(id.clone())
                            .or_insert_with(|| display.clone());
                        if (display.len(), &display) < (best.len(), &*best) {
                            *best = display;
                        }
                    }
                }
            }
        }
        names
    }

    fn identity(&self, id: &str) -> String {
        if let Some(name) = self.visible.get(id) {
            return name.clone();
        }
        if let Some((head, args)) = id.split_once('<')
            && let Some(args) = args.strip_suffix('>')
        {
            return format!("{}<{}>", self.identity(head), self.type_arguments(args));
        }
        if let Some(public) = self.public.get(id) {
            return public.clone();
        }
        id.trim_start_matches("$rinzu_").replace("::", ".")
    }

    fn type_arguments(&self, value: &str) -> String {
        // The type's identity already encodes its generic arguments. Rewrite
        // qualified names within that spelling without splitting nested types.
        let mut text = String::new();
        let mut token = String::new();
        for ch in value.chars() {
            if ch.is_alphanumeric() || matches!(ch, '_' | '$' | ':' | '.') {
                token.push(ch);
            } else {
                if !token.is_empty() {
                    text.push_str(&self.identity(&std::mem::take(&mut token)));
                }
                text.push(ch);
            }
        }
        text.push_str(&self.identity(&token));
        text
    }

    fn bound(&self, types: Option<impl IntoIterator<Item = String>>) -> Bound {
        let Some(types) = types else {
            return Bound::Unknown;
        };
        let mut types: Vec<_> = types.into_iter().collect();
        if types.iter().any(|ty| ty == "Error") {
            return Bound::Open;
        }
        if types.is_empty() {
            return Bound::None;
        }
        types.sort();
        types.dedup();
        types.iter_mut().for_each(|ty| *ty = self.identity(ty));
        types.sort();
        Bound::Finite { types }
    }

    fn ty(&self, ty: &Type) -> String {
        match ty {
            Type::List(inner) => format!("List<{}>", self.ty(inner)),
            Type::Map(key, value) => format!("Map<{}, {}>", self.ty(key), self.ty(value)),
            Type::Future(inner) => format!("Future<{}>", self.ty(inner)),
            Type::FunctionReference(reference) => self.reference(reference, ""),
            Type::Named(name) if name.starts_with("$rinzu_record_") => {
                if let Some(schema) = self
                    .checked
                    .and_then(|checked| checked.source_types.get(name))
                {
                    let fields = schema
                        .fields
                        .iter()
                        .map(|(name, ty)| format!("{name}: {}", self.ty(ty)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{{{fields}}}")
                } else {
                    self.identity(name)
                }
            }
            Type::Named(name) | Type::Native { name, .. } => self.identity(name),
            _ => ty.to_string(),
        }
    }

    fn reference(&self, reference: &FunctionReferenceType, name: &str) -> String {
        let parameters = reference
            .parameters
            .iter()
            .map(|parameter| self.ty(parameter))
            .collect::<Vec<_>>()
            .join(", ");
        let modifier = if reference.supplant { "supplant " } else { "" };
        let contract = reference.error_bound.as_ref().map(|types| {
            types
                .iter()
                .map(|name| self.identity(name))
                .collect::<Vec<_>>()
        });
        format!(
            "{modifier}fn {name}({parameters}){} -> {}",
            error_bound_spelling(contract.as_deref()),
            self.ty(&reference.result)
        )
    }

    fn annotation(&self, ty: &TypeRef) -> String {
        match &ty.kind {
            TypeRefKind::Named(path) => self.identity(&path.join(".")),
            TypeRefKind::Generic { path, arguments } => format!(
                "{}<{}>",
                self.identity(&path.join(".")),
                arguments
                    .iter()
                    .map(|arg| self.annotation(arg))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeRefKind::List(inner) => format!("List<{}>", self.annotation(inner)),
            TypeRefKind::Map(key, value) => {
                format!("Map<{}, {}>", self.annotation(key), self.annotation(value))
            }
            TypeRefKind::Future(inner) => format!("Future<{}>", self.annotation(inner)),
            TypeRefKind::Record(fields) => format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|field| { format!("{}: {}", field.name, self.annotation(&field.ty)) })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeRefKind::Function {
                supplant,
                parameters,
                result,
                error_contract,
            } => {
                let parameters = parameters
                    .iter()
                    .map(|parameter| self.annotation(parameter))
                    .collect::<Vec<_>>()
                    .join(", ");
                let modifier = if *supplant { "supplant " } else { "" };
                let contract = error_contract
                    .as_ref()
                    .map_or_else(String::new, |c| c.spelling(|ty| self.annotation(ty)));
                format!(
                    "{modifier}fn ({parameters}){contract} -> {}",
                    self.annotation(result)
                )
            }
            _ => crate::interfaces::spelling(ty),
        }
    }
}

#[derive(Default)]
struct ResolvedExpressions {
    calls: HashMap<SourceRange, (Vec<String>, Vec<SourceRange>)>,
}
impl Visitor for ResolvedExpressions {
    fn expression(&mut self, expression: &mut Expression) {
        if let ExpressionKind::Call { target, arguments }
        | ExpressionKind::Spawn { target, arguments } = &expression.kind
        {
            let arguments = match arguments {
                Arguments::Positional(values) => values.iter().map(|value| value.range).collect(),
                Arguments::Named(values) => {
                    values.iter().map(|(_, value, _)| value.range).collect()
                }
            };
            self.calls
                .insert(expression.range, (target.clone(), arguments));
        }
    }
}

struct Declarations<'a> {
    functions: HashMap<SourceRange, &'a FunctionDecl>,
    aliases: HashMap<SourceRange, &'a TypeAliasDecl>,
    aliases_by_id: HashMap<&'a str, &'a TypeAliasDecl>,
}

impl<'a> Declarations<'a> {
    fn new(names: &Names, modules: impl Iterator<Item = &'a Module>) -> Self {
        let mut result = Self {
            functions: HashMap::new(),
            aliases: HashMap::new(),
            aliases_by_id: HashMap::new(),
        };
        for module in modules {
            for declaration in &module.declarations {
                if let Declaration::Interface(interface) = declaration {
                    for function in &interface.methods {
                        if let Some(span) = names.span(function.range, &function.name) {
                            result.functions.insert(span, function);
                        }
                    }
                }
                let Some(span) = names.span(declaration.range(), declaration.name()) else {
                    continue;
                };
                match declaration {
                    Declaration::Function(function) => {
                        if function.name.contains("::") || !result.functions.contains_key(&span) {
                            result.functions.insert(span, function);
                        }
                    }
                    Declaration::TypeAlias(alias) => {
                        result.aliases.insert(span, alias);
                        result.aliases_by_id.insert(&alias.name, alias);
                    }
                    _ => {}
                }
            }
        }
        result
    }
}

pub fn build_hover_index(
    path: &std::path::Path,
    sources: &BTreeMap<PathBuf, SourceFile>,
    modules: &[(SourceFile, Module)],
    linked: &[&Module],
    navigation: &EditorIndex,
    checked: Option<&CheckedModule>,
    graph: &[ResolvedSourceModule],
) -> EditorHoverIndex {
    let Some((source, module)) = modules.iter().find(|(source, _)| source.path() == path) else {
        return EditorHoverIndex::default();
    };
    let names = Names::new(sources);
    // Linked syntax retains declared type alias spellings. Put it after the
    // parsed declarations so canonical alias identities win when available.
    let declarations = Declarations::new(
        &names,
        modules
            .iter()
            .map(|(_, module)| module)
            .chain(linked.iter().copied()),
    );
    let mut builder = Builder {
        source,
        sources,
        names: &names,
        display: NamesInFile::new(graph, source.id(), checked),
        navigation,
        checked,
        declarations,
        expressions: ResolvedExpressions::default(),
        types: HashMap::new(),
        functions: HashMap::new(),
        function_declarations: HashMap::new(),
        definitions: navigation
            .symbols
            .iter()
            .map(|symbol| (symbol.definition, symbol.definition))
            .chain(
                navigation
                    .references
                    .iter()
                    .map(|reference| (reference.range, reference.definition)),
            )
            .collect(),
        binding_kinds: navigation
            .symbols
            .iter()
            .filter(|symbol| symbol.kind != "import")
            .map(|symbol| (symbol.definition, symbol.kind))
            .collect(),
        binding_annotations: HashMap::new(),
        index: EditorHoverIndex::default(),
    };
    if let Some(checked) = checked {
        for (&range, ty) in &checked.expression_types {
            if range.file_id == source.id() {
                builder
                    .types
                    .entry(physical(range))
                    .or_default()
                    .push((range, ty));
            }
        }
        for function in &checked.functions {
            builder
                .functions
                .insert(function.declaration.name.as_str(), function);
            if function.declaration.range.file_id == source.id() {
                builder
                    .function_declarations
                    .entry(physical(function.declaration.range))
                    .or_default()
                    .push(function);
            }
        }
        for declaration in &checked.syntax.declarations {
            if declaration.range().file_id == source.id() {
                ast_walk::declaration(&mut declaration.clone(), &mut builder.expressions);
            }
        }
    }
    let mut syntax = module.clone();
    ast_walk::module(
        &mut syntax,
        &mut BindingAnnotations {
            names: &names,
            annotations: &mut builder.binding_annotations,
        },
    );
    for alias in syntax.declarations.iter().filter_map(|decl| {
        if let Declaration::TypeAlias(alias) = decl {
            Some(alias)
        } else {
            None
        }
    }) {
        if let Some(span) = names.span(alias.range, &alias.name) {
            builder.alias(span, alias.name.clone(), span, None);
        }
    }
    ast_walk::module(&mut syntax, &mut builder);
    builder.index
}

struct BindingAnnotations<'a> {
    names: &'a Names,
    annotations: &'a mut HashMap<SourceRange, TypeRef>,
}
impl Visitor for BindingAnnotations<'_> {
    fn enter_function(&mut self, function: &FunctionDecl) {
        for parameter in &function.parameters {
            if let Some(span) = self.names.span(parameter.range, &parameter.name) {
                self.annotations.insert(span, parameter.ty.clone());
            }
        }
    }
    fn enter_statement(&mut self, statement: &Statement) {
        if let StatementKind::Local {
            name,
            annotation: Some(ty),
            ..
        } = &statement.kind
            && let Some(span) = self.names.span(statement.range, name)
        {
            self.annotations.insert(span, ty.clone());
        }
    }
}

struct Builder<'a> {
    source: &'a SourceFile,
    sources: &'a BTreeMap<PathBuf, SourceFile>,
    names: &'a Names,
    display: NamesInFile<'a>,
    navigation: &'a EditorIndex,
    checked: Option<&'a CheckedModule>,
    declarations: Declarations<'a>,
    expressions: ResolvedExpressions,
    types: HashMap<SourceRange, Vec<(SourceRange, &'a Type)>>,
    functions: HashMap<&'a str, &'a CheckedFunction>,
    function_declarations: HashMap<SourceRange, Vec<&'a CheckedFunction>>,
    definitions: HashMap<SourceRange, SourceRange>,
    binding_kinds: HashMap<SourceRange, &'static str>,
    binding_annotations: HashMap<SourceRange, TypeRef>,
    index: EditorHoverIndex,
}

impl<'a> Builder<'a> {
    fn syntax(&self, range: SourceRange) -> Option<String> {
        self.sources
            .values()
            .find(|source| source.id() == range.file_id)
            .map(|source| snippet(source, range).to_owned())
            .filter(|text| !text.is_empty())
    }

    fn typed(&self, range: SourceRange) -> &[(SourceRange, &'a Type)] {
        self.types.get(&physical(range)).map_or(&[], Vec::as_slice)
    }

    fn signature_type(&self, source: Option<&TypeRef>, resolved: &Type) -> String {
        let Some(source) = source else {
            return self.display.ty(resolved);
        };
        match (&source.kind, resolved) {
            (TypeRefKind::Named(path) | TypeRefKind::Generic { path, .. }, _)
                if self
                    .declarations
                    .aliases_by_id
                    .contains_key(path.join(".").as_str()) =>
            {
                self.display.annotation(source)
            }
            (TypeRefKind::List(item), Type::List(ty)) => {
                format!("List<{}>", self.signature_type(Some(item), ty))
            }
            (TypeRefKind::Map(key, value), Type::Map(key_type, value_type)) => format!(
                "Map<{}, {}>",
                self.signature_type(Some(key), key_type),
                self.signature_type(Some(value), value_type)
            ),
            (TypeRefKind::Future(value), Type::Future(ty)) => {
                format!("Future<{}>", self.signature_type(Some(value), ty))
            }
            _ => self.display.ty(resolved),
        }
    }

    fn function(
        &self,
        target: SourceRange,
        label: String,
        function: &CheckedFunction,
    ) -> EditorHover {
        let decl = &function.declaration;
        let mut hover = EditorHover::new("function", target, label, true);
        hover.definition = function_symbol(self.navigation, &decl.name)
            .or_else(|| self.names.span(decl.range, &decl.name))
            .map(physical);
        hover.contract = decl
            .error_contract
            .as_ref()
            .map(|contract| physical(contract.range));
        hover.public_name = self
            .display
            .public
            .get(&crate::methods::template_name(&decl.name))
            .cloned();
        let original = hover
            .definition
            .and_then(|span| self.declarations.functions.get(&span).copied())
            .filter(|function| function.type_parameters.is_empty());
        let params = decl
            .parameters
            .iter()
            .zip(&function.parameter_types)
            .enumerate()
            .map(|(index, (p, ty))| {
                format!(
                    "{}: {}",
                    p.name,
                    self.signature_type(
                        original
                            .and_then(|f| f.parameters.get(index))
                            .map(|p| &p.ty),
                        ty
                    )
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let contract = decl.error_contract.as_ref().map_or_else(String::new, |c| {
            c.spelling(|ty| self.display.annotation(ty))
        });
        let result = self.signature_type(original.map(|f| &f.result), &function.result_type);
        let modifier = if decl.is_async { "async " } else { "" };
        hover.signature = Some(format!(
            "{modifier}fn {}({params}){contract} -> {result}",
            hover.label
        ));
        if decl.is_async {
            hover.effects.push(Effect {
                phase: "call",
                basis: "language",
                bound: Bound::None,
            });
        }
        hover.effects.push(Effect {
            phase: if decl.is_async { "future" } else { "call" },
            basis: if decl.error_contract.is_some() {
                "declared"
            } else {
                "inferred"
            },
            bound: self
                .display
                .bound(Some(function.error_types.iter().cloned())),
        });
        hover
    }

    fn syntax_function(
        &self,
        target: SourceRange,
        label: String,
        definition: SourceRange,
        function: &FunctionDecl,
    ) -> Option<EditorHover> {
        let mut hover = EditorHover::new("function", target, label, false);
        hover.definition = Some(definition);
        hover.contract = function.error_contract.as_ref().map(|c| physical(c.range));
        hover.signature = self.syntax(SourceRange {
            end_byte: function.body.range.start_byte,
            ..function.range
        });
        hover.signature.is_some().then_some(hover)
    }

    fn alias(
        &mut self,
        target: SourceRange,
        label: String,
        definition: SourceRange,
        use_range: Option<SourceRange>,
    ) {
        let Some(alias) = self.declarations.aliases.get(&definition) else {
            return;
        };
        let mut hover = EditorHover::new("type", target, label, self.checked.is_some());
        hover.definition = Some(definition);
        if self.checked.is_some() {
            let parameters = alias
                .type_parameters
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>();
            let generics = if parameters.is_empty() {
                String::new()
            } else {
                format!("<{}>", parameters.join(", "))
            };
            hover.signature = Some(format!(
                "type {}{generics} = {}",
                hover.label,
                self.display.annotation(&alias.ty)
            ));
            hover.value_type = use_range
                .and_then(|range| self.checked?.type_references.get(&range))
                .map(|ty| self.display.ty(ty));
            if let TypeRefKind::Function {
                error_contract: Some(contract),
                ..
            } = &alias.ty.kind
            {
                hover.contract = Some(physical(contract.range));
                hover.effects.push(Effect {
                    phase: "call",
                    basis: "declared",
                    bound: self
                        .display
                        .bound(Some(contract.types.iter().map(crate::interfaces::spelling))),
                });
            }
            hover.public_name = self.display.public.get(&alias.name).cloned();
        } else {
            hover.signature = self.syntax(alias.range);
        }
        self.index.insert(hover);
    }

    fn callback_annotation<'b>(
        &'b self,
        mut ty: &'b TypeRef,
    ) -> Option<(&'b TypeRef, &'b ErrorContract)> {
        for _ in 0..=self.declarations.aliases.len() {
            let path = match &ty.kind {
                TypeRefKind::Function {
                    error_contract: Some(contract),
                    ..
                } => return Some((ty, contract)),
                TypeRefKind::Named(path) | TypeRefKind::Generic { path, .. } => path,
                _ => return None,
            };
            let alias = self
                .declarations
                .aliases_by_id
                .get(path.join(".").as_str())
                .copied()
                .or_else(|| {
                    let token = self.name_span(ty.range, path, None)?;
                    let definition = self.definitions.get(&token)?;
                    self.declarations.aliases.get(definition).copied()
                })?;
            if !alias.type_parameters.is_empty() {
                return None;
            }
            ty = &alias.ty;
        }
        None
    }

    fn declared_effect(&self, phase: &'static str, contract: &ErrorContract) -> Effect {
        Effect {
            phase,
            basis: "declared",
            bound: self
                .display
                .bound(Some(contract.types.iter().map(crate::interfaces::spelling))),
        }
    }

    fn binding(
        &self,
        span: SourceRange,
        label: String,
        definition: SourceRange,
        ty: Option<&Type>,
        kind: &'static str,
    ) -> Option<EditorHover> {
        let annotation = self.binding_annotations.get(&definition);
        let mut hover = EditorHover::new(kind, span, label, ty.is_some());
        hover.definition = Some(definition);
        if let Some(ty) = ty {
            let callable = annotation.and_then(|annotation| self.callback_annotation(annotation));
            let rendered = self.display.ty(ty);
            let annotation = annotation.map(|annotation| self.display.annotation(annotation));
            let displayed = annotation.as_ref().unwrap_or(&rendered);
            hover.signature = Some(format!("{}: {displayed}", hover.label));
            hover.value_type = Some(displayed.clone());
            if let Some((declaration, contract)) = callable {
                let expanded = self.display.annotation(declaration);
                if *displayed != expanded {
                    hover.signature = Some(format!("{}: {displayed}\n{expanded}", hover.label));
                }
                hover.contract = Some(physical(contract.range));
                hover.effects.push(self.declared_effect("call", contract));
            } else if let Type::FunctionReference(reference) = ty {
                if *displayed != rendered {
                    hover.signature = Some(format!("{}: {displayed}\n{rendered}", hover.label));
                }
                if let Some(bound) = &reference.error_bound {
                    hover.effects.push(Effect {
                        phase: "call",
                        basis: "declared",
                        bound: self.display.bound(Some(bound.iter().cloned())),
                    });
                }
            } else if ty == &Type::Error
                || matches!(ty, Type::Named(name) if self.checked.is_some_and(|c| c.source_types.get(name).is_some_and(|s| s.kind == "error")))
            {
                // In a narrowed branch, use the type of the reference.
                hover.value_type = Some(rendered.clone());
                hover.signature = Some(format!("{}: {rendered}", hover.label));
            }
        } else {
            let annotation = annotation?;
            hover.signature = Some(format!(
                "{}: {}",
                hover.label,
                self.syntax(annotation.range)?
            ));
        }
        Some(hover)
    }

    fn name_span(
        &self,
        range: SourceRange,
        path: &[String],
        member: Option<&str>,
    ) -> Option<SourceRange> {
        if let Some(member) = member {
            return self.names.span(range, member);
        }
        let identifiers = self.names.0.get(&range.file_id.0)?;
        let start = identifiers.partition_point(|(_, span)| span.start_byte < range.start_byte);
        let candidates = identifiers.get(start..start.checked_add(path.len())?)?;
        if candidates
            .iter()
            .zip(path)
            .all(|((name, token), part)| name == part && token.end_byte <= range.end_byte)
        {
            return candidates.last().map(|(_, span)| *span);
        }
        self.names.span(range, path.last()?)
    }

    fn expr(
        &self,
        span: SourceRange,
        label: &str,
        range: SourceRange,
        ty: &Type,
        call: bool,
    ) -> Option<EditorHover> {
        let checked = self.checked?;
        let target = self
            .expressions
            .calls
            .get(&range)
            .map(|(path, _)| path.join("."))
            .or_else(|| checked.static_references.get(&range).cloned());
        if call && let Some(indirect) = checked.indirect_signatures.get(&range) {
            let mut hover = EditorHover::new("call", span, label.into(), true);
            hover.definition = self.definitions.get(&span).copied();
            let annotation = hover
                .definition
                .and_then(|definition| self.binding_annotations.get(&definition))
                .and_then(|ty| self.callback_annotation(ty))
                .map(|(_, contract)| contract);
            hover.contract = indirect
                .contract
                .map(physical)
                .or_else(|| annotation.map(|contract| physical(contract.range)));
            hover.value_type = Some(self.display.ty(ty));
            let mut signature = indirect.signature.clone();
            if let Some(contract) = annotation {
                signature.error_bound = Some(
                    contract
                        .types
                        .iter()
                        .map(crate::interfaces::spelling)
                        .collect(),
                );
            }
            hover.signature = Some(self.display.reference(&signature, label));
            let recorded = checked
                .editor_effects
                .as_ref()
                .and_then(|effects| effects.calls.get(&range));
            let targets = checked.indirect_calls.get(&range);
            let asynchronous = targets.is_some_and(|targets| {
                !targets.is_empty()
                    && targets.iter().all(|target| {
                        self.functions
                            .get(target.as_str())
                            .is_some_and(|function| function.declaration.is_async)
                    })
            });
            if asynchronous {
                hover.effects.push(Effect {
                    phase: "call",
                    basis: "language",
                    bound: Bound::None,
                });
            }
            hover.effects.push(Effect {
                phase: if asynchronous { "future" } else { "call" },
                basis: if signature.error_bound.is_some() {
                    "declared"
                } else {
                    "inferred"
                },
                bound: self
                    .display
                    .bound(signature.error_bound.clone().or_else(|| {
                        if asynchronous {
                            Some(
                                targets
                                    .into_iter()
                                    .flatten()
                                    .flat_map(|target| {
                                        self.functions[target.as_str()].error_types.iter().cloned()
                                    })
                                    .collect(),
                            )
                        } else {
                            recorded.map(|types| types.iter().cloned().collect())
                        }
                    })),
            });
            return Some(hover);
        }
        if let Some(function) = target.as_deref().and_then(|name| self.functions.get(name)) {
            let mut hover = self.function(span, label.into(), function);
            if call {
                hover.kind = "call";
            }
            hover.value_type = Some(self.display.ty(ty));
            hover.definition = hover
                .definition
                .or_else(|| self.definitions.get(&span).copied());
            return Some(hover);
        }
        if call {
            let recorded = checked
                .editor_effects
                .as_ref()
                .and_then(|effects| effects.calls.get(&range));
            if recorded.is_none()
                && !checked.data_operations.contains_key(&range)
                && !checked.source_native_calls.contains_key(&range)
            {
                return None;
            }
            let mut hover = EditorHover::new("call", span, label.into(), true);
            hover.definition = self.definitions.get(&span).copied();
            hover.value_type = Some(self.display.ty(ty));
            if let Some((_, arguments)) = self.expressions.calls.get(&range) {
                let parameters = arguments
                    .iter()
                    .filter_map(|range| checked.expression_type(*range))
                    .map(|ty| self.display.ty(ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                hover.signature = Some(format!(
                    "fn {label}({parameters}) -> {}",
                    self.display.ty(ty)
                ));
            }
            hover.effects.push(Effect {
                phase: "call",
                basis: "flow",
                bound: self
                    .display
                    .bound(recorded.map(|types| types.iter().cloned())),
            });
            return Some(hover);
        }
        let definition = self.definitions.get(&span).copied()?;
        if let Some(alias) = self.declarations.functions.get(&definition) {
            return self.syntax_function(span, label.into(), definition, alias);
        }
        let kind = *self.binding_kinds.get(&definition)?;
        if !matches!(kind, "variable" | "parameter" | "constant") {
            return None;
        }
        let ty = checked
            .error_narrowings
            .get(&range)
            .or_else(|| checked.error_upcasts.get(&range))
            .unwrap_or(ty);
        self.binding(span, label.into(), definition, Some(ty), kind)
    }
}

impl Visitor for Builder<'_> {
    fn enter_function(&mut self, function: &FunctionDecl) {
        let checked = self
            .function_declarations
            .get(&physical(function.range))
            .map_or_else(Vec::new, Vec::clone);
        if let Some(span) = self.names.span(function.range, &function.name) {
            if function.type_parameters.is_empty()
                && let [candidate] = checked.as_slice()
            {
                let hover = self.function(span, function.name.clone(), candidate);
                self.index.insert(hover);
            } else if let Some(mut hover) =
                self.syntax_function(span, function.name.clone(), span, function)
            {
                if self.checked.is_some() && !function.type_parameters.is_empty() {
                    hover.resolution = "checked";
                    if let Some(contract) = &function.error_contract {
                        hover.effects.push(Effect {
                            phase: if function.is_async { "future" } else { "call" },
                            basis: "declared",
                            bound: self.display.bound(Some(
                                contract.types.iter().map(crate::interfaces::spelling),
                            )),
                        });
                    }
                }
                self.index.insert(hover);
            }
        }
        for (index, parameter) in function.parameters.iter().enumerate() {
            if let Some(span) = self.names.span(parameter.range, &parameter.name) {
                let mut types = checked
                    .iter()
                    .filter_map(|function| function.parameter_types.get(index))
                    .peekable();
                if !function.type_parameters.is_empty() || types.peek().is_none() {
                    if let Some(mut hover) =
                        self.binding(span, parameter.name.clone(), span, None, "parameter")
                    {
                        if self.checked.is_some() {
                            hover.resolution = "checked";
                        }
                        self.index.insert(hover);
                    }
                } else {
                    for ty in types {
                        if let Some(hover) =
                            self.binding(span, parameter.name.clone(), span, Some(ty), "parameter")
                        {
                            self.index.insert(hover);
                        }
                    }
                }
            }
        }
    }

    fn enter_statement(&mut self, statement: &Statement) {
        if let StatementKind::Local {
            name,
            annotation,
            initializer,
            ..
        } = &statement.kind
            && let Some(span) = self.names.span(statement.range, name)
        {
            let variants = self.typed(initializer.range).to_vec();
            if variants.is_empty()
                && let Some(hover) = self.binding(span, name.clone(), span, None, "variable")
            {
                self.index.insert(hover);
            }
            for (range, ty) in variants {
                let ty = annotation
                    .as_ref()
                    .and_then(|ty| {
                        self.checked?.type_references.get(&SourceRange {
                            instance: range.instance,
                            ..ty.range
                        })
                    })
                    .unwrap_or(ty);
                if let Some(hover) = self.binding(span, name.clone(), span, Some(ty), "variable") {
                    self.index.insert(hover);
                }
            }
        }
    }

    fn ty(&mut self, ty: &mut TypeRef) {
        if let TypeRefKind::Named(path) | TypeRefKind::Generic { path, .. } = &ty.kind
            && let Some(span) = self.name_span(ty.range, path, None)
            && let Some(definition) = self.definitions.get(&span).copied()
        {
            self.alias(span, path.join("."), definition, Some(ty.range));
        }
    }

    fn expression(&mut self, expression: &mut Expression) {
        if let ExpressionKind::Await(operand) = &expression.kind {
            let Some(span) = self.names.span(expression.range, "await") else {
                return;
            };
            let operand_text = snippet(self.source, operand.range)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let label = if operand_text.chars().count() <= 96 {
                format!("await {operand_text}")
            } else {
                "await".into()
            };
            for (range, ty) in self.typed(expression.range).to_vec() {
                let checked = self.checked.unwrap();
                let mut hover = EditorHover::new("await", span, label.clone(), true);
                hover.value_type = Some(self.display.ty(ty));
                hover.operand_type = checked
                    .expression_type(SourceRange {
                        instance: range.instance,
                        ..operand.range
                    })
                    .map(|ty| self.display.ty(ty));
                let types = checked
                    .editor_effects
                    .as_ref()
                    .and_then(|effects| effects.awaits.get(&range));
                hover.effects.push(Effect {
                    phase: "await",
                    basis: "flow",
                    bound: self.display.bound(types.map(|types| types.iter().cloned())),
                });
                self.index.insert(hover);
            }
            return;
        }
        let (path, member, call) = match &expression.kind {
            ExpressionKind::Name(path) => (path, None, false),
            ExpressionKind::Call { target, .. } | ExpressionKind::Spawn { target, .. } => {
                (target, None, true)
            }
            ExpressionKind::GenericCall {
                target, variant, ..
            } => (target, variant.as_deref(), true),
            ExpressionKind::GenericReference { target, member, .. } => {
                (target, member.as_deref(), false)
            }
            _ => return,
        };
        let Some(span) = self.name_span(expression.range, path, member) else {
            return;
        };
        let mut label = path.join(".");
        if let Some(member) = member {
            label.push('.');
            label.push_str(member);
        }
        let variants = self.typed(expression.range).to_vec();
        if variants.is_empty()
            && let Some(definition) = self.definitions.get(&span).copied()
        {
            if let Some(function) = self.declarations.functions.get(&definition)
                && let Some(hover) = self.syntax_function(span, label.clone(), definition, function)
            {
                self.index.insert(hover);
            } else if let Some(hover) =
                self.binding(span, label.clone(), definition, None, "variable")
            {
                self.index.insert(hover);
            }
        }
        for (range, ty) in variants {
            if let Some(hover) = self.expr(span, &label, range, ty, call) {
                self.index.insert(hover);
            }
        }
    }
}

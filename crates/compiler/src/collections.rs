//! Built-in collection modules. The higher-order APIs are checked and lowered
//! as ordinary generic functions so their callbacks use managed call semantics.
use crate::{CheckedModule, FileId, SourceFile, SourceRange, Type, ast::*, ast_walk};
use ast_walk::Visitor;
use std::collections::HashSet;

pub(crate) const LIST_SYMBOLS: &[&str] = &[
    "length",
    "get",
    "append",
    "set",
    "remove",
    "map",
    "filter",
    "concatAll",
    "LengthOverflow",
];
pub(crate) const MAP_SYMBOLS: &[&str] = &[
    "length",
    "get",
    "insert",
    "remove",
    "keys",
    "entries",
    "fromList",
    "fromEntries",
    "DuplicateKey",
    "LengthOverflow",
];

pub(crate) fn entry_fields(key: &Type, value: &Type) -> Vec<(String, Type)> {
    vec![("key".into(), key.clone()), ("value".into(), value.clone())]
}

const SOURCE: &str = r#"
error ListOverflow { payload: {currentLength: Int, addedLength: Int} }
error MapOverflow { payload: {currentLength: Int, addedLength: Int} }
error DuplicateKey<K: Key> { payload: {key: K, firstIndex: Int, duplicateIndex: Int} }

fn map<T, U>(values: List<T>, transform: fn (T) -> U) -> List<U> {
    let empty: List<U> = [];
    var output = reserveList(empty, lengthList(values));
    for value in values {
        output = pushList(output, transform(value));
    }
    return finish(output);
}

fn filter<T>(values: List<T>, predicate: fn (T) -> Bool) -> List<T> {
    var output: List<T> = [];
    for value in values {
        if predicate(value) {
            output = pushList(output, value);
        }
    }
    return finish(output);
}

fn fromList<T, K: Key>(values: List<T>, keyOf: fn (T) -> K) -> Map<K, T> {
    var output: Map<K, T> = {};
    var index = 0;
    for value in values {
        let key = keyOf(value);
        let first = mapIndex(output, key);
        if first >= 0 {
            throw DuplicateKey<K>(message: "duplicate map key",
                payload: {key = key, firstIndex = first, duplicateIndex = index});
        }
        output = putMap(output, key, value);
        index = index + 1;
    }
    return finish(output);
}

fn fromEntries<K: Key, V>(entries: List<{key: K, value: V}>) -> Map<K, V> {
    var output: Map<K, V> = {};
    var index = 0;
    for entry in entries {
        let first = mapIndex(output, entry.key);
        if first >= 0 {
            throw DuplicateKey<K>(message: "duplicate map key",
                payload: {key = entry.key, firstIndex = first, duplicateIndex = index});
        }
        output = putMap(output, entry.key, entry.value);
        index = index + 1;
    }
    return finish(output);
}
"#;

pub(crate) fn source_file() -> &'static SourceFile {
    static FILE: std::sync::OnceLock<SourceFile> = std::sync::OnceLock::new();
    FILE.get_or_init(|| SourceFile::new(FileId(u32::MAX - 7), "<rinzu collections>", SOURCE))
}

fn prelude() -> &'static Module {
    static MODULE: std::sync::OnceLock<Module> = std::sync::OnceLock::new();
    MODULE.get_or_init(|| {
        let mut module = crate::parse_source(source_file()).expect("valid collection prelude");
        fn identity(name: &str) -> Option<&'static str> {
            Some(match name {
                "ListOverflow" => "rinzu.list::LengthOverflow",
                "MapOverflow" => "rinzu.map::LengthOverflow",
                "DuplicateKey" => "rinzu.map::DuplicateKey",
                "map" => "rinzu.list::map",
                "filter" => "rinzu.list::filter",
                "fromList" => "rinzu.map::fromList",
                "fromEntries" => "rinzu.map::fromEntries",
                "lengthList" => crate::DataOperation::ListLength.name(),
                "reserveList" => crate::DataOperation::ListBufferReserve.name(),
                "pushList" => crate::DataOperation::ListBufferAppend.name(),
                "mapIndex" => crate::DataOperation::MapBufferIndex.name(),
                "putMap" => crate::DataOperation::MapBufferInsert.name(),
                "finish" => crate::DataOperation::CollectionFinish.name(),
                _ => return None,
            })
        }
        struct Link;
        impl ast_walk::Visitor for Link {
            fn expression(&mut self, expression: &mut Expression) {
                if let ExpressionKind::Call { target, .. }
                | ExpressionKind::GenericCall { target, .. } = &mut expression.kind
                    && let [name] = target.as_mut_slice()
                    && let Some(id) = identity(name)
                {
                    *name = id.into();
                }
            }
        }
        for declaration in &mut module.declarations {
            let name = match declaration {
                Declaration::Error(error) => &mut error.name,
                Declaration::Function(function) => &mut function.name,
                _ => unreachable!("collection prelude declaration"),
            };
            *name = identity(name)
                .expect("collection declaration identity")
                .into();
            ast_walk::declaration(declaration, &mut Link);
        }
        module
    })
}

pub(crate) fn prepare(module: &Module) -> Module {
    // Package linking resolves these names per source file. The in-memory
    // compiler entry point also accepts a single, unlinked source module.
    struct LinkImports {
        list: bool,
        map: bool,
        referenced: HashSet<String>,
    }
    impl LinkImports {
        fn path(&mut self, path: &mut Vec<String>) {
            if let [root, module, symbol, ..] = path.as_slice()
                && root == "rinzu"
                && match module.as_str() {
                    "list" => self.list && LIST_SYMBOLS.contains(&symbol.as_str()),
                    "map" => self.map && MAP_SYMBOLS.contains(&symbol.as_str()),
                    _ => false,
                }
            {
                let name = format!("rinzu.{module}::{symbol}");
                path.splice(..3, [name]);
            }
            if let Some(name) = path.first()
                && (name.starts_with("rinzu.list::") || name.starts_with("rinzu.map::"))
            {
                self.referenced.insert(name.clone());
            }
        }
    }
    impl ast_walk::Visitor for LinkImports {
        fn expression(&mut self, expression: &mut Expression) {
            match &mut expression.kind {
                ExpressionKind::Name(path)
                | ExpressionKind::Call { target: path, .. }
                | ExpressionKind::Bind { target: path, .. }
                | ExpressionKind::Spawn { target: path, .. }
                | ExpressionKind::GenericCall { target: path, .. }
                | ExpressionKind::GenericReference { target: path, .. } => self.path(path),
                _ => {}
            }
        }
        fn ty(&mut self, ty: &mut TypeRef) {
            match &mut ty.kind {
                TypeRefKind::Named(path) | TypeRefKind::Generic { path, .. } => self.path(path),
                _ => {}
            }
        }
    }
    let mut module = module.clone();
    let imported = |name| module.imports.iter().any(|i| i.path == ["rinzu", name]);
    let mut linker = LinkImports {
        list: imported("list"),
        map: imported("map"),
        referenced: HashSet::new(),
    };
    ast_walk::module(&mut module, &mut linker);
    for export in &mut module.exports {
        linker.path(&mut export.path);
    }
    let mut used = linker.referenced;
    for operation in [
        crate::DataOperation::ListAppend,
        crate::DataOperation::ListConcatAll,
        crate::DataOperation::MapInsert,
    ] {
        if used.contains(operation.name()) {
            used.insert(operation.length_overflow().unwrap().into());
        }
    }
    if used.contains("rinzu.map::fromList") || used.contains("rinzu.map::fromEntries") {
        used.insert("rinzu.map::DuplicateKey".into());
    }
    module.declarations.extend(
        prelude()
            .declarations
            .iter()
            .filter(|declaration| used.contains(declaration.name()))
            .cloned(),
    );
    module
}

pub(crate) fn bounded_counter(
    checked: &CheckedModule,
    condition: &Expression,
    body: &Block,
) -> bool {
    use BinaryOperator::*;
    let ExpressionKind::Binary {
        operator,
        left,
        right,
    } = &condition.kind
    else {
        return false;
    };
    let direction = match operator {
        Less | LessEqual => 1,
        Greater | GreaterEqual => -1,
        _ => return false,
    };
    let ExpressionKind::Name(counter) = &left.kind else {
        return false;
    };
    if counter.len() != 1
        || !checked
            .expression_type(left.range)
            .is_some_and(Type::is_integer)
    {
        return false;
    }
    let Some(last) = body.statements.last() else {
        return false;
    };
    let StatementKind::Assign { name, value } = &last.kind else {
        return false;
    };
    if name != &counter[0] {
        return false;
    }
    let ExpressionKind::Binary {
        operator: update,
        left: previous,
        right: step,
    } = &value.kind
    else {
        return false;
    };
    if !matches!(&previous.kind,ExpressionKind::Name(path) if path==counter) {
        return false;
    }
    let ExpressionKind::Literal(Literal::Int(step)) = step.kind else {
        return false;
    };
    let delta = match update {
        Add => step,
        Subtract => match step.checked_neg() {
            Some(value) => value,
            None => return false,
        },
        _ => return false,
    };
    if delta.signum() != direction {
        return false;
    }
    let bound = match &right.kind {
        ExpressionKind::Literal(_) => None,
        ExpressionKind::Name(path) if path.len() == 1 && path != counter => Some(path[0].as_str()),
        ExpressionKind::Call {
            arguments: Arguments::Positional(values),
            ..
        } if checked.data_operations.get(&right.range)
            == Some(&crate::DataOperation::ListLength) =>
        {
            let Some(Expression {
                kind: ExpressionKind::Name(path),
                ..
            }) = values.first()
            else {
                return false;
            };
            if path.len() != 1 {
                return false;
            }
            Some(path[0].as_str())
        }
        _ => return false,
    };
    struct Writes<'a> {
        counter: &'a str,
        bound: Option<&'a str>,
        update: SourceRange,
        invalid: bool,
    }
    impl Visitor for Writes<'_> {
        fn enter_statement(&mut self, statement: &Statement) {
            if let StatementKind::Assign { name, .. } = &statement.kind
                && (Some(name.as_str()) == self.bound
                    || (name == self.counter && statement.range != self.update))
            {
                self.invalid = true;
            }
            if let StatementKind::Local { name, .. } = &statement.kind
                && (name == self.counter || Some(name.as_str()) == self.bound)
            {
                self.invalid = true;
            }
            if matches!(statement.kind, StatementKind::Continue) {
                self.invalid = true;
            }
        }
    }
    let mut writes = Writes {
        counter: &counter[0],
        bound,
        update: last.range,
        invalid: false,
    };
    ast_walk::block(&mut body.clone(), &mut writes);
    !writes.invalid
}

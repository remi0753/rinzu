//! Memoized function evaluations for freshly resolved and type-checked input.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ast::Declaration;
use crate::{CheckedFunction, CheckedModule, FileId, IrFunction, SourceRange, Type};

pub(crate) type Fingerprint = [u8; 32];
type ContextKey = (u32, (u32, u32, u32, u32, u32));
pub(crate) type Contexts = RefCell<BTreeMap<ContextKey, u32>>;

#[derive(Default, Serialize, Deserialize)]
pub struct FunctionFrontendCache {
    pub(crate) errors: Memos<SharedSummary<crate::error_effects::Summary>>,
    pub(crate) error_checks: Memos<()>,
    pub(crate) futures: Memos<SharedSummary<crate::error_effects::responsibility::Contract>>,
    pub(crate) lowering: Memos<IrFunction>,
}

impl FunctionFrontendCache {
    pub(crate) fn retain_functions(&mut self, checked: &CheckedModule) {
        let names: HashSet<_> = checked
            .functions
            .iter()
            .map(|function| function.declaration.name.as_str())
            .collect();
        self.errors.retain(|name, _| names.contains(name.as_str()));
        self.error_checks
            .retain(|name, _| names.contains(name.as_str()));
        self.futures.retain(|name, _| names.contains(name.as_str()));
        self.lowering
            .retain(|name, _| names.contains(name.as_str()));
    }
}

pub(crate) type Memos<T> = BTreeMap<String, Memo<T>>;

#[derive(Serialize, Deserialize)]
pub(crate) struct SharedSummary<T> {
    pub(crate) value: Arc<T>,
    pub(crate) digest: Option<Fingerprint>,
}

impl<T> Clone for SharedSummary<T> {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
            digest: self.digest,
        }
    }
}

impl<T: Serialize> SharedSummary<T> {
    pub(crate) fn new(value: T) -> Self {
        Self {
            digest: fingerprint(Some(&value)),
            value: Arc::new(value),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Memo<T> {
    input: Fingerprint,
    reads: Vec<(Read, Fingerprint)>,
    contexts: Vec<(ContextKey, u32)>,
    value: T,
}

#[derive(Default)]
pub(crate) struct Reads {
    facts: HashSet<Read>,
    contexts: Vec<(ContextKey, u32)>,
    seen_contexts: HashSet<ContextKey>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Reads>> = const { RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    static RESTORED: RefCell<BTreeMap<&'static str, usize>> = const { RefCell::new(BTreeMap::new()) };
}

#[cfg(test)]
mod tests;

pub(crate) fn recording<T>(evaluate: impl FnOnce() -> T) -> (T, Reads) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ACTIVE.with_borrow_mut(|active| *active = None);
        }
    }
    ACTIVE.with_borrow_mut(|active| {
        assert!(active.is_none(), "nested function memo recording");
        *active = Some(Reads::default());
    });
    let _reset = Reset;
    let value = evaluate();
    let reads = ACTIVE.with_borrow_mut(|active| active.take().expect("active recording"));
    (value, reads)
}

pub(crate) fn read(query: impl FnOnce() -> Read) {
    ACTIVE.with_borrow_mut(|active| {
        if let Some(reads) = active {
            reads.facts.insert(query());
        }
    });
}

pub(crate) fn context(key: ContextKey, assigned: u32) {
    ACTIVE.with_borrow_mut(|active| {
        if let Some(reads) = active
            && reads.seen_contexts.insert(key)
        {
            reads.contexts.push((key, assigned));
        }
    });
}

pub(crate) fn fingerprint(value: impl Serialize) -> Option<Fingerprint> {
    Some(Sha256::digest(rmp_serde::to_vec(&value).ok()?).into())
}

impl<T> Memo<T> {
    pub(crate) fn capture(
        input: Fingerprint,
        reads: Reads,
        value: T,
        resolve: impl Fn(&Read) -> Option<Fingerprint>,
    ) -> Option<Self> {
        let facts = reads
            .facts
            .into_iter()
            .map(|read| {
                let fingerprint = resolve(&read)?;
                Some((read, fingerprint))
            })
            .collect::<Option<_>>()?;
        Some(Self {
            input,
            reads: facts,
            contexts: reads.contexts,
            value,
        })
    }

    pub(crate) fn restore(
        &self,
        input: Fingerprint,
        resolve: impl Fn(&Read) -> Option<Fingerprint>,
        contexts: Option<&Contexts>,
    ) -> Option<&T> {
        if self.input != input
            || !self
                .reads
                .iter()
                .all(|(read, evidence)| resolve(read).as_ref() == Some(evidence))
        {
            return None;
        }
        if !self.contexts.is_empty() {
            let mut contexts = contexts?.borrow_mut();
            let mut additional = BTreeMap::new();
            // The engine assigns identifiers in first-visit order. Stage the
            // same visits and commit them only when the entire memo matches.
            for &(key, assigned) in &self.contexts {
                let next = u32::try_from(contexts.len() + additional.len() + 1).ok()?;
                let current = contexts
                    .get(&key)
                    .copied()
                    .unwrap_or_else(|| *additional.entry(key).or_insert(next));
                if current != assigned {
                    return None;
                }
            }
            contexts.extend(additional);
        }
        #[cfg(test)]
        RESTORED
            .with_borrow_mut(|counts| *counts.entry(std::any::type_name::<T>()).or_default() += 1);
        Some(&self.value)
    }
}

/// Results are computed against this invocation's inputs. In a fixed point
/// the resolver supplies current, possibly intermediate, summary values.
pub(crate) fn evaluate<T: Clone>(
    memos: &mut Memos<T>,
    name: &str,
    input: impl FnOnce() -> Option<Fingerprint>,
    resolve: impl Fn(&Read) -> Option<Fingerprint>,
    contexts: Option<&Contexts>,
    observe: impl Fn(&Read),
    compute: impl FnOnce() -> T,
) -> (T, bool) {
    let Some(input) = input() else {
        return (compute(), false);
    };
    if let Some(memo) = memos.get(name)
        && let Some(value) = memo.restore(input, &resolve, contexts)
    {
        for (read, _) in &memo.reads {
            observe(read);
        }
        return (value.clone(), true);
    }
    let (value, reads) = recording(compute);
    if let Some(memo) = Memo::capture(input, reads, value.clone(), resolve) {
        memos.insert(name.into(), memo);
    }
    (value, false)
}

#[derive(Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub(crate) enum Read {
    Range(Field, SourceRange),
    Function(String),
    FunctionAsync(String),
    FunctionExists(String),
    Native(String),
    Schema(String),
    PrivateEnvironment(String),
    ClosureSource,
    DisplayNative(Type),
    HasRethrow,
    Error(String),
    Future(String),
    DataFunction(String),
    CallbackFamily(String),
}

/// Index and hash the immutable checked facts lazily. Multiple analyses of
/// the same function in this check share their input fingerprints.
pub(crate) struct Facts<'a> {
    checked: &'a CheckedModule,
    functions: HashMap<&'a str, &'a CheckedFunction>,
    natives: HashMap<&'a str, &'a crate::SourceNativeSignature>,
    cached: RefCell<HashMap<Read, Option<Fingerprint>>>,
}

impl<'a> Facts<'a> {
    pub(crate) fn new(checked: &'a CheckedModule) -> Self {
        Self {
            checked,
            functions: checked
                .functions
                .iter()
                .map(|function| (function.declaration.name.as_str(), function))
                .collect(),
            natives: checked
                .source_native_signatures
                .iter()
                .map(|signature| (signature.name.as_str(), signature))
                .collect(),
            cached: RefCell::default(),
        }
    }

    pub(crate) fn body(&self, name: &str) -> Option<Fingerprint> {
        self.resolve(&Read::Function(name.into()))
    }

    pub(crate) fn resolve(&self, query: &Read) -> Option<Fingerprint> {
        match self.cached.borrow_mut().entry(query.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => *entry.insert(self.compute(query)),
        }
    }

    fn compute(&self, query: &Read) -> Option<Fingerprint> {
        match query {
            Read::Range(field, range) => field.fingerprint(self.checked, range),
            Read::Function(name) => fingerprint(self.functions.get(name.as_str())),
            Read::FunctionAsync(name) => fingerprint(
                self.functions
                    .get(name.as_str())
                    .is_some_and(|function| function.declaration.is_async),
            ),
            Read::FunctionExists(name) => fingerprint(self.functions.contains_key(name.as_str())),
            Read::Native(name) => fingerprint(self.natives.get(name.as_str())),
            Read::Schema(name) => fingerprint(self.checked.source_types.get(name)),
            Read::PrivateEnvironment(name) => fingerprint(private_environment(self.checked, name)),
            Read::ClosureSource => fingerprint(self.checked.closure_source),
            Read::DisplayNative(ty) => fingerprint(crate::sema::display_reads_native_record(
                &self.checked.source_types,
                ty,
            )),
            Read::HasRethrow => fingerprint(!self.checked.rethrows.is_empty()),
            Read::Error(_) | Read::Future(_) | Read::DataFunction(_) | Read::CallbackFamily(_) => {
                None
            }
        }
    }
}

fn private_environment(checked: &CheckedModule, ty: &str) -> bool {
    let Some(file) = checked.closure_source else {
        return false;
    };
    checked.syntax.declarations.iter().any(|declaration| {
        matches!(declaration, Declaration::Record(record)
            if record.name == ty
            && record.range.file_id == file
            && record.fields.len() == 1
            && record.fields[0].name == "environment")
    })
}

impl CheckedModule {
    pub(crate) fn memo_function(&self, name: &str) -> Option<&CheckedFunction> {
        read(|| Read::Function(name.into()));
        self.functions.iter().find(|f| f.declaration.name == name)
    }

    pub(crate) fn memo_is_async(&self, name: &str) -> bool {
        read(|| Read::FunctionAsync(name.into()));
        self.functions
            .iter()
            .any(|f| f.declaration.name == name && f.declaration.is_async)
    }

    pub(crate) fn memo_has_function(&self, name: &str) -> bool {
        read(|| Read::FunctionExists(name.into()));
        self.functions.iter().any(|f| f.declaration.name == name)
    }

    pub(crate) fn memo_native(&self, name: &str) -> Option<&crate::SourceNativeSignature> {
        read(|| Read::Native(name.into()));
        self.source_native_signatures
            .iter()
            .find(|s| s.name == name)
    }

    pub(crate) fn memo_schema(&self, name: &str) -> Option<&crate::SourceTypeSchema> {
        read(|| Read::Schema(name.into()));
        self.source_types.get(name)
    }

    pub(crate) fn memo_private_environment(&self, ty: &str) -> bool {
        read(|| Read::PrivateEnvironment(ty.into()));
        private_environment(self, ty)
    }

    pub(crate) fn memo_closure_source(&self) -> Option<FileId> {
        read(|| Read::ClosureSource);
        self.closure_source
    }

    pub(crate) fn memo_display_native(&self, ty: &Type) -> bool {
        read(|| Read::DisplayNative(ty.clone()));
        crate::sema::display_reads_native_record(&self.source_types, ty)
    }

    pub(crate) fn memo_has_rethrow(&self) -> bool {
        read(|| Read::HasRethrow);
        !self.rethrows.is_empty()
    }
}

macro_rules! checked_ranges {
    ($( $kind:ident $method:ident $field:ident : $type:ty; )* ; $( $set:ident $has:ident $set_field:ident; )*) => {
        #[derive(Clone, Copy, Eq, Hash, PartialEq, Serialize, Deserialize)]
        pub(crate) enum Field { $($kind,)* $($set,)* }

        impl CheckedModule {
            $(
                pub(crate) fn $method(&self, range: &SourceRange) -> Option<&$type> {
                    read(|| Read::Range(Field::$kind, *range));
                    self.$field.get(range)
                }
            )*
            $(
                pub(crate) fn $has(&self, range: &SourceRange) -> bool {
                    read(|| Read::Range(Field::$set, *range));
                    self.$set_field.contains(range)
                }
            )*
        }

        impl Field {
            fn fingerprint(self, checked: &CheckedModule, range: &SourceRange) -> Option<Fingerprint> {
                match self {
                    $(Self::$kind => fingerprint(checked.$field.get(range)),)*
                    $(Self::$set => fingerprint(checked.$set_field.contains(range)),)*
                }
            }
        }
    };
}

checked_ranges! {
    Expression fact_expression expression_types: Type;
    Upcast fact_error_upcasts error_upcasts: Type;
    Pattern fact_error_patterns error_patterns: Type;
    Rethrow fact_rethrows rethrows: crate::sema::errors::Rethrow;
    Test fact_error_tests error_tests: Type;
    Narrowing fact_error_narrowings error_narrowings: Type;
    Binding fact_bindings bindings: crate::sema::BoundFunction;
    StaticReference fact_static_references static_references: String;
    TypeReference fact_type_references type_references: Type;
    IndirectMember fact_indirect_fields indirect_fields: String;
    NativeCapture fact_native_captures native_captures: Type;
    CoreLength fact_core_lengths core_lengths: crate::CoreLengthKind;
    TaskControl fact_task_controls task_controls: crate::TaskControlKind;
    DataOperation fact_data_operations data_operations: crate::DataOperation;
    Stdout fact_stdout_calls stdout_calls: bool;
    NativeCall fact_native_calls native_calls: crate::NativeCallClass;
    SourceNativeCall fact_source_native_calls source_native_calls: crate::SourceNativeSignature;
    ReferenceTargets fact_reference_targets reference_targets: Vec<String>;
    IndirectCall fact_indirect_calls indirect_calls: Vec<String>;
    IndirectSignature fact_indirect_signatures indirect_signatures: crate::sema::IndirectSignature;
    Conversion fact_error_conversions error_conversions: crate::error_effects::ErrorConversion;
    Supplant fact_supplant_calls supplant_calls: crate::sema::SupplantCall;
    Constant fact_constant_references constant_references: crate::IntegerConstant;
    Bytes fact_byte_coercions byte_coercions: (u64, u64);
    ;
    NativeRead fact_native_record_reads native_record_reads;
    InBoundsCollection fact_in_bounds_collection_updates in_bounds_collection_updates;
}

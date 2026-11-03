//! Closed immutable data values and their typed equality. These native-owned
//! snapshots are independent of managed handle identities and transport layout.

use crate::key::{KeyHasher, KeySchema, KeyType};
use crate::scalar::{Comparison, ScalarKind, ScalarValue};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub enum ValueType {
    Scalar(ScalarKind),
    Unit,
    String,
    Bytes(u64),
    DynamicBytes,
    Error,
    Record {
        identity: String,
        fields: Vec<usize>,
    },
    State {
        identity: String,
        fields: Vec<usize>,
    },
    /// Readonly Error fields with ordinary same-Domain managed references.
    /// The common Error descriptor accepts any of these nominal identities.
    ErrorRecord {
        identity: String,
        fields: Vec<usize>,
    },
    Enum {
        identity: String,
        variants: Vec<Vec<usize>>,
    },
    List(usize),
    Map {
        key: usize,
        value: usize,
    },
    /// Future, function, Error and native values have no value equality.
    Other,
}

#[derive(Clone, Debug)]
pub enum Value {
    Scalar(ScalarValue),
    Error(crate::error_value::ErrorValue),
    Unit,
    String(String),
    Bytes(Vec<u8>),
    Record {
        identity: String,
        fields: Vec<Value>,
    },
    Enum {
        identity: String,
        variant: u32,
        fields: Vec<Value>,
    },
    List(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Other {
        ty: usize,
        slot: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueError {
    InvalidSchema,
    TypeMismatch,
    NoEquality,
    NotKey,
    DuplicateKey,
}

#[derive(Clone, Debug)]
pub struct ValueSchema {
    types: Vec<ValueType>,
    equality: Vec<bool>,
    transferable: Vec<bool>,
    keys: KeySchema,
}

impl ValueSchema {
    pub fn type_count(&self) -> usize {
        self.types.len()
    }

    pub fn ty(&self, index: usize) -> Result<&ValueType, ValueError> {
        self.types.get(index).ok_or(ValueError::InvalidSchema)
    }

    pub fn new(types: Vec<ValueType>) -> Result<Self, ValueError> {
        let mut nominal = HashSet::new();
        let mut dependencies = Vec::with_capacity(types.len());
        let mut key_types = Vec::with_capacity(types.len());
        for ty in &types {
            let children = match ty {
                ValueType::Record { identity, fields }
                | ValueType::State { identity, fields }
                | ValueType::ErrorRecord { identity, fields } => {
                    check_identity(&mut nominal, identity)?;
                    fields.clone()
                }
                ValueType::Enum { identity, variants } => {
                    check_identity(&mut nominal, identity)?;
                    variants.iter().flatten().copied().collect()
                }
                ValueType::List(element) => vec![*element],
                ValueType::Map { key, value } => vec![*key, *value],
                _ => Vec::new(),
            };
            if children.iter().any(|child| *child >= types.len()) {
                return Err(ValueError::InvalidSchema);
            }
            if let ValueType::ErrorRecord { fields, .. } = ty
                && (!(4..=5).contains(&fields.len())
                    || !matches!(types.get(fields[0]), Some(ValueType::String))
                    || !matches!(types.get(fields[1]), Some(ValueType::String)))
            {
                return Err(ValueError::InvalidSchema);
            }
            dependencies.push(children);
            key_types.push(match ty {
                ValueType::Scalar(crate::scalar::ScalarKind::Bool) => KeyType::Bool,
                ValueType::Scalar(kind) => KeyType::Integer(*kind),
                ValueType::String => KeyType::String,
                ValueType::Bytes(length) => KeyType::Bytes {
                    length: Some(*length),
                },
                ValueType::DynamicBytes => KeyType::Bytes { length: None },
                ValueType::Record { identity, fields } => KeyType::Record {
                    identity: identity.clone(),
                    fields: fields.clone(),
                },
                ValueType::Enum { identity, variants } => KeyType::Enum {
                    identity: identity.clone(),
                    variants: variants.clone(),
                },
                _ => KeyType::NotKey,
            });
        }
        let keys = KeySchema::new(key_types).map_err(|_| ValueError::InvalidSchema)?;
        for ty in &types {
            if let ValueType::Map { key, .. } = ty
                && !keys.is_key(*key).map_err(|_| ValueError::InvalidSchema)?
            {
                return Err(ValueError::InvalidSchema);
            }
        }
        let mut equality = types
            .iter()
            .map(|ty| {
                !matches!(
                    ty,
                    ValueType::Other
                        | ValueType::State { .. }
                        | ValueType::ErrorRecord { .. }
                        | ValueType::Error
                )
            })
            .collect::<Vec<_>>();
        let mut transferable = types
            .iter()
            .map(|ty| {
                !matches!(
                    ty,
                    ValueType::Other | ValueType::State { .. } | ValueType::ErrorRecord { .. }
                )
            })
            .collect::<Vec<_>>();
        let mut colors = vec![0u8; types.len()];
        for root in 0..types.len() {
            if colors[root] != 0 {
                continue;
            }
            colors[root] = 1;
            let mut stack = vec![(root, 0usize)];
            while let Some(&(node, index)) = stack.last() {
                if let Some(&child) = dependencies[node].get(index) {
                    if colors[child] == 0 {
                        colors[child] = 1;
                        stack.push((child, 0));
                    } else {
                        equality[node] &= colors[child] == 2 && equality[child];
                        transferable[node] &= colors[child] == 2 && transferable[child];
                        stack.last_mut().unwrap().1 += 1;
                    }
                } else {
                    colors[node] = 2;
                    stack.pop();
                }
            }
        }
        Ok(Self {
            types,
            equality,
            transferable,
            keys,
        })
    }

    pub fn has_transferable(&self, root: usize) -> Result<bool, ValueError> {
        self.transferable
            .get(root)
            .copied()
            .ok_or(ValueError::InvalidSchema)
    }

    pub fn has_equality(&self, root: usize) -> Result<bool, ValueError> {
        self.equality
            .get(root)
            .copied()
            .ok_or(ValueError::InvalidSchema)
    }

    pub fn is_key(&self, root: usize) -> Result<bool, ValueError> {
        self.keys
            .is_key(root)
            .map_err(|_| ValueError::InvalidSchema)
    }

    pub fn validate(&self, root: usize, value: &Value) -> Result<(), ValueError> {
        if root >= self.types.len() {
            return Err(ValueError::InvalidSchema);
        }
        let mut stack = Vec::new();
        let mut current = (root, value);
        loop {
            let (node, value) = current;
            match (&self.types[node], value) {
                (ValueType::Scalar(kind), Value::Scalar(value)) if *kind == value.kind() => {}
                (ValueType::Unit, Value::Unit) | (ValueType::String, Value::String(_)) => {}
                (ValueType::Error, Value::Error(error)) if error.validate() => {}
                (ValueType::Error, Value::Other { ty, .. })
                    if matches!(self.types.get(*ty), Some(ValueType::ErrorRecord { .. })) => {}
                (ValueType::DynamicBytes, Value::Bytes(bytes))
                    if bytes.len() <= i64::MAX as usize => {}
                (ValueType::Bytes(length), Value::Bytes(bytes))
                    if *length == bytes.len() as u64 => {}
                (
                    ValueType::Record {
                        identity,
                        fields: types,
                    },
                    Value::Record {
                        identity: actual,
                        fields,
                    },
                ) if identity == actual => {
                    if types.len() != fields.len() {
                        return Err(ValueError::TypeMismatch);
                    }
                    stack.extend(types.iter().copied().zip(fields));
                }
                (
                    ValueType::Enum { identity, variants },
                    Value::Enum {
                        identity: actual,
                        variant,
                        fields,
                    },
                ) if identity == actual => {
                    let types = variants
                        .get(*variant as usize)
                        .ok_or(ValueError::TypeMismatch)?;
                    if types.len() != fields.len() {
                        return Err(ValueError::TypeMismatch);
                    }
                    stack.extend(types.iter().copied().zip(fields));
                }
                (ValueType::List(element), Value::List(values)) => {
                    stack.extend(values.iter().map(|value| (*element, value)))
                }
                (ValueType::Map { key, value }, Value::Map(entries)) => {
                    let mut seen: HashMap<u64, Vec<&Value>> = HashMap::new();
                    for (entry_key, entry_value) in entries {
                        let hash = self.key_hash(*key, entry_key)?;
                        let bucket = seen.entry(hash).or_default();
                        for previous in &*bucket {
                            if self.equal(*key, previous, entry_key)? {
                                return Err(ValueError::DuplicateKey);
                            }
                        }
                        bucket.push(entry_key);
                        stack.push((*value, entry_value));
                    }
                }
                (
                    ValueType::Other | ValueType::State { .. } | ValueType::ErrorRecord { .. },
                    Value::Other { ty, .. },
                ) if *ty == node => {}
                _ => return Err(ValueError::TypeMismatch),
            }
            let Some(next) = stack.pop() else { break };
            current = next;
        }
        Ok(())
    }

    pub fn key_hash(&self, root: usize, value: &Value) -> Result<u64, ValueError> {
        if !self.is_key(root)? {
            return Err(ValueError::NotKey);
        }
        self.validate(root, value)?;
        let mut hash = KeyHasher::default();
        let mut stack = Vec::new();
        let mut current = value;
        loop {
            let value = current;
            match value {
                Value::Scalar(value) if value.kind() == crate::scalar::ScalarKind::Bool => {
                    hash.boolean(value.bits() != 0);
                }
                Value::Scalar(value) => hash.integer(*value).map_err(|_| ValueError::NotKey)?,
                Value::String(text) => hash.string(text),
                Value::Bytes(bytes) => hash.bytes(bytes),
                Value::Record { identity, fields } => {
                    hash.record_header(identity, fields.len());
                    stack.extend(fields.iter().rev());
                }
                Value::Enum {
                    identity,
                    variant,
                    fields,
                } => {
                    hash.enum_header(identity, *variant, fields.len());
                    stack.extend(fields.iter().rev());
                }
                _ => return Err(ValueError::NotKey),
            }
            let Some(next) = stack.pop() else { break };
            current = next;
        }
        Ok(hash.finish())
    }

    /// Every literal entry has already been evaluated by the caller. Validate
    /// all of them before coalescing duplicates; the later value replaces the
    /// earlier value in a newly owned collection.
    pub fn map(&self, root: usize, entries: Vec<(Value, Value)>) -> Result<Value, ValueError> {
        let Some(ValueType::Map { key, value }) = self.types.get(root) else {
            return Err(ValueError::TypeMismatch);
        };
        for (entry_key, entry_value) in &entries {
            self.validate(*key, entry_key)?;
            self.validate(*value, entry_value)?;
        }
        let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
        let mut normalized: Vec<(Value, Value)> = Vec::with_capacity(entries.len());
        for (entry_key, entry_value) in entries {
            let hash = self.key_hash(*key, &entry_key)?;
            let bucket = buckets.entry(hash).or_default();
            let mut existing = None;
            for index in &*bucket {
                if self.equal(*key, &normalized[*index].0, &entry_key)? {
                    existing = Some(*index);
                    break;
                }
            }
            if let Some(index) = existing {
                normalized[index].1 = entry_value;
            } else {
                bucket.push(normalized.len());
                normalized.push((entry_key, entry_value));
            }
        }
        Ok(Value::Map(normalized))
    }

    pub fn equal(&self, root: usize, left: &Value, right: &Value) -> Result<bool, ValueError> {
        if !self.has_equality(root)? {
            return Err(ValueError::NoEquality);
        }
        self.validate(root, left)?;
        self.validate(root, right)?;
        let mut stack = Vec::new();
        let mut current = (root, left, right);
        loop {
            let (node, left, right) = current;
            match (&self.types[node], left, right) {
                (ValueType::Scalar(_), Value::Scalar(a), Value::Scalar(b)) => {
                    if !a
                        .compare(Comparison::Equal, *b)
                        .map_err(|_| ValueError::TypeMismatch)?
                    {
                        return Ok(false);
                    }
                }
                (ValueType::Unit, Value::Unit, Value::Unit) => {}
                (ValueType::String, Value::String(a), Value::String(b)) if a == b => {}
                (
                    ValueType::Bytes(_) | ValueType::DynamicBytes,
                    Value::Bytes(a),
                    Value::Bytes(b),
                ) if a == b => {}
                (
                    ValueType::Record { fields: types, .. },
                    Value::Record { fields: a, .. },
                    Value::Record { fields: b, .. },
                ) => stack.extend(
                    types
                        .iter()
                        .copied()
                        .zip(a.iter().zip(b))
                        .map(|(ty, (a, b))| (ty, a, b)),
                ),
                (
                    ValueType::Enum { variants, .. },
                    Value::Enum {
                        variant: av,
                        fields: a,
                        ..
                    },
                    Value::Enum {
                        variant: bv,
                        fields: b,
                        ..
                    },
                ) if av == bv => stack.extend(
                    variants[*av as usize]
                        .iter()
                        .copied()
                        .zip(a.iter().zip(b))
                        .map(|(ty, (a, b))| (ty, a, b)),
                ),
                (ValueType::List(element), Value::List(a), Value::List(b))
                    if a.len() == b.len() =>
                {
                    stack.extend(a.iter().zip(b).map(|(a, b)| (*element, a, b)))
                }
                (ValueType::Map { key, value }, Value::Map(a), Value::Map(b))
                    if a.len() == b.len() =>
                {
                    let mut right: HashMap<u64, Vec<&(Value, Value)>> = HashMap::new();
                    for entry in b {
                        right
                            .entry(self.key_hash(*key, &entry.0)?)
                            .or_default()
                            .push(entry);
                    }
                    for (ak, av) in a {
                        let hash = self.key_hash(*key, ak)?;
                        let mut matched = None;
                        if let Some(bucket) = right.get(&hash) {
                            for entry in bucket {
                                if self.equal(*key, ak, &entry.0)? {
                                    matched = Some(&entry.1);
                                    break;
                                }
                            }
                        }
                        let Some(bv) = matched else {
                            return Ok(false);
                        };
                        stack.push((*value, av, bv));
                    }
                }
                _ => return Ok(false),
            }
            let Some(next) = stack.pop() else { break };
            current = next;
        }
        Ok(true)
    }
}

fn check_identity(identities: &mut HashSet<String>, identity: &str) -> Result<(), ValueError> {
    if identity.is_empty() || identity.contains('\0') || !identities.insert(identity.to_owned()) {
        return Err(ValueError::InvalidSchema);
    }
    Ok(())
}

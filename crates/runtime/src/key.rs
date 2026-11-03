//! Private content hashes for Map keys. Type tags and length framing are
//! shared with the AOT backend; handles, addresses and GC locations never enter
//! a hash. Equality still decides a bucket collision.

use crate::scalar::{ScalarError, ScalarValue};

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Copy, Debug)]
pub struct KeyHasher {
    hash: u64,
}

impl Default for KeyHasher {
    fn default() -> Self {
        Self { hash: OFFSET }
    }
}

impl KeyHasher {
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash = (self.hash ^ u64::from(*byte)).wrapping_mul(PRIME);
        }
    }

    pub fn integer(&mut self, value: ScalarValue) -> Result<(), ScalarError> {
        if !value.kind().integer() {
            return Err(ScalarError::TypeMismatch);
        }
        self.write(&[0x10, value.kind() as u8]);
        self.write(&value.bits().to_le_bytes()[..(value.kind().width() / 8) as usize]);
        Ok(())
    }

    pub fn boolean(&mut self, value: bool) {
        self.write(&[0x11, u8::from(value)]);
    }

    pub fn bytes(&mut self, value: &[u8]) {
        self.write(&[0x21]);
        self.write(&(value.len() as u64).to_le_bytes());
        self.write(value);
    }

    pub(crate) fn string_header(&mut self, length: usize) {
        self.write(&[0x20]);
        self.write(&(length as u64).to_le_bytes());
    }

    pub fn string(&mut self, value: &str) {
        self.string_header(value.len());
        self.write(value.as_bytes());
    }

    pub(crate) fn enum_header(&mut self, identity: &str, variant: u32, fields: usize) {
        self.write(&[0x30]);
        self.write(&(identity.len() as u64).to_le_bytes());
        self.write(identity.as_bytes());
        self.write(&variant.to_le_bytes());
        self.write(&(fields as u64).to_le_bytes());
    }

    pub(crate) fn record_header(&mut self, identity: &str, fields: usize) {
        self.write(&[0x31]);
        self.write(&(identity.len() as u64).to_le_bytes());
        self.write(identity.as_bytes());
        self.write(&(fields as u64).to_le_bytes());
    }

    #[must_use]
    pub const fn finish(self) -> u64 {
        self.hash
    }
}

#[must_use]
pub fn string_hash(value: &str) -> u64 {
    let mut hash = KeyHasher::default();
    hash.string(value);
    hash.finish()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyType {
    Integer(crate::scalar::ScalarKind),
    String,
    Bool,
    Bytes {
        length: Option<u64>,
    },
    Record {
        identity: String,
        fields: Vec<usize>,
    },
    Enum {
        identity: String,
        variants: Vec<Vec<usize>>,
    },
    /// Floats, collections, native values and other excluded types.
    NotKey,
}

#[derive(Clone, Debug, PartialEq)]
pub enum KeyValue {
    Integer(ScalarValue),
    String(String),
    Bool(bool),
    Bytes(Vec<u8>),
    Record {
        identity: String,
        fields: Vec<KeyValue>,
    },
    Enum {
        identity: String,
        variant: u32,
        fields: Vec<KeyValue>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyError {
    InvalidSchema,
    NotKey,
    TypeMismatch,
}

/// Closed indexed metadata permits cycle detection without native recursion.
/// Nominal names are the compiler's canonical identities, including imports.
#[derive(Clone, Debug)]
pub struct KeySchema {
    types: Vec<KeyType>,
    allowed: Vec<bool>,
}

impl KeySchema {
    pub fn new(types: Vec<KeyType>) -> Result<Self, KeyError> {
        let mut identities = std::collections::HashSet::new();
        let mut dependencies = Vec::with_capacity(types.len());
        let mut allowed = Vec::with_capacity(types.len());
        for ty in &types {
            let children = match ty {
                KeyType::Record { identity, fields } => {
                    if identity.is_empty()
                        || identity.contains('\0')
                        || !identities.insert(identity.clone())
                    {
                        return Err(KeyError::InvalidSchema);
                    }
                    fields.clone()
                }
                KeyType::Enum { identity, variants } => {
                    if identity.is_empty()
                        || identity.contains('\0')
                        || !identities.insert(identity.clone())
                    {
                        return Err(KeyError::InvalidSchema);
                    }
                    variants.iter().flatten().copied().collect::<Vec<_>>()
                }
                _ => Vec::new(),
            };
            if children.iter().any(|child| *child >= types.len()) {
                return Err(KeyError::InvalidSchema);
            }
            allowed.push(match ty {
                KeyType::Integer(kind) => kind.integer(),
                KeyType::NotKey => false,
                _ => true,
            });
            dependencies.push(children);
        }
        let mut colors = vec![0u8; types.len()];
        for root in 0..types.len() {
            if colors[root] != 0 {
                continue;
            }
            colors[root] = 1;
            let mut stack = vec![(root, 0usize)];
            while let Some(&(node, child_index)) = stack.last() {
                if let Some(&child) = dependencies[node].get(child_index) {
                    if colors[child] == 0 {
                        colors[child] = 1;
                        stack.push((child, 0));
                    } else {
                        allowed[node] &= colors[child] == 2 && allowed[child];
                        stack.last_mut().unwrap().1 += 1;
                    }
                } else {
                    colors[node] = 2;
                    stack.pop();
                }
            }
        }
        Ok(Self { types, allowed })
    }

    pub fn is_key(&self, root: usize) -> Result<bool, KeyError> {
        self.allowed
            .get(root)
            .copied()
            .ok_or(KeyError::InvalidSchema)
    }

    fn validate(&self, root: usize, value: &KeyValue) -> Result<(), KeyError> {
        if !self.is_key(root)? {
            return Err(KeyError::NotKey);
        }
        let mut stack = vec![(root, value)];
        while let Some((node, value)) = stack.pop() {
            match (&self.types[node], value) {
                (KeyType::Integer(kind), KeyValue::Integer(value)) if *kind == value.kind() => {}
                (KeyType::String, KeyValue::String(_)) => {}
                (KeyType::Bool, KeyValue::Bool(_)) => {}
                (KeyType::Bytes { length }, KeyValue::Bytes(bytes))
                    if length.is_none_or(|length| length == bytes.len() as u64) => {}
                (
                    KeyType::Record {
                        identity,
                        fields: types,
                    },
                    KeyValue::Record {
                        identity: actual,
                        fields,
                    },
                ) if identity == actual && types.len() == fields.len() => {
                    stack.extend(types.iter().copied().zip(fields));
                }
                (
                    KeyType::Enum { identity, variants },
                    KeyValue::Enum {
                        identity: actual,
                        variant,
                        fields,
                    },
                ) if identity == actual => {
                    let types = variants
                        .get(*variant as usize)
                        .ok_or(KeyError::TypeMismatch)?;
                    if types.len() != fields.len() {
                        return Err(KeyError::TypeMismatch);
                    }
                    stack.extend(types.iter().copied().zip(fields));
                }
                _ => return Err(KeyError::TypeMismatch),
            }
        }
        Ok(())
    }

    pub fn hash(&self, root: usize, value: &KeyValue) -> Result<u64, KeyError> {
        self.validate(root, value)?;
        let mut hash = KeyHasher::default();
        let mut stack = vec![value];
        while let Some(value) = stack.pop() {
            match value {
                KeyValue::Integer(value) => {
                    hash.integer(*value).map_err(|_| KeyError::TypeMismatch)?
                }
                KeyValue::String(value) => hash.string(value),
                KeyValue::Bool(value) => hash.boolean(*value),
                KeyValue::Bytes(value) => hash.bytes(value),
                KeyValue::Record { identity, fields } => {
                    hash.record_header(identity, fields.len());
                    stack.extend(fields.iter().rev());
                }
                KeyValue::Enum {
                    identity,
                    variant,
                    fields,
                } => {
                    hash.enum_header(identity, *variant, fields.len());
                    stack.extend(fields.iter().rev());
                }
            }
        }
        Ok(hash.finish())
    }

    pub fn equal(&self, root: usize, left: &KeyValue, right: &KeyValue) -> Result<bool, KeyError> {
        // Validate complete inputs, even when an earlier payload differs.
        self.validate(root, left)?;
        self.validate(root, right)?;
        let mut stack = vec![(left, right)];
        while let Some((left, right)) = stack.pop() {
            match (left, right) {
                (KeyValue::Integer(a), KeyValue::Integer(b)) if a.bits() == b.bits() => {}
                (KeyValue::String(a), KeyValue::String(b)) if a == b => {}
                (KeyValue::Bool(a), KeyValue::Bool(b)) if a == b => {}
                (KeyValue::Bytes(a), KeyValue::Bytes(b)) if a == b => {}
                (KeyValue::Record { fields: af, .. }, KeyValue::Record { fields: bf, .. }) => {
                    stack.extend(af.iter().zip(bf));
                }
                (
                    KeyValue::Enum {
                        variant: a,
                        fields: af,
                        ..
                    },
                    KeyValue::Enum {
                        variant: b,
                        fields: bf,
                        ..
                    },
                ) if a == b => stack.extend(af.iter().zip(bf)),
                _ => return Ok(false),
            }
        }
        Ok(true)
    }
}

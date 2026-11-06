//! Owner-local JSON values and their private typed runtime operations.
mod output;
mod typed;

use super::composite::DecodedSlot;
use super::*;
use crate::failure::FailureCause;
use crate::json as core;
use crate::scalar::{ScalarKind, ScalarValue};
use crate::value::ValueType;
use rinzu_abi::data_operation::DataOperation;
use std::ops::Range;

const PARSED_BYTES: u32 = 0;
const PARSED_STRING: u32 = 1;
const NULL: u32 = 2;
const BOOLEAN: u32 = 3;
const TEXT: u32 = 4;
const TOKEN: u32 = 5;
const INTEGER: u32 = 6;
const FLOAT: u32 = 7;
const ARRAY: u32 = 8;
const OBJECT: u32 = 9;

pub struct JsonCall<'a> {
    pub operation: DataOperation,
    pub result: usize,
    pub types: &'a [usize],
    pub slots: &'a [ManagedSlot],
}

enum Fault {
    Runtime(DataError),
    User(core::Error),
}
impl From<DataError> for Fault {
    fn from(error: DataError) -> Self {
        Self::Runtime(error)
    }
}
impl From<HeapError> for Fault {
    fn from(error: HeapError) -> Self {
        Self::Runtime(error.into())
    }
}
impl From<crate::value::ValueError> for Fault {
    fn from(error: crate::value::ValueError) -> Self {
        Self::Runtime(error.into())
    }
}
impl From<core::Error> for Fault {
    fn from(error: core::Error) -> Self {
        Self::User(error)
    }
}

type Result<T> = std::result::Result<T, Fault>;

#[derive(Clone, Copy)]
enum Repr {
    Parsed {
        source: HeapRef,
        index: HeapRef,
        node: u32,
        tag: u32,
    },
    Built {
        child: Option<HeapValue>,
        tag: u32,
    },
}
impl Repr {
    fn kind(self, heap: &DomainHeap) -> Result<u32> {
        Ok(match self {
            Self::Parsed {
                source,
                index,
                node,
                ..
            } => document(heap, source, index)?.node(node).kind,
            Self::Built { tag, .. } => match tag {
                NULL => core::NULL,
                BOOLEAN => core::BOOLEAN,
                TEXT => core::TEXT,
                TOKEN | INTEGER | FLOAT => core::NUMBER,
                ARRAY => core::ARRAY,
                OBJECT => core::OBJECT,
                _ => return Err(DataError::InvalidEncoding.into()),
            },
        })
    }
}
#[derive(Clone, Copy)]
enum Child {
    Parsed(Repr),
    Built(HeapRef),
}

fn scalar(kind: ScalarKind, bits: u64) -> ManagedSlot {
    ManagedSlot::Scalar(ScalarValue::from_bits(kind, bits).expect("valid JSON scalar"))
}
fn int(value: i64) -> ManagedSlot {
    scalar(ScalarKind::I64, value as u64)
}
fn bits(slot: ManagedSlot) -> Result<u64> {
    match slot {
        ManagedSlot::Scalar(value) => Ok(value.bits()),
        _ => Err(DataError::WrongKind.into()),
    }
}
fn reference(slot: ManagedSlot) -> Result<ManagedHandle> {
    match slot {
        ManagedSlot::Reference(handle) => Ok(handle),
        _ => Err(DataError::WrongKind.into()),
    }
}
fn heap_reference(value: HeapValue) -> Result<HeapRef> {
    match value {
        HeapValue::Reference(reference) => Ok(reference),
        _ => Err(DataError::WrongKind.into()),
    }
}
fn reference_field(heap: &DomainHeap, object: HeapRef, index: usize) -> Result<HeapRef> {
    heap_reference(heap.field_at(object, index)?)
}

fn bytes(heap: &DomainHeap, reference: HeapRef) -> Result<&[u8]> {
    let words = heap.packed_words(reference)?;
    let (&length, words) = words.split_first().ok_or(DataError::InvalidEncoding)?;
    let length = usize::try_from(length).map_err(|_| DataError::AddressSpaceExhausted)?;
    if words.len() != length.div_ceil(8) {
        return Err(DataError::InvalidEncoding.into());
    }
    // SAFETY: each packed word is initialized, the byte length is verified
    // against its storage, and the heap loan prevents movement or writes.
    Ok(unsafe { std::slice::from_raw_parts(words.as_ptr().cast(), length) })
}

fn document(heap: &DomainHeap, source: HeapRef, index: HeapRef) -> Result<core::Document<'_>> {
    Ok(core::Document::new(
        bytes(heap, source)?,
        bytes(heap, index)?,
    ))
}

fn number_error(reason: u32) -> Fault {
    core::Error {
        code: core::NUMBER_ERROR,
        reason,
        ..Default::default()
    }
    .into()
}

fn expect_kind(expected: u32, actual: u32) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(core::Error {
            code: core::TYPE,
            expected,
            actual,
            ..Default::default()
        }
        .into())
    }
}

impl ManagedData {
    pub fn json_operation(
        &mut self,
        heap: &mut DomainHeap,
        schema: &core::Schema,
        call: JsonCall<'_>,
        mut poll: impl FnMut() -> Option<FailureCause>,
    ) -> std::result::Result<ManagedSlot, DataError> {
        use DataOperation::*;
        let outcome = match call.operation {
            JsonParse | JsonParseWithLimits | JsonDecode | JsonDecodeWithLimits => {
                self.json_parse(heap, schema, &call, &mut poll)
            }
            JsonEncode | JsonEncodeWithLimits => self.json_encode(heap, &call, &mut poll),
            _ => self.json_query(heap, &call),
        };
        match outcome {
            Ok(value) => Ok(value),
            Err(Fault::Runtime(error)) => Err(error),
            Err(Fault::User(error)) => Err(DataError::ManagedThrown(
                self.json_error(heap, schema, error)?,
            )),
        }
    }

    fn json_limits(
        &self,
        heap: &DomainHeap,
        slot: Option<ManagedSlot>,
        bytes_kind: u32,
    ) -> Result<core::Limits> {
        let Some(slot) = slot else {
            return Ok(core::Limits::default());
        };
        let (record, _) = self.any_reference(heap, reference(slot)?)?;
        let value = |index| heap.integer_field(record, index);
        let (bytes, depth, nodes) = if bytes_kind == core::INPUT {
            (value(3)?, value(2)?, value(4)?)
        } else {
            (value(4)?, value(2)?, value(3)?)
        };
        Ok(core::Limits {
            bytes,
            depth,
            nodes,
        }
        .validate(bytes_kind)?)
    }

    fn json_error(
        &mut self,
        heap: &mut DomainHeap,
        schema: &core::Schema,
        error: core::Error,
    ) -> std::result::Result<ManagedHandle, DataError> {
        let ty = schema.errors[(error.code - 1) as usize] as usize;
        let (name, message) = match error.code {
            core::DECODE => ("DecodeFailed", "invalid JSON input"),
            core::INVALID_LIMIT => ("InvalidLimits", "invalid JSON limit"),
            core::LIMIT => ("LimitExceeded", "JSON limit exceeded"),
            core::TYPE => ("TypeMismatch", "unexpected JSON value kind"),
            core::NUMBER_ERROR => ("NumberError", "JSON number conversion failed"),
            core::SCHEMA => ("SchemaMismatch", "JSON does not match the requested type"),
            _ => return Err(DataError::InvalidEncoding),
        };
        let ValueType::ErrorRecord { fields, .. } = self.value_schema()?.ty(ty)? else {
            return Err(DataError::WrongKind);
        };
        let [_, _, source, cause, payload] = *fields.as_slice() else {
            return Err(DataError::InvalidEncoding);
        };
        let ValueType::Record { fields, .. } = self.value_schema()?.ty(payload)? else {
            return Err(DataError::WrongKind);
        };
        let mut types = [0; 6];
        types[..fields.len()].copy_from_slice(fields);
        let first_offset = |offset| (offset != core::NONE).then(|| int(i64::from(offset)));
        let offset = int(i64::from(error.offset));
        let mut values = [ManagedSlot::Unit; 6];
        match error.code {
            core::DECODE => {
                values[0] = offset;
                values[1] = self.json_option(heap, types[1], first_offset(error.first))?;
                values[2] = self.json_enum(heap, types[2], error.reason)?;
            }
            core::INVALID_LIMIT => {
                values[0] = self.json_enum(heap, types[0], error.reason)?;
                values[1..4].copy_from_slice(&[
                    int(error.maximum),
                    int(error.minimum),
                    int(error.supplied),
                ]);
            }
            core::LIMIT => {
                values[0] = self.json_option(heap, types[0], first_offset(error.offset))?;
                values[1] = self.json_enum(heap, types[1], error.reason)?;
                values[2] = int(error.limit);
            }
            core::TYPE => {
                values[0] = self.json_enum(heap, types[0], error.actual)?;
                values[1] = self.json_enum(heap, types[1], error.expected)?;
            }
            core::NUMBER_ERROR => values[0] = self.json_enum(heap, types[0], error.reason)?,
            core::SCHEMA => {
                let actual = if error.reason == 0 {
                    Some(self.json_enum(heap, self.json_option_child(types[0])?, error.actual)?)
                } else {
                    None
                };
                let expected = if error.reason == 0 {
                    Some(self.json_enum(heap, self.json_option_child(types[2])?, error.expected)?)
                } else {
                    None
                };
                values[0] = self.json_option_owned(heap, types[0], actual)?;
                values[1] = offset;
                values[2] = self.json_option_owned(heap, types[2], expected)?;
                let key = error
                    .key
                    .as_ref()
                    .map(|key| self.json_string(heap, key))
                    .transpose()?;
                values[3] = self.json_option_owned(heap, types[3], key)?;
                values[4] = self.json_string(heap, &error.path)?;
                values[5] = self.json_enum(heap, types[5], error.reason)?;
            }
            _ => unreachable!(),
        }
        let count = match error.code {
            core::SCHEMA => 6,
            core::INVALID_LIMIT => 4,
            core::DECODE | core::LIMIT => 3,
            core::TYPE => 2,
            _ => 1,
        };
        let payload = self.json_owned_composite(heap, payload, None, &values[..count])?;
        let name = ManagedSlot::Reference(self.create_string(heap, name)?);
        let message = ManagedSlot::Reference(self.create_string(heap, message)?);
        let source = self.json_enum(heap, source, 0)?;
        let cause = self.json_enum(heap, cause, 0)?;
        self.json_owned_composite(heap, ty, None, &[name, message, source, cause, payload])
            .and_then(|slot| {
                if let ManagedSlot::Reference(handle) = slot {
                    Ok(handle)
                } else {
                    Err(DataError::WrongKind)
                }
            })
    }

    fn json_string(
        &mut self,
        heap: &mut DomainHeap,
        text: &[u8],
    ) -> std::result::Result<ManagedSlot, DataError> {
        let text = std::str::from_utf8(text).map_err(|_| DataError::InvalidEncoding)?;
        self.create_string(heap, text).map(ManagedSlot::Reference)
    }

    fn json_copy(
        &mut self,
        heap: &mut DomainHeap,
        source: HeapRef,
        range: Range<usize>,
        text: bool,
    ) -> Result<ManagedSlot> {
        let kind = if text {
            DataKind::String
        } else {
            DataKind::Bytes(range.len() as u64)
        };
        let target =
            heap.copy_packed_bytes(if text { STRING_TYPE } else { BYTES_TYPE }, source, range)?;
        Ok(ManagedSlot::Reference(self.root(heap, target, kind)?))
    }

    fn json_enum(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        variant: u32,
    ) -> std::result::Result<ManagedSlot, DataError> {
        self.create_composite(heap, ty, Some(variant), &[])
            .map(ManagedSlot::Reference)
    }

    fn json_option_child(&self, ty: usize) -> std::result::Result<usize, DataError> {
        let ValueType::Enum { variants, .. } = self.value_schema()?.ty(ty)? else {
            return Err(DataError::WrongKind);
        };
        let [none, some] = variants.as_slice() else {
            return Err(DataError::InvalidEncoding);
        };
        match (none.as_slice(), some.as_slice()) {
            ([], [child]) => Ok(*child),
            _ => Err(DataError::InvalidEncoding),
        }
    }

    fn json_option(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        value: Option<ManagedSlot>,
    ) -> std::result::Result<ManagedSlot, DataError> {
        self.create_composite(heap, ty, Some(u32::from(value.is_some())), value.as_slice())
            .map(ManagedSlot::Reference)
    }

    fn json_option_owned(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        value: Option<ManagedSlot>,
    ) -> std::result::Result<ManagedSlot, DataError> {
        self.json_owned_composite(heap, ty, Some(u32::from(value.is_some())), value.as_slice())
    }

    fn json_release(
        &mut self,
        heap: &mut DomainHeap,
        slots: impl IntoIterator<Item = ManagedSlot>,
    ) -> std::result::Result<(), DataError> {
        for slot in slots {
            if let ManagedSlot::Reference(handle) = slot {
                self.release(heap, handle)?;
            }
        }
        Ok(())
    }

    fn json_owned_composite(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        variant: Option<u32>,
        slots: &[ManagedSlot],
    ) -> std::result::Result<ManagedSlot, DataError> {
        let result = self.create_composite(heap, ty, variant, slots);
        self.json_release(heap, slots.iter().copied())?;
        result.map(ManagedSlot::Reference)
    }

    fn json_rep_type(&self, ty: usize) -> Result<usize> {
        match self.value_schema()?.ty(ty)? {
            ValueType::State { fields, identity }
                if matches!(identity.as_str(), "rinzu.json::Value" | "Value") =>
            {
                match fields.as_slice() {
                    [rep] => Ok(*rep),
                    _ => Err(DataError::InvalidEncoding.into()),
                }
            }
            _ => Err(DataError::WrongKind.into()),
        }
    }

    fn json_value(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        tag: u32,
        slots: &[DecodedSlot],
    ) -> Result<ManagedSlot> {
        let representation = self.create_composite_decoded_hashed(
            heap,
            self.json_rep_type(ty)?,
            Some(tag),
            slots,
            |hash| hash,
        )?;
        Ok(self.json_owned_composite(heap, ty, None, &[ManagedSlot::Reference(representation)])?)
    }

    fn json_view(&self, heap: &DomainHeap, value: HeapRef) -> Result<Repr> {
        let rep = reference_field(heap, value, 2)?;
        let tag =
            u32::try_from(heap.integer_field(rep, 1)?).map_err(|_| DataError::InvalidEncoding)?;
        Ok(match tag {
            PARSED_BYTES | PARSED_STRING => Repr::Parsed {
                source: reference_field(heap, rep, 2)?,
                index: reference_field(heap, rep, 3)?,
                node: u32::try_from(heap.integer_field(rep, 4)?)
                    .map_err(|_| DataError::InvalidEncoding)?,
                tag,
            },
            NULL => Repr::Built { child: None, tag },
            BOOLEAN..=OBJECT => Repr::Built {
                child: Some(heap.field_at(rep, 2)?),
                tag,
            },
            _ => return Err(DataError::InvalidEncoding.into()),
        })
    }

    fn parsed_slots(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        source: HeapRef,
        index: HeapRef,
        node: u32,
        tag: u32,
    ) -> Result<ManagedSlot> {
        let source_kind = if tag == PARSED_STRING {
            DataKind::String
        } else {
            DataKind::Bytes(bytes(heap, source)?.len() as u64)
        };
        let index_kind = DataKind::Bytes(bytes(heap, index)?.len() as u64);
        self.json_value(
            heap,
            ty,
            tag,
            &[
                DecodedSlot::Reference(source, source_kind),
                DecodedSlot::Reference(index, index_kind),
                DecodedSlot::Value(int(i64::from(node))),
            ],
        )
    }

    fn json_child_value(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
        child: Child,
    ) -> Result<ManagedSlot> {
        match child {
            Child::Parsed(Repr::Parsed {
                source,
                index,
                node,
                tag,
            }) => self.parsed_slots(heap, ty, source, index, node, tag),
            Child::Built(reference) => Ok(ManagedSlot::Reference(self.root(
                heap,
                reference,
                DataKind::Composite(ty),
            )?)),
            _ => Err(DataError::InvalidEncoding.into()),
        }
    }

    fn json_query(&mut self, heap: &mut DomainHeap, call: &JsonCall<'_>) -> Result<ManagedSlot> {
        use DataOperation::*;
        let JsonCall {
            operation,
            result,
            types,
            slots,
        } = *call;
        let data = |slot| DecodedSlot::Value(slot);
        match operation {
            JsonNull => return self.json_value(heap, result, NULL, &[]),
            JsonConstruct | JsonNumber | JsonFloatValue => {
                let tag = match (operation, self.value_schema()?.ty(types[0])?) {
                    (JsonNumber, ValueType::String) => {
                        let (source, _) = self.any_reference(heap, reference(slots[0])?)?;
                        if !core::number_valid(bytes(heap, source)?) {
                            return Err(number_error(core::SYNTAX));
                        }
                        TOKEN
                    }
                    (JsonFloatValue, ValueType::Scalar(ScalarKind::F64)) => {
                        if !f64::from_bits(bits(slots[0])?).is_finite() {
                            return Err(number_error(core::NONFINITE));
                        }
                        FLOAT
                    }
                    (JsonConstruct, ValueType::Scalar(ScalarKind::Bool)) => BOOLEAN,
                    (JsonConstruct, ValueType::String) => TEXT,
                    (JsonConstruct, ValueType::Scalar(ScalarKind::I64)) => INTEGER,
                    (JsonConstruct, ValueType::List(_)) => ARRAY,
                    (JsonConstruct, ValueType::Map { .. }) => OBJECT,
                    _ => return Err(DataError::WrongKind.into()),
                };
                return self.json_value(heap, result, tag, &[data(slots[0])]);
            }
            _ => {}
        }
        self.json_rep_type(types[0])?;
        let (value, _) = self.any_reference(heap, reference(slots[0])?)?;
        let view = self.json_view(heap, value)?;
        let actual = view.kind(heap)?;
        if operation == JsonKind {
            return Ok(self.json_enum(heap, result, actual)?);
        }
        let expected = match operation {
            JsonField | JsonIntField | JsonFloatField | JsonEntryAt | JsonObjectLength => {
                core::OBJECT
            }
            JsonAt | JsonArrayLength => core::ARRAY,
            JsonBool => core::BOOLEAN,
            JsonString => core::TEXT,
            JsonInt | JsonFloat | JsonNumberText => core::NUMBER,
            _ => return Err(DataError::InvalidEncoding.into()),
        };
        expect_kind(expected, actual)?;
        if matches!(expected, core::ARRAY | core::OBJECT) {
            return self.json_container_query(heap, call, view);
        }
        if operation == JsonBool {
            let value = match view {
                Repr::Parsed {
                    source,
                    index,
                    node,
                    ..
                } => {
                    let at = document(heap, source, index)?.node(node).start as usize;
                    u64::from(bytes(heap, source)?[at] == b't')
                }
                Repr::Built { child, .. } => integer(child.as_ref())? as u64,
            };
            return Ok(scalar(ScalarKind::Bool, value));
        }
        if operation == JsonInt || operation == JsonFloat {
            return json_numeric(heap, view, operation == JsonFloat);
        }
        match view {
            Repr::Parsed {
                source,
                index,
                node,
                ..
            } => self.json_parsed_text(heap, source, index, node),
            Repr::Built {
                child: Some(HeapValue::Reference(reference)),
                ..
            } => Ok(ManagedSlot::Reference(self.root(
                heap,
                reference,
                DataKind::String,
            )?)),
            Repr::Built { child, tag } => {
                let mut output = [0; 32];
                let text = core::format(integer(child.as_ref())? as u64, tag == FLOAT, &mut output);
                Ok(self.json_string(heap, text)?)
            }
        }
    }

    fn json_parsed_text(
        &mut self,
        heap: &mut DomainHeap,
        source: HeapRef,
        index: HeapRef,
        node: u32,
    ) -> Result<ManagedSlot> {
        let text = document(heap, source, index)?.text(node);
        let length = text.len();
        let address = text.as_ptr().addr();
        let input = bytes(heap, source)?;
        let base = input.as_ptr().addr();
        let (reference, start) = if length == 0 {
            (source, 0)
        } else if (base..base + input.len()).contains(&address) {
            (source, address - base)
        } else {
            (index, address - bytes(heap, index)?.as_ptr().addr())
        };
        self.json_copy(heap, reference, start..start + length, true)
    }

    fn json_container_query(
        &mut self,
        heap: &mut DomainHeap,
        call: &JsonCall<'_>,
        view: Repr,
    ) -> Result<ManagedSlot> {
        use DataOperation::*;
        let JsonCall {
            operation,
            result,
            types,
            slots,
        } = *call;
        let by_key = matches!(operation, JsonField | JsonIntField | JsonFloatField);
        let index = if matches!(operation, JsonAt | JsonEntryAt) {
            u32::try_from(bits(slots[1])? as i64).ok()
        } else {
            None
        };
        let key = if by_key {
            let (reference, _) = self.any_reference(heap, reference(slots[1])?)?;
            bytes(heap, reference)?
        } else {
            &[]
        };
        let mut entry_key = None;
        let child = match view {
            Repr::Parsed {
                source,
                index: tape,
                node,
                tag,
            } => {
                let doc = document(heap, source, tape)?;
                if matches!(operation, JsonArrayLength | JsonObjectLength) {
                    return Ok(int(i64::from(doc.node(node).length)));
                }
                let found = if by_key {
                    doc.field(node, key)
                } else {
                    index.and_then(|i| doc.child(node, i))
                };
                found.map(|node| {
                    let node = if operation == JsonEntryAt {
                        entry_key = Some(Child::Parsed(Repr::Parsed {
                            source,
                            index: tape,
                            node,
                            tag,
                        }));
                        node + 1
                    } else {
                        node
                    };
                    Child::Parsed(Repr::Parsed {
                        source,
                        index: tape,
                        node,
                        tag,
                    })
                })
            }
            Repr::Built {
                child: Some(HeapValue::Reference(collection)),
                tag,
            } => {
                let stride = if tag == OBJECT { 2 } else { 1 };
                let count = (heap.header(collection)?.field_count as usize - 2) / stride;
                if matches!(operation, JsonArrayLength | JsonObjectLength) {
                    return Ok(int(count as i64));
                }
                let selected = if by_key {
                    let mut found = None;
                    for i in 0..count {
                        if key == bytes(heap, reference_field(heap, collection, 2 + i * 2)?)? {
                            found = Some(i);
                            break;
                        }
                    }
                    found
                } else {
                    index.map(|i| i as usize).filter(|i| *i < count)
                };
                if let Some(i) = selected {
                    if operation == JsonEntryAt {
                        entry_key =
                            Some(Child::Built(reference_field(heap, collection, 2 + i * 2)?));
                    }
                    Some(Child::Built(reference_field(
                        heap,
                        collection,
                        2 + i * stride + stride - 1,
                    )?))
                } else {
                    None
                }
            }
            _ => return Err(DataError::WrongKind.into()),
        };
        let output = if let Some(child) = child {
            Some(match operation {
                JsonIntField | JsonFloatField => {
                    let view = match child {
                        Child::Parsed(view) => view,
                        Child::Built(value) => self.json_view(heap, value)?,
                    };
                    expect_kind(core::NUMBER, view.kind(heap)?)?;
                    json_numeric(heap, view, operation == JsonFloatField)?
                }
                JsonEntryAt => {
                    let key = match entry_key.ok_or(DataError::InvalidEncoding)? {
                        Child::Parsed(Repr::Parsed {
                            source,
                            index,
                            node,
                            ..
                        }) => self.json_parsed_text(heap, source, index, node)?,
                        Child::Built(key) => {
                            ManagedSlot::Reference(self.root(heap, key, DataKind::String)?)
                        }
                        _ => return Err(DataError::InvalidEncoding.into()),
                    };
                    let value = self.json_child_value(heap, types[0], child)?;
                    self.json_owned_composite(
                        heap,
                        self.json_option_child(result)?,
                        None,
                        &[key, value],
                    )?
                }
                JsonAt | JsonField => self.json_child_value(heap, types[0], child)?,
                _ => return Err(DataError::InvalidEncoding.into()),
            })
        } else {
            None
        };
        Ok(self.json_option_owned(heap, result, output)?)
    }
}

fn json_numeric(heap: &DomainHeap, view: Repr, float: bool) -> Result<ManagedSlot> {
    let bits = match view {
        Repr::Parsed {
            source,
            index,
            node,
            ..
        } => core::number(document(heap, source, index)?.text(node), float),
        Repr::Built {
            tag: TOKEN,
            child: Some(HeapValue::Reference(token)),
        } => core::number(bytes(heap, token)?, float),
        Repr::Built { tag: FLOAT, child } if float => Ok(integer(child.as_ref())? as u64),
        Repr::Built { tag: FLOAT, .. } => Err(core::INTEGER),
        Repr::Built {
            tag: INTEGER,
            child,
        } if float => Ok((integer(child.as_ref())? as f64).to_bits()),
        Repr::Built {
            tag: INTEGER,
            child,
        } => Ok(integer(child.as_ref())? as u64),
        _ => return Err(DataError::WrongKind.into()),
    }
    .map_err(number_error)?;
    Ok(scalar(
        if float {
            ScalarKind::F64
        } else {
            ScalarKind::I64
        },
        bits,
    ))
}

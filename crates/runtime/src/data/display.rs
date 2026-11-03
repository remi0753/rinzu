//! Shallow Owner-heap reads for the common iterative display engine.
use super::*;
use crate::scalar::ScalarKind;
use crate::value::ValueType;

pub const C_SOURCE: &str = include_str!("../display.c");

#[derive(Clone, Debug)]
pub enum DisplayType {
    Scalar(ScalarKind),
    Unit,
    String,
    Bytes,
    Record {
        name: String,
        fields: Vec<(String, usize)>,
    },
    Enum {
        name: String,
        variants: Vec<(String, Vec<(String, usize)>)>,
    },
    List(usize),
    Map {
        key: usize,
        value: usize,
    },
    /// Already formatted type label; its value is never inspected.
    Marker(String),
    Error,
}
#[derive(Clone, Debug)]
pub struct DisplaySchema(pub Vec<DisplayType>);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Text {
    bytes: *const u8,
    length: usize,
}
impl Text {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.as_ptr(),
            length: bytes.len(),
        }
    }
}
#[repr(C)]
struct Field {
    name: Text,
    ty: usize,
}
#[repr(C)]
struct Variant {
    name: Text,
    fields: *const Field,
    count: usize,
}
#[repr(C)]
#[derive(Default)]
struct Layout {
    kind: u32,
    scalar: u32,
    name: Text,
    fields: *const Field,
    count: usize,
    variants: *const Variant,
    element: usize,
    key: usize,
    value: usize,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Slot {
    bits: u64,
    reference: u32,
}
#[repr(C)]
struct Node {
    text: Text,
    fields: *const Slot,
    count: usize,
    variant: usize,
}
struct Catalog {
    _fields: Vec<Vec<Vec<Field>>>,
    _variants: Vec<Vec<Variant>>,
    layouts: Vec<Layout>,
}

#[cfg(unix)]
unsafe extern "C" {
    fn rinzu_display_format(
        types: *const Layout,
        count: usize,
        ty: usize,
        slot: Slot,
        nested: i32,
        load: unsafe extern "C" fn(*mut std::ffi::c_void, usize, u64, *mut Node) -> i32,
        context: *mut std::ffi::c_void,
        bytes: *mut *mut u8,
        length: *mut usize,
    ) -> i32;
}

struct Reader<'a> {
    data: &'a ManagedData,
    heap: &'a DomainHeap,
    references: Vec<HeapRef>,
    bytes: Vec<u8>,
    fields: Vec<Slot>,
    error: Option<DataError>,
}
impl Reader<'_> {
    fn slot(&mut self, field: HeapValue) -> Slot {
        match field {
            HeapValue::Integer(bits) => Slot {
                bits: bits as u64,
                reference: 0,
            },
            HeapValue::Null => Slot {
                bits: 0,
                reference: 0,
            },
            HeapValue::Reference(reference) => {
                let bits = self.references.len() as u64;
                self.references.push(reference);
                Slot { bits, reference: 1 }
            }
        }
    }
    fn read(&mut self, ty: usize, bits: u64) -> Result<Node, DataError> {
        let reference = *self
            .references
            .get(usize::try_from(bits).map_err(|_| DataError::InvalidEncoding)?)
            .ok_or(DataError::InvalidEncoding)?;
        self.bytes.clear();
        self.fields.clear();
        let mut variant = 0;
        match self.data.value_schema()?.ty(ty)? {
            ValueType::Error | ValueType::ErrorRecord { .. } => {
                self.bytes = self
                    .data
                    .error_message_at(self.heap, reference)?
                    .into_bytes();
            }
            ValueType::String => {
                self.bytes = ManagedData::string_at(self.heap, reference)?.into_bytes()
            }
            ValueType::DynamicBytes => {
                self.bytes = ManagedData::bytes_at(
                    self.heap,
                    reference,
                    self.heap.integer_field(reference, 0)? as u64,
                )?
            }
            ValueType::Bytes(length) => {
                self.bytes = ManagedData::bytes_at(self.heap, reference, *length)?
            }
            ValueType::Record { .. }
            | ValueType::State { .. }
            | ValueType::Enum { .. }
            | ValueType::List(_)
            | ValueType::Map { .. } => {
                self.data
                    .require_native_record_readable(self.heap, reference)?;
                let (tag, fields) = self.data.composite_fields(self.heap, reference, ty)?;
                variant = tag.map_or(0, |tag| tag as usize);
                for field in fields {
                    let slot = self.slot(field);
                    self.fields.push(slot);
                }
            }
            ValueType::Other if self.heap.header(reference)?.type_id == ERROR_TYPE => {
                let fields = self.heap.fields(reference)?;
                let Some(HeapValue::Reference(message)) = fields.first() else {
                    return Err(DataError::InvalidEncoding);
                };
                self.bytes = ManagedData::string_at(self.heap, *message)?.into_bytes();
            }
            _ => return Err(DataError::WrongKind),
        }
        Ok(Node {
            text: Text::new(&self.bytes),
            fields: self.fields.as_ptr(),
            count: self.fields.len(),
            variant,
        })
    }
}
unsafe extern "C" fn load(
    raw: *mut std::ffi::c_void,
    ty: usize,
    bits: u64,
    node: *mut Node,
) -> i32 {
    // SAFETY: the synchronous formatter borrows this Reader exclusively; all
    // exposed scratch slices are copied or consumed before the next callback.
    let reader = unsafe { &mut *raw.cast::<Reader<'_>>() };
    match reader.read(ty, bits) {
        Ok(value) => {
            // SAFETY: the formatter passes its live output node for this read.
            unsafe { node.write(value) };
            0
        }
        Err(error) => {
            reader.error = Some(error);
            1
        }
    }
}

impl DisplaySchema {
    fn validate(&self, values: &crate::value::ValueSchema) -> Result<(), DataError> {
        if self.0.len() != values.type_count() {
            return Err(DataError::InvalidEncoding);
        }
        for (index, ty) in self.0.iter().enumerate() {
            let valid = match (ty, values.ty(index)?) {
                (DisplayType::Scalar(a), ValueType::Scalar(b)) => a == b,
                (DisplayType::Unit, ValueType::Unit)
                | (DisplayType::String, ValueType::String)
                | (DisplayType::Bytes, ValueType::Bytes(_) | ValueType::DynamicBytes)
                | (DisplayType::Error, ValueType::Error | ValueType::ErrorRecord { .. })
                | (DisplayType::Marker(_), ValueType::State { .. })
                | (DisplayType::Marker(_) | DisplayType::Error, ValueType::Other) => true,
                (
                    DisplayType::Record { name, fields },
                    ValueType::Record {
                        identity,
                        fields: types,
                    }
                    | ValueType::State {
                        identity,
                        fields: types,
                    },
                ) => name == identity && fields.iter().map(|(_, ty)| ty).eq(types),
                (
                    DisplayType::Enum { name, variants },
                    ValueType::Enum {
                        identity,
                        variants: types,
                    },
                ) => {
                    name == identity
                        && variants.len() == types.len()
                        && variants
                            .iter()
                            .zip(types)
                            .all(|((_, fields), types)| fields.iter().map(|(_, ty)| ty).eq(types))
                }
                (DisplayType::List(a), ValueType::List(b)) => a == b,
                (DisplayType::Map { key: a, value: b }, ValueType::Map { key: c, value: d }) => {
                    a == c && b == d
                }
                _ => false,
            };
            if !valid {
                return Err(DataError::InvalidEncoding);
            }
        }
        Ok(())
    }
    fn layouts(&self) -> Catalog {
        let groups: Vec<Vec<Vec<Field>>> = self
            .0
            .iter()
            .map(|ty| {
                let fields: Vec<&Vec<(String, usize)>> = match ty {
                    DisplayType::Record { fields, .. } => vec![fields],
                    DisplayType::Enum { variants, .. } => {
                        variants.iter().map(|(_, fields)| fields).collect()
                    }
                    _ => vec![],
                };
                fields
                    .into_iter()
                    .map(|fields| {
                        fields
                            .iter()
                            .map(|(name, ty)| Field {
                                name: Text::new(name.as_bytes()),
                                ty: *ty,
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect();
        let variants: Vec<Vec<Variant>> = self
            .0
            .iter()
            .enumerate()
            .map(|(index, ty)| match ty {
                DisplayType::Enum { variants, .. } => variants
                    .iter()
                    .enumerate()
                    .map(|(tag, (name, _))| Variant {
                        name: Text::new(name.as_bytes()),
                        fields: groups[index][tag].as_ptr(),
                        count: groups[index][tag].len(),
                    })
                    .collect(),
                _ => vec![],
            })
            .collect();
        let layouts = self
            .0
            .iter()
            .enumerate()
            .map(|(index, ty)| {
                let mut layout = Layout::default();
                layout.kind = match ty {
                    DisplayType::Scalar(kind) => {
                        layout.scalar = *kind as u32;
                        0
                    }
                    DisplayType::Unit => 1,
                    DisplayType::String => 2,
                    DisplayType::Bytes => 3,
                    DisplayType::Record { name, .. } => {
                        layout.name = Text::new(name.as_bytes());
                        layout.fields = groups[index][0].as_ptr();
                        layout.count = groups[index][0].len();
                        4
                    }
                    DisplayType::Enum { name, .. } => {
                        layout.name = Text::new(name.as_bytes());
                        layout.variants = variants[index].as_ptr();
                        layout.count = variants[index].len();
                        5
                    }
                    DisplayType::List(element) => {
                        layout.element = *element;
                        6
                    }
                    DisplayType::Map { key, value } => {
                        layout.key = *key;
                        layout.value = *value;
                        7
                    }
                    DisplayType::Marker(text) => {
                        layout.name = Text::new(text.as_bytes());
                        8
                    }
                    DisplayType::Error => 9,
                };
                layout
            })
            .collect();
        Catalog {
            _fields: groups,
            _variants: variants,
            layouts,
        }
    }
}

impl ManagedData {
    /// Joins typed display values into one managed String without performing I/O.
    pub fn format_string(
        &mut self,
        heap: &mut DomainHeap,
        schema: &DisplaySchema,
        parts: &[(usize, ManagedSlot)],
    ) -> Result<ManagedHandle, DataError> {
        let mut bytes = Vec::new();
        for &(ty, slot) in parts {
            let part = self.display(heap, schema, ty, slot)?;
            bytes
                .try_reserve(part.len())
                .unwrap_or_else(|_| crate::allocation::fatal());
            bytes.extend_from_slice(&part);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| DataError::InvalidEncoding)?;
        self.create_string(heap, text)
    }

    /// Returns owned native bytes while every managed source remains rooted.
    /// No recursive snapshot, managed allocation, or extra managed roots.
    pub fn display(
        &self,
        heap: &DomainHeap,
        schema: &DisplaySchema,
        ty: usize,
        slot: ManagedSlot,
    ) -> Result<Vec<u8>, DataError> {
        self.require_domain(heap)?;
        schema.validate(self.value_schema()?)?;
        let field = self.slot_field(heap, ty, slot)?;
        let mut reader = Reader {
            data: self,
            heap,
            references: vec![],
            bytes: vec![],
            fields: vec![],
            error: None,
        };
        let slot = reader.slot(field);
        let catalog = schema.layouts();
        let mut bytes = std::ptr::null_mut();
        let mut length = 0;
        #[cfg(unix)]
        // SAFETY: all catalog and scratch storage stays alive until return.
        let status = unsafe {
            rinzu_display_format(
                catalog.layouts.as_ptr(),
                catalog.layouts.len(),
                ty,
                slot,
                0,
                load,
                (&mut reader as *mut Reader<'_>).cast(),
                &mut bytes,
                &mut length,
            )
        };
        #[cfg(not(unix))]
        let status = {
            let _ = (catalog, slot, load);
            1
        };
        if status != 0 {
            return Err(reader.error.unwrap_or(DataError::InvalidEncoding));
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(length)
            .unwrap_or_else(|_| crate::allocation::fatal());
        if length != 0 {
            // SAFETY: successful formatting returns a live malloc buffer of
            // exactly length bytes. Copy before releasing the C allocation.
            output.extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, length) });
        }
        // SAFETY: the successful formatter transfers this malloc allocation.
        unsafe { libc::free(bytes.cast()) };
        Ok(output)
    }
}

//! Borrowed slices and owned workspaces for the shared C JSON scanner.
use crate::resource_accounting::{self as accounting, Kind};
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

pub const NULL: u32 = 0;
pub const BOOLEAN: u32 = 1;
pub const NUMBER: u32 = 2;
pub const TEXT: u32 = 3;
pub const ARRAY: u32 = 4;
pub const OBJECT: u32 = 5;

pub const DECODE: u32 = 1;
pub const INVALID_LIMIT: u32 = 2;
pub const LIMIT: u32 = 3;
pub const TYPE: u32 = 4;
pub const NUMBER_ERROR: u32 = 5;
pub const SCHEMA: u32 = 6;
pub const INPUT: u32 = 0;
pub const DEPTH: u32 = 1;
pub const NODES: u32 = 2;
pub const OUTPUT: u32 = 3;
pub const SYNTAX: u32 = 0;
pub const INTEGER: u32 = 1;
pub const RANGE: u32 = 2;
pub const NONFINITE: u32 = 3;
pub const NONE: u32 = u32::MAX;

pub const BOOL_TY: u32 = 1;
pub const INT_TY: u32 = 2;
pub const FLOAT_TY: u32 = 3;
pub const STRING_TY: u32 = 4;
pub const LIST_TY: u32 = 5;
pub const MAP_TY: u32 = 6;
pub const RECORD_TY: u32 = 7;
pub const OPTION_TY: u32 = 8;
pub const SCALAR: u32 = 0;
pub const OPEN: u32 = 1;
pub const KEY: u32 = 2;
pub const CLOSE: u32 = 3;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Limits {
    pub bytes: i64,
    pub depth: i64,
    pub nodes: i64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: 128 * 1024 * 1024,
            depth: 1024,
            nodes: 16 * 1024 * 1024,
        }
    }
}
impl Limits {
    pub fn validate(self, kind: u32) -> Result<Self, Error> {
        let mut error = RawError::default();
        // SAFETY: the result pointer refers to writable storage for this call.
        if unsafe { rinzu_json_limits(self, kind, &mut error) } == 0 {
            Err(error.owned())
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Slice {
    data: *const u8,
    length: usize,
}
impl Slice {
    fn from(bytes: &[u8]) -> Self {
        Self {
            data: bytes.as_ptr(),
            length: bytes.len(),
        }
    }

    unsafe fn get<'a>(self) -> &'a [u8] {
        if self.length == 0 {
            return &[];
        }
        // SAFETY: the caller ties the slice to the live storage of its producer.
        unsafe { std::slice::from_raw_parts(self.data, self.length) }
    }
}

#[derive(Debug, Default)]
pub struct Error {
    pub code: u32,
    pub reason: u32,
    pub offset: u32,
    pub first: u32,
    pub expected: u32,
    pub actual: u32,
    pub limit: i64,
    pub supplied: i64,
    pub minimum: i64,
    pub maximum: i64,
    pub path: Vec<u8>,
    pub key: Option<Vec<u8>>,
}

#[derive(Default)]
#[repr(C)]
struct RawError {
    code: u32,
    reason: u32,
    offset: u32,
    first: u32,
    expected: u32,
    actual: u32,
    limit: i64,
    supplied: i64,
    minimum: i64,
    maximum: i64,
    path: Slice,
    key: Slice,
}

impl RawError {
    fn owned(&self) -> Error {
        // SAFETY: a diagnostic is read before dropping the parser, schema, or
        // the borrowed input that owns its text. Limits diagnostics have no text.
        let path = unsafe { self.path.get() }.to_vec();
        // SAFETY: a non-NULL key is a diagnostic slice with the same lifetime.
        let key = (!self.key.data.is_null()).then(|| unsafe { self.key.get() }.to_vec());
        Error {
            code: self.code,
            reason: self.reason,
            offset: self.offset,
            first: self.first,
            expected: self.expected,
            actual: self.actual,
            limit: self.limit,
            supplied: self.supplied,
            minimum: self.minimum,
            maximum: self.maximum,
            path,
            key,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Type {
    pub kind: u32,
    pub child: u32,
    pub first: u32,
    pub count: u32,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Field {
    pub name: u32,
    pub length: u32,
    pub ty: u32,
}
#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub types: Vec<Type>,
    pub fields: Vec<Field>,
    pub names: Vec<u8>,
    pub errors: [u32; 6],
}
#[repr(C)]
struct RawSchema {
    types: *const Type,
    fields: *const Field,
    names: *const u8,
    root: u32,
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Event {
    pub bits: u64,
    pub event: u32,
    pub kind: u32,
    pub ty: u32,
    pub field: u32,
    start: u32,
    length: u32,
    pub scalars: u32,
    decoded: u32,
}
#[repr(C)]
pub(crate) struct Batch {
    events: [Event; 256],
    decoded: Slice,
    count: usize,
}
impl Default for Batch {
    fn default() -> Self {
        Self {
            events: [Event::default(); 256],
            decoded: Slice::default(),
            count: 0,
        }
    }
}
pub(crate) struct Progress<'a> {
    pub done: bool,
    pub events: &'a [Event],
    pub decoded: &'a [u8],
}
impl Event {
    pub fn is_decoded(self) -> bool {
        self.decoded != 0
    }
    pub fn text_range(self) -> std::ops::Range<usize> {
        let start = self.start as usize;
        start..start + self.length as usize
    }
}

#[repr(C)]
struct RawParser {
    _private: [u8; 0],
}

pub(crate) struct Parser<'a> {
    raw: NonNull<RawParser>,
    length: usize,
    accounted: usize,
    _schema: PhantomData<&'a Schema>,
}
impl<'a> Parser<'a> {
    pub(crate) fn new(length: usize, limits: Limits, typed: Option<(&'a Schema, usize)>) -> Self {
        let schema = typed.map(|(schema, root)| RawSchema {
            types: schema.types.as_ptr(),
            fields: schema.fields.as_ptr(),
            names: schema.names.as_ptr(),
            root: u32::try_from(root).expect("JSON schema id"),
        });
        // SAFETY: the C parser copies RawSchema while the borrowed tables
        // remain available for its entire lifetime.
        let raw = unsafe {
            rinzu_json_parser_new(
                length,
                limits,
                schema.as_ref().map_or(ptr::null(), ptr::from_ref),
            )
        };
        let mut parser = Self {
            raw: NonNull::new(raw).expect("JSON parser"),
            length,
            accounted: 0,
            _schema: PhantomData,
        };
        parser.account();
        parser
    }

    fn account(&mut self) {
        if !accounting::enabled() && self.accounted == 0 {
            return;
        }
        // SAFETY: the parser has sole ownership of its native workspace.
        let bytes = unsafe { rinzu_json_scratch_bytes(self.raw.as_ptr()) };
        account(bytes, &mut self.accounted);
    }

    pub(crate) fn step<'s>(
        &'s mut self,
        source: &[u8],
        mut batch: Option<&'s mut Batch>,
    ) -> Result<Progress<'s>, Error> {
        assert_eq!(source.len(), self.length);
        // SAFETY: the input and optional output batch are live for the call.
        // The parser saves offsets only. Error text is copied before returning.
        let result = unsafe {
            rinzu_json_step(
                self.raw.as_ptr(),
                source.as_ptr(),
                batch.as_deref_mut().map_or(ptr::null_mut(), ptr::from_mut),
            )
        };
        self.account();
        match result {
            0 | 1 => {
                let (events, decoded) = match batch {
                    // SAFETY: decoded storage is retained by the borrowed
                    // parser and cannot move until the next step.
                    Some(batch) => (&batch.events[..batch.count], unsafe { batch.decoded.get() }),
                    None => (&[][..], &[][..]),
                };
                Ok(Progress {
                    done: result == 0,
                    events,
                    decoded,
                })
            }
            // SAFETY: a failed parser has a diagnostic tied to these inputs.
            2 => Err(unsafe { &*rinzu_json_error(self.raw.as_ptr()) }.owned()),
            _ => unreachable!("JSON parser status"),
        }
    }

    pub(crate) fn index(&mut self) -> Buffer {
        // SAFETY: called after a successful untyped parse. The returned buffer
        // is newly owned by the caller, independent of the parser.
        let buffer = Buffer::new(unsafe { rinzu_json_index(self.raw.as_ptr()) });
        self.account();
        buffer
    }
}
impl Drop for Parser<'_> {
    fn drop(&mut self) {
        // SAFETY: a unique non-NULL parser is disposed exactly once.
        unsafe { rinzu_json_parser_free(self.raw.as_ptr()) };
        account(0, &mut self.accounted);
    }
}

fn account(bytes: usize, current: &mut usize) {
    if bytes < *current {
        accounting::release(Kind::RuntimeControl, (*current - bytes) as u64);
    } else if bytes > *current
        && !accounting::acquire(Kind::RuntimeControl, (bytes - *current) as u64)
    {
        return;
    }
    *current = bytes;
}

#[repr(C)]
struct RawBuffer {
    data: *mut u8,
    length: usize,
    capacity: usize,
    moved: usize,
}
#[derive(Default)]
pub(crate) struct Buffer {
    raw: RawBuffer,
    accounted: usize,
}
impl Default for RawBuffer {
    fn default() -> Self {
        Self {
            data: ptr::null_mut(),
            length: 0,
            capacity: 0,
            moved: 0,
        }
    }
}
impl Buffer {
    fn new(raw: RawBuffer) -> Self {
        let mut buffer = Self { raw, accounted: 0 };
        buffer.account();
        buffer
    }
    fn account(&mut self) {
        account(self.raw.capacity, &mut self.accounted);
    }
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: the initialized prefix belongs to this native buffer.
        unsafe {
            Slice {
                data: self.raw.data,
                length: self.raw.length,
            }
            .get()
        }
    }
    pub fn append(&mut self, limit: usize, bytes: &[u8], quote: bool) -> bool {
        // SAFETY: input is live and separate from this writable buffer.
        let written = unsafe {
            if quote {
                rinzu_json_escape(&mut self.raw, limit, Slice::from(bytes))
            } else {
                rinzu_json_append(&mut self.raw, limit, Slice::from(bytes))
            }
        };
        self.account();
        written != 0
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: native code frees only its own allocation.
        unsafe { rinzu_json_buffer_free(&mut self.raw) };
        account(0, &mut self.accounted);
    }
}

#[repr(C)]
struct RawDocument {
    index: *const u8,
    source: Slice,
    regions: [u32; 6],
}
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Node {
    pub kind: u32,
    pub start: u32,
    pub end: u32,
    pub depth: u32,
    pub nodes: u32,
    pub length: u32,
}

pub(crate) struct Document<'a> {
    raw: RawDocument,
    _input: PhantomData<&'a [u8]>,
}
impl<'a> Document<'a> {
    pub fn new(source: &'a [u8], index: &'a [u8]) -> Self {
        let mut raw = std::mem::MaybeUninit::uninit();
        // SAFETY: both slices refer to an original validated input and its
        // matching private index, and the C call initializes the output view.
        unsafe { rinzu_json_document(raw.as_mut_ptr(), Slice::from(source), Slice::from(index)) };
        // SAFETY: the core initialized all fields of the view.
        let raw = unsafe { raw.assume_init() };
        Self {
            raw,
            _input: PhantomData,
        }
    }
    pub fn node(&self, node: u32) -> Node {
        // SAFETY: node is a private parser-produced index into this view.
        unsafe { rinzu_json_node(&self.raw, node) }
    }
    pub fn child(&self, node: u32, index: u32) -> Option<u32> {
        // SAFETY: callers check that node names a container.
        let child = unsafe { rinzu_json_child(&self.raw, node, index) };
        (child != NONE).then_some(child)
    }
    pub fn field(&self, node: u32, key: &[u8]) -> Option<u32> {
        // SAFETY: callers check that node names an object.
        let child = unsafe { rinzu_json_find(&self.raw, node, Slice::from(key)) };
        (child != NONE).then_some(child)
    }
    pub fn text(&self, node: u32) -> &'a [u8] {
        // SAFETY: callers use text/number/key nodes, and both possible result
        // storages are borrowed for the lifetime of the document.
        unsafe { rinzu_json_text(&self.raw, node).get() }
    }
}
pub(crate) fn number_valid(text: &[u8]) -> bool {
    // SAFETY: the slice is live through the scan.
    unsafe { rinzu_json_number_valid(Slice::from(text)) != 0 }
}
pub(crate) fn number(text: &[u8], float: bool) -> Result<u64, u32> {
    let mut bits = 0;
    // SAFETY: the input is a validated JSON number and the result slot is writable.
    let result = unsafe {
        if float {
            rinzu_json_float(Slice::from(text), &mut bits)
        } else {
            rinzu_json_int(Slice::from(text), &mut bits)
        }
    };
    if result < 0 {
        Ok(bits)
    } else {
        Err(result as u32)
    }
}
pub(crate) fn format(bits: u64, float: bool, out: &mut [u8; 32]) -> &[u8] {
    // SAFETY: the core's formatters write at most 32 bytes to this slot.
    let length = unsafe {
        if float {
            rinzu_json_format_float(bits, out.as_mut_ptr())
        } else {
            rinzu_json_format_int(bits, out.as_mut_ptr())
        }
    };
    &out[..length]
}

unsafe extern "C" {
    fn rinzu_json_limits(limits: Limits, kind: u32, error: *mut RawError) -> i32;
    fn rinzu_json_parser_new(
        length: usize,
        limits: Limits,
        schema: *const RawSchema,
    ) -> *mut RawParser;
    fn rinzu_json_parser_free(parser: *mut RawParser);
    fn rinzu_json_step(parser: *mut RawParser, bytes: *const u8, batch: *mut Batch) -> i32;
    fn rinzu_json_error(parser: *const RawParser) -> *const RawError;
    fn rinzu_json_index(parser: *mut RawParser) -> RawBuffer;
    fn rinzu_json_scratch_bytes(parser: *const RawParser) -> usize;
    fn rinzu_json_document(doc: *mut RawDocument, source: Slice, index: Slice);
    fn rinzu_json_node(doc: *const RawDocument, node: u32) -> Node;
    fn rinzu_json_child(doc: *const RawDocument, node: u32, index: u32) -> u32;
    fn rinzu_json_find(doc: *const RawDocument, node: u32, key: Slice) -> u32;
    fn rinzu_json_text(doc: *const RawDocument, node: u32) -> Slice;
    fn rinzu_json_number_valid(token: Slice) -> i32;
    fn rinzu_json_int(token: Slice, bits: *mut u64) -> i32;
    fn rinzu_json_float(token: Slice, bits: *mut u64) -> i32;
    fn rinzu_json_format_int(bits: u64, out: *mut u8) -> usize;
    fn rinzu_json_format_float(bits: u64, out: *mut u8) -> usize;
    fn rinzu_json_buffer_free(buffer: *mut RawBuffer);
    fn rinzu_json_append(buffer: *mut RawBuffer, limit: usize, text: Slice) -> i32;
    fn rinzu_json_escape(buffer: *mut RawBuffer, limit: usize, text: Slice) -> i32;
}

//! Immutable error snapshots, distinct from pending execution failure.
use crate::failure::{Diagnostic, ErrorKind, Failure, FailureCause, SourcePosition};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorNode {
    pub code: i64,
    pub message: String,
    pub context: String,
    pub source: Option<SourcePosition>,
    pub native_status: Option<i64>,
    pub cause: Option<usize>,
    pub cleanup: Vec<usize>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorValue {
    pub nodes: Vec<ErrorNode>,
}
impl ErrorValue {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            nodes: vec![ErrorNode {
                code,
                message: message.into(),
                context: String::new(),
                source: None,
                native_status: None,
                cause: None,
                cleanup: Vec::new(),
            }],
        }
    }
    pub fn root(&self) -> &ErrorNode {
        self.nodes.last().expect("validated Error")
    }
    pub fn with_context(&self, context: String) -> Self {
        let mut value = self.clone();
        value.nodes.last_mut().unwrap().context = context;
        value
    }
    pub fn with_cause(&self, cause: &Self) -> Result<Self, Self> {
        if self
            .nodes
            .len()
            .checked_add(cause.nodes.len())
            .is_none_or(|n| n > 64)
        {
            return Err(Self::new(101, "Error cause graph exceeds 64 nodes"));
        }
        let mut nodes = self.nodes[..self.nodes.len() - 1].to_vec();
        let offset = nodes.len();
        nodes.extend(cause.nodes.iter().cloned().map(|mut node| {
            node.cause = node.cause.map(|n| n + offset);
            for n in &mut node.cleanup {
                *n += offset;
            }
            node
        }));
        let mut root = self.root().clone();
        root.cause = Some(nodes.len() - 1);
        nodes.push(root);
        Ok(Self { nodes })
    }
    pub fn prefix(&self, index: usize) -> Self {
        Self {
            nodes: self.nodes[..=index].to_vec(),
        }
    }
    pub fn from_failure(failure: &Failure) -> Self {
        let mut nodes: Vec<_> = failure.cleanup.iter().map(Self::diagnostic).collect();
        let mut root = Self::diagnostic(&failure.primary);
        root.cleanup = (0..nodes.len()).collect();
        for diagnostic in &failure.cleanup {
            root.message.push_str("; cleanup: ");
            root.message.push_str(diagnostic.cause.message());
        }
        nodes.push(root);
        Self { nodes }
    }
    fn diagnostic(d: &Diagnostic) -> ErrorNode {
        let code = match d.cause {
            FailureCause::Error(ErrorKind::Conversion) => 1,
            FailureCause::Error(ErrorKind::NullNativeValue) => 2,
            FailureCause::Error(ErrorKind::InvalidUtf8) => 3,
            FailureCause::Error(ErrorKind::NativeRecordBusy) => 4,
            FailureCause::Error(ErrorKind::UnobservedChildFailure) => 5,
            FailureCause::Error(ErrorKind::Io) => 6,
            FailureCause::Error(ErrorKind::SupplantBusy) => 7,
            FailureCause::Error(ErrorKind::UnboundValue) => 8,
            FailureCause::Error(ErrorKind::User) => 9,
            _ => unreachable!("only recoverable failures become Error values"),
        };
        ErrorNode {
            code,
            message: d.cause.message().into(),
            context: String::new(),
            source: d.source,
            native_status: None,
            cause: None,
            cleanup: Vec::new(),
        }
    }
    pub fn validate(&self) -> bool {
        !self.nodes.is_empty()
            && self.nodes.len() <= u32::MAX as usize
            && self.nodes.iter().enumerate().all(|(i, n)| {
                n.cause.is_none_or(|child| child < i)
                    && n.cleanup.iter().all(|child| *child < i)
                    && n.cleanup.len() <= u32::MAX as usize
            })
    }
    pub fn encode(&self) -> Vec<u8> {
        assert!(self.validate());
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(self.nodes.len() as u32).to_le_bytes());
        for n in &self.nodes {
            bytes.extend_from_slice(&n.code.to_le_bytes());
            for text in [&n.message, &n.context] {
                bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
                bytes.extend_from_slice(text.as_bytes());
            }
            bytes.push(u8::from(n.source.is_some()));
            let s = n.source.unwrap_or(SourcePosition {
                file: 0,
                start_byte: 0,
                end_byte: 0,
            });
            bytes.extend_from_slice(&s.file.to_le_bytes());
            bytes.extend_from_slice(&s.start_byte.to_le_bytes());
            bytes.extend_from_slice(&s.end_byte.to_le_bytes());
            bytes.push(u8::from(n.native_status.is_some()));
            bytes.extend_from_slice(&n.native_status.unwrap_or(0).to_le_bytes());
            bytes.extend_from_slice(&n.cause.map_or(u32::MAX, |i| i as u32).to_le_bytes());
            bytes.extend_from_slice(&(n.cleanup.len() as u32).to_le_bytes());
            for child in &n.cleanup {
                bytes.extend_from_slice(&(*child as u32).to_le_bytes());
            }
        }
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        struct Cursor<'a>(&'a [u8]);
        impl Cursor<'_> {
            fn take(&mut self, n: usize) -> Option<&[u8]> {
                let (head, tail) = self.0.split_at_checked(n)?;
                self.0 = tail;
                Some(head)
            }
            fn u32(&mut self) -> Option<u32> {
                Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
            }
            fn u64(&mut self) -> Option<u64> {
                Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
            }
            fn flag(&mut self) -> Option<bool> {
                match self.take(1)?[0] {
                    0 => Some(false),
                    1 => Some(true),
                    _ => None,
                }
            }
            fn text(&mut self) -> Option<String> {
                let n = usize::try_from(self.u64()?).ok()?;
                Some(std::str::from_utf8(self.take(n)?).ok()?.to_owned())
            }
        }
        let mut c = Cursor(bytes);
        let count = c.u32()? as usize;
        if count == 0 || count > c.0.len() / 58 {
            return None;
        }
        let mut nodes = Vec::with_capacity(count);
        for _ in 0..count {
            let code = c.u64()? as i64;
            let message = c.text()?;
            let context = c.text()?;
            let source_present = c.flag()?;
            let source = SourcePosition {
                file: c.u64()?,
                start_byte: c.u32()?,
                end_byte: c.u32()?,
            };
            if !source_present
                && source
                    != (SourcePosition {
                        file: 0,
                        start_byte: 0,
                        end_byte: 0,
                    })
            {
                return None;
            }
            let status_present = c.flag()?;
            let status = c.u64()? as i64;
            if !status_present && status != 0 {
                return None;
            }
            let cause = c.u32()?;
            let count = c.u32()? as usize;
            if count > c.0.len() / 4 {
                return None;
            }
            let cleanup = (0..count)
                .map(|_| c.u32().map(|i| i as usize))
                .collect::<Option<Vec<_>>>()?;
            nodes.push(ErrorNode {
                code,
                message,
                context,
                source: source_present.then_some(source),
                native_status: status_present.then_some(status),
                cause: (cause != u32::MAX).then_some(cause as usize),
                cleanup,
            });
        }
        let value = Self { nodes };
        (c.0.is_empty() && value.validate()).then_some(value)
    }
}

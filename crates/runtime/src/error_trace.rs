//! Optional diagnostic snapshots contain names and positions, never frames,
//! locals, managed handles, or executable addresses.
use crate::failure::SourcePosition;
use std::sync::{Arc, Mutex};

/// Rendering is a report-time operation. It uses only compiler metadata and
/// never opens source files or keeps managed execution state alive.
pub fn format_failure(
    failure: &crate::failure::Failure,
    sources: &crate::data::SourceCatalog,
) -> String {
    use std::fmt::Write;
    let mut output = String::new();
    for (index, trace) in &failure.traces {
        let Some(diagnostic) = (if *index == 0 {
            Some(&failure.primary)
        } else {
            failure.cleanup.get(index - 1)
        }) else {
            continue;
        };
        writeln!(output, "error[{index}]: {}", diagnostic.cause.message()).unwrap();
        if trace.frames.is_empty() {
            output.push_str("  (no managed stack recorded)\n");
        }
        for frame in trace.frames.iter() {
            write!(output, "  at {}", frame.function.escape_debug()).unwrap();
            if let Some(position) = frame.source {
                if let Some((file, (line, column))) = sources.files.get(&position.file).zip(
                    sources
                        .positions
                        .get(&position.file)
                        .and_then(|positions| positions.get(&position.start_byte)),
                ) {
                    write!(output, " ({}:{line}:{column})", file.escape_debug()).unwrap();
                } else {
                    write!(
                        output,
                        " (file {}:{}:{})",
                        position.file, position.start_byte, position.end_byte
                    )
                    .unwrap();
                }
            }
            output.push('\n');
        }
    }
    output
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceFrame {
    pub function: Arc<str>,
    pub source: Option<SourcePosition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorTrace {
    /// Innermost frame first, frozen at the original failure.
    pub frames: Arc<[TraceFrame]>,
}

#[derive(Clone, Debug, Default)]
pub struct TraceContext(Option<Arc<Mutex<TraceStack>>>);

impl TraceContext {
    pub fn new(enabled: bool) -> Self {
        Self(enabled.then(|| Arc::new(Mutex::new(TraceStack::new(true)))))
    }
    pub fn from_environment(enabled: bool) -> Self {
        Self::new(
            enabled || std::env::var_os("RINZU_ERROR_TRACE").is_some_and(|value| value == "1"),
        )
    }
    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }
    pub fn source(&self, source: Option<SourcePosition>) {
        if let Some(stack) = &self.0 {
            stack
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .source(source);
        }
    }
    pub fn capture(&self, source: Option<SourcePosition>) -> Option<Arc<ErrorTrace>> {
        self.0
            .as_ref()?
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .capture(source)
    }
    pub fn enter(&self, function: &str, source: Option<SourcePosition>) -> TraceGuard {
        self.enter_inner(function, source, false)
    }
    /// A task records its own active stack. It never reconstructs a completed
    /// async caller or includes an unrelated task that resumes on this Owner.
    pub fn task(&self, function: &str, source: Option<SourcePosition>) -> TraceGuard {
        self.enter_inner(function, source, true)
    }
    fn enter_inner(
        &self,
        function: &str,
        source: Option<SourcePosition>,
        task: bool,
    ) -> TraceGuard {
        let Some(stack) = &self.0 else {
            return TraceGuard {
                stack: None,
                token: None,
                suspended: None,
            };
        };
        let mut active = stack.lock().unwrap_or_else(|p| p.into_inner());
        let suspended = task.then(|| std::mem::take(&mut active.frames));
        let token = active.enter(function, source);
        TraceGuard {
            stack: Some(Arc::clone(stack)),
            token,
            suspended,
        }
    }
}

pub struct TraceGuard {
    stack: Option<Arc<Mutex<TraceStack>>>,
    token: Option<usize>,
    suspended: Option<Vec<TraceFrame>>,
}
impl Drop for TraceGuard {
    fn drop(&mut self) {
        if let Some(stack) = &self.stack {
            let mut active = stack.lock().unwrap_or_else(|p| p.into_inner());
            if !active.leave(self.token) {
                std::process::abort();
            }
            if let Some(suspended) = self.suspended.take() {
                if !active.frames.is_empty() {
                    std::process::abort();
                }
                active.frames = suspended;
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct TraceStack {
    enabled: bool,
    frames: Vec<TraceFrame>,
}

impl TraceStack {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            frames: Vec::new(),
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn enter(&mut self, function: &str, source: Option<SourcePosition>) -> Option<usize> {
        if !self.enabled {
            return None;
        }
        let depth = self.frames.len();
        self.frames.push(TraceFrame {
            function: Arc::from(function),
            source,
        });
        Some(depth)
    }
    pub fn leave(&mut self, token: Option<usize>) -> bool {
        let Some(depth) = token else {
            return !self.enabled;
        };
        if depth.checked_add(1) != Some(self.frames.len()) {
            return false;
        }
        self.frames.pop();
        true
    }
    pub fn source(&mut self, source: Option<SourcePosition>) {
        if let Some(frame) = self.frames.last_mut() {
            frame.source = source;
        }
    }
    pub fn capture(&self, source: Option<SourcePosition>) -> Option<Arc<ErrorTrace>> {
        if !self.enabled {
            return None;
        }
        let mut frames = self.frames.iter().rev().cloned().collect::<Vec<_>>();
        if let Some(frame) = frames.first_mut() {
            frame.source = source;
        }
        Some(Arc::new(ErrorTrace {
            frames: frames.into(),
        }))
    }
    pub fn depth(&self) -> usize {
        self.frames.len()
    }
}

//! Text diagnostics use the inputs from the check that produced each range.
use std::cell::RefCell;
use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};

use rinzu_compiler::{Diagnostic, FileId, SourceFile, SourceRange, SourceSnapshot};

#[derive(Clone, Copy)]
pub(crate) enum Input<'a> {
    Source(&'a SourceFile),
    Bytes(&'a Path, Option<&'a [u8]>),
}

impl<'a> Input<'a> {
    pub(crate) fn path(self) -> &'a Path {
        match self {
            Self::Source(file) => file.path(),
            Self::Bytes(path, _) => path,
        }
    }

    pub(crate) fn position(self, offset: usize) -> Option<(usize, usize)> {
        match self {
            Self::Source(file) => (file.original_offset(0)
                ..=file.original_offset(file.text().len()))
                .contains(&offset)
                .then(|| file.line_column(offset)),
            Self::Bytes(_, bytes) => {
                let bytes = bytes?;
                let bom = usize::from(bytes.starts_with(b"\xef\xbb\xbf")) * 3;
                let prefix = std::str::from_utf8(bytes.get(bom..offset)?).ok()?;
                let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
                let column = prefix.rsplit('\n').next()?.chars().count() + 1;
                Some((line, column))
            }
        }
    }

    fn line(self, number: usize) -> Option<&'a str> {
        let raw = match self {
            Self::Source(file) => file.text().split('\n').nth(number - 1)?,
            Self::Bytes(_, bytes) => {
                let bytes = bytes?;
                let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
                std::str::from_utf8(bytes.split(|&byte| byte == b'\n').nth(number - 1)?).ok()?
            }
        };
        Some(raw.strip_suffix('\r').unwrap_or(raw))
    }
}

pub(crate) fn snapshot_input<'a>(
    snapshot: &'a SourceSnapshot<'_>,
    id: FileId,
) -> Option<Input<'a>> {
    let path = snapshot.path_for_id(id)?;
    Some(
        snapshot
            .sources()
            .get(path)
            .map(Input::Source)
            .unwrap_or_else(|| Input::Bytes(path, snapshot.bytes_for_path(path))),
    )
}

pub(crate) struct Renderer {
    cwd: PathBuf,
}

impl Renderer {
    pub(crate) fn new() -> Self {
        Self::at(std::env::current_dir().unwrap_or_default())
    }

    pub(crate) fn at(cwd: PathBuf) -> Self {
        Self { cwd }
    }

    pub(crate) fn path(&self, path: &Path) -> String {
        if path
            .to_str()
            .is_some_and(|name| name.starts_with('<') && name.ends_with('>'))
        {
            return path.display().to_string();
        }
        let absolute = self.cwd.join(path);
        let mut normalized = PathBuf::new();
        for part in absolute.components() {
            match part {
                Component::ParentDir if normalized.has_root() => {
                    normalized.pop();
                }
                _ => normalized.push(part),
            }
        }
        let display = normalized.strip_prefix(&self.cwd).unwrap_or(&normalized);
        if display.as_os_str().is_empty() {
            ".".into()
        } else {
            display.display().to_string()
        }
    }

    fn location(
        &self,
        output: &mut String,
        path: &Path,
        line: Option<usize>,
        column: Option<usize>,
    ) {
        output.push_str(&self.path(path));
        if let Some(line) = line {
            write!(output, ":{line}").unwrap();
            if let Some(column) = column {
                write!(output, ":{column}").unwrap();
            }
        }
        output.push_str(": ");
    }

    fn range(&self, output: &mut String, input: Input<'_>, range: SourceRange) {
        let position = input.position(range.start_byte as usize);
        self.location(
            output,
            input.path(),
            position.map(|p| p.0),
            position.map(|p| p.1),
        );
    }

    pub(crate) fn render<'a>(
        &self,
        diagnostic: &Diagnostic,
        resolve: impl Fn(FileId) -> Option<Input<'a>>,
    ) -> String {
        let mut output = String::new();
        let input = diagnostic
            .primary_path()
            .is_none()
            .then(|| resolve(diagnostic.primary.file_id))
            .flatten();
        if let Some(path) = diagnostic.primary_path() {
            self.location(&mut output, path, None, None);
        } else if let Some(input) = input {
            self.range(&mut output, input, diagnostic.primary);
        }
        writeln!(
            output,
            "{}: {}",
            diagnostic.code.as_str(),
            diagnostic.message
        )
        .unwrap();
        if let Some(input) = input {
            excerpt(&mut output, input, diagnostic.primary);
        }
        for (range, message) in &diagnostic.secondary {
            output.push_str("  note: ");
            if let Some(input) = resolve(range.file_id) {
                self.range(&mut output, input, *range);
            }
            writeln!(output, "{message}").unwrap();
        }
        for suggestion in &diagnostic.suggestions {
            output.push_str("  help: ");
            if let Some((range, _)) = &suggestion.replacement
                && let Some(input) = resolve(range.file_id)
            {
                self.range(&mut output, input, *range);
            }
            writeln!(output, "{}", suggestion.message).unwrap();
        }
        output
    }

    pub(crate) fn stage(
        &self,
        path: Option<&Path>,
        stage: &str,
        tool: Option<&str>,
        detail: &str,
    ) -> String {
        let mut output = String::new();
        if let Some(path) = path {
            self.location(&mut output, path, None, None);
        }
        write!(output, "error: {stage} failed").unwrap();
        if let Some(tool) = tool {
            write!(output, " ({tool})").unwrap();
        }
        output.push('\n');
        for line in detail.lines() {
            writeln!(output, "  {line}").unwrap();
        }
        output
    }
}

fn excerpt(output: &mut String, input: Input<'_>, range: SourceRange) {
    const WIDTH: usize = 120;
    let Some((number, column)) = input.position(range.start_byte as usize) else {
        return;
    };
    let Some(line) = input.line(number) else {
        return;
    };
    let length = line.chars().count();
    let start = (column - 1).min(length);
    let left = if length > WIDTH {
        start.saturating_sub(WIDTH / 3).min(length - WIDTH)
    } else {
        0
    };
    let right = (left + WIDTH).min(length);
    let byte = |column| {
        line.char_indices()
            .nth(column)
            .map_or(line.len(), |(i, _)| i)
    };
    let before = if left == 0 { "" } else { "..." };
    let after = if right == length { "" } else { "..." };
    writeln!(
        output,
        "  {number} | {before}{}{after}",
        &line[byte(left)..byte(right)]
    )
    .unwrap();
    let Some((end_line, end_column)) = input.position(range.end_byte as usize) else {
        return;
    };
    let end = if number == end_line {
        end_column - 1
    } else {
        length
    };
    let end = end.clamp(start, right);
    if line.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        let width = (end - start).max(1);
        let padding = number.to_string().len() + 3;
        let indent = before.len() + start - left;
        writeln!(
            output,
            "{:padding$}| {:indent$}{}",
            "",
            "",
            "^".repeat(width)
        )
        .unwrap();
    } else if start < end {
        writeln!(
            output,
            "  range: {:?}{}",
            &line[byte(start)..byte(end)],
            if end == right && right < length && (end_line > number || end_column - 1 > right) {
                "..."
            } else {
                ""
            }
        )
        .unwrap();
    }
    let last = end_line - usize::from(end_line > number && end_column == 1);
    if last > number {
        writeln!(output, "  range continues through line {last}").unwrap();
    }
}

pub(crate) fn write_block(block: &str) {
    CAPTURE.with(|capture| {
        if let Some(blocks) = capture.borrow_mut().as_mut() {
            blocks.push(block.to_owned());
        } else {
            let _ = io::stderr().lock().write_all(block.as_bytes());
        }
    });
}

thread_local! {
    static CAPTURE: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Verification workers keep compiler diagnostics separate from program stderr.
pub(crate) fn capture<T>(action: impl FnOnce() -> T) -> (T, Vec<String>) {
    struct Restore(Option<Vec<String>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CAPTURE.replace(self.0.take());
        }
    }
    let _restore = Restore(CAPTURE.replace(Some(Vec::new())));
    let result = action();
    (result, CAPTURE.take().unwrap())
}

pub(crate) fn emit<'a>(errors: &[Diagnostic], resolve: impl Fn(FileId) -> Option<Input<'a>>) {
    let renderer = Renderer::new();
    for error in errors {
        write_block(&renderer.render(error, &resolve));
    }
}

pub(crate) fn emit_snapshot(errors: &[Diagnostic], snapshot: &SourceSnapshot<'_>) {
    emit(errors, |id| snapshot_input(snapshot, id));
}

pub(crate) fn emit_file(errors: &[Diagnostic], file: &SourceFile) {
    emit(errors, |id| {
        (id == file.id()).then_some(Input::Source(file))
    });
}

pub(crate) fn file_failure(path: &Path, message: String, help: &str) -> String {
    emit(
        &[Diagnostic::error(
            rinzu_compiler::DiagnosticCode::PackageIo,
            SourceRange::empty(FileId::MANIFEST, 0),
            message,
            help,
        )
        .with_path(path)],
        |_| None,
    );
    "input could not be read".into()
}

/// Capture the external tool's own positions and details in one diagnostic block.
pub(crate) fn tool_output(
    result: io::Result<std::process::Output>,
    path: Option<&Path>,
    stage: &str,
    tool: &str,
) -> Result<std::process::Output, String> {
    let detail = match result {
        Ok(output) if output.status.success() => return Ok(output),
        Ok(output) if output.stderr.is_empty() => {
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        Ok(output) => String::from_utf8_lossy(&output.stderr).into_owned(),
        Err(error) => error.to_string(),
    };
    Err(stage_failure(path, stage, Some(tool), &detail))
}

pub(crate) fn stage_failure(
    path: Option<&Path>,
    stage: &str,
    tool: Option<&str>,
    detail: &str,
) -> String {
    write_block(&Renderer::new().stage(path, stage, tool, detail));
    "compilation failed".into()
}

#[cfg(test)]
mod tests;

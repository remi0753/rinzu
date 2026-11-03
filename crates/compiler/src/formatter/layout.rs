//! Small document renderer: delimiters and argument groups choose flat or indented layout.

pub(super) enum Doc<'a> {
    Text(&'a str),
    Line {
        flat: &'static str,
        extra_indent: usize,
    },
    HardLine,
    BlankLine,
    Concat(Vec<Self>),
    Nest(usize, Box<Self>),
    Group {
        body: Box<Self>,
        anchored: bool,
        break_nested: bool,
    },
}

impl<'a> Doc<'a> {
    pub(super) fn line(flat: &'static str, extra_indent: usize) -> Self {
        Self::Line { flat, extra_indent }
    }

    pub(super) fn group(parts: Vec<Self>, anchored: bool) -> Self {
        Self::Group {
            body: Box::new(Self::Concat(parts)),
            anchored,
            break_nested: !anchored,
        }
    }

    pub(super) fn terms(parts: Vec<Self>) -> Self {
        Self::Group {
            body: Box::new(Self::Concat(parts)),
            anchored: false,
            break_nested: false,
        }
    }

    fn has_hard_line(&self) -> bool {
        match self {
            Self::HardLine | Self::BlankLine => true,
            Self::Concat(parts) => parts.iter().any(Self::has_hard_line),
            Self::Nest(_, body) | Self::Group { body, .. } => body.has_hard_line(),
            Self::Text(_) | Self::Line { .. } => false,
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Flat,
    Broken,
}

type Command<'a, 'doc> = (usize, Mode, &'doc Doc<'a>);

fn fits<'a>(remaining: usize, mut commands: Vec<Command<'a, '_>>, nested_can_break: bool) -> bool {
    let mut remaining = remaining;
    while let Some((indent, mode, document)) = commands.pop() {
        match document {
            Doc::Text(text) => {
                let first_line = text.split('\n').next().unwrap_or("");
                let width = first_line.chars().count();
                if width > remaining {
                    return false;
                }
                remaining -= width;
                if text.contains('\n') {
                    return true;
                }
            }
            Doc::Line { flat, .. } => {
                if matches!(mode, Mode::Broken) {
                    return true;
                }
                if flat.len() > remaining {
                    return false;
                }
                remaining -= flat.len();
            }
            Doc::HardLine | Doc::BlankLine => return true,
            Doc::Concat(parts) => {
                commands.extend(parts.iter().rev().map(|part| (indent, mode, part)))
            }
            Doc::Nest(extra, body) => commands.push((indent + extra, mode, body)),
            Doc::Group { body, anchored, .. } => {
                if nested_can_break && *anchored {
                    let probe = vec![(indent, Mode::Flat, body.as_ref())];
                    if !fits(remaining, probe, false) {
                        let mut probe = commands;
                        probe.push((indent, Mode::Broken, body));
                        return fits(remaining, probe, false);
                    }
                }
                commands.push((indent, Mode::Flat, body));
            }
        }
    }
    true
}

pub(super) fn render(document: &Doc<'_>, width: usize) -> String {
    let mut output = String::new();
    let mut column = 0;
    let mut line_indent = 0;
    let mut pending_indent = 0;
    let mut commands = vec![(0, Mode::Broken, document)];
    while let Some((indent, mode, document)) = commands.pop() {
        match document {
            Doc::Text(text) => {
                if text.is_empty() {
                    continue;
                }
                if output.is_empty() || output.ends_with('\n') {
                    output.push_str(&" ".repeat(pending_indent));
                    column = pending_indent;
                    line_indent = pending_indent;
                }
                output.push_str(text);
                if let Some((_, last)) = text.rsplit_once('\n') {
                    column = last.chars().count();
                } else {
                    column += text.chars().count();
                }
            }
            Doc::Line { flat, extra_indent } => {
                // A nested delimiter may break while its enclosing expression stays flat.
                // Recheck the suffix on the delimiter's closing line before keeping an operator.
                let fits_suffix = *extra_indent == 0
                    || flat.len() <= width.saturating_sub(column)
                        && fits(
                            width.saturating_sub(column + flat.len()),
                            commands.clone(),
                            true,
                        );
                if matches!(mode, Mode::Flat) && fits_suffix {
                    output.push_str(flat);
                    column += flat.len();
                } else {
                    newline(&mut output);
                    pending_indent = indent + extra_indent;
                    column = pending_indent;
                }
            }
            Doc::HardLine => {
                newline(&mut output);
                pending_indent = indent;
                column = pending_indent;
            }
            Doc::BlankLine => {
                newline(&mut output);
                if !output.is_empty() && !output.ends_with("\n\n") {
                    output.push('\n');
                }
                pending_indent = indent;
                column = pending_indent;
            }
            Doc::Concat(parts) => {
                commands.extend(parts.iter().rev().map(|part| (indent, mode, part)))
            }
            Doc::Nest(extra, body) => commands.push((indent + extra, mode, body)),
            Doc::Group {
                body,
                anchored,
                break_nested,
            } => {
                let indent = if *anchored {
                    indent.max(if output.is_empty() || output.ends_with('\n') {
                        pending_indent
                    } else {
                        line_indent
                    })
                } else {
                    indent
                };
                let mut probe = commands.clone();
                probe.push((indent, Mode::Flat, body));
                let mode = if !(*anchored && body.has_hard_line())
                    && fits(width.saturating_sub(column), probe, *break_nested)
                {
                    Mode::Flat
                } else {
                    Mode::Broken
                };
                commands.push((indent, mode, body));
            }
        }
    }
    newline(&mut output);
    output
}

fn newline(output: &mut String) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

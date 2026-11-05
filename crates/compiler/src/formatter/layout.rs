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

    pub(super) fn remaining_after_flat(&self, remaining: usize) -> Option<usize> {
        match self {
            Self::Text(text) if !text.contains('\n') => remaining.checked_sub(text.chars().count()),
            Self::Line { flat, .. } => remaining.checked_sub(flat.len()),
            Self::Concat(parts) => parts.iter().try_fold(remaining, |remaining, part| {
                part.remaining_after_flat(remaining)
            }),
            Self::Nest(_, body) | Self::Group { body, .. } => body.remaining_after_flat(remaining),
            Self::Text(_) | Self::HardLine | Self::BlankLine => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Flat,
    // Recheck continuation points after rendering nested groups.
    Flexible,
    Broken,
}

// Breaks available to the first-line fit check, in addition to mandatory breaks.
enum Breaks {
    Fixed,
    Delimiters,
    Continuations,
}

type Command<'a, 'doc> = (usize, Mode, &'doc Doc<'a>);

fn fits<'a>(mut remaining: usize, mut commands: Vec<Command<'a, '_>>, breaks: Breaks) -> bool {
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
            Doc::Line { flat, extra_indent } => {
                if matches!(mode, Mode::Broken)
                    || matches!(mode, Mode::Flexible)
                        && *extra_indent != 0
                        && matches!(breaks, Breaks::Continuations)
                {
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
                let mode = if matches!(mode, Mode::Flat) {
                    Mode::Flat
                } else if *anchored && !matches!(breaks, Breaks::Fixed) {
                    Mode::Broken
                } else {
                    Mode::Flexible
                };
                commands.push((indent, mode, body));
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
                // A nested group may have wrapped. Measure the remaining space
                // on its closing line before keeping a continuation point inline.
                let keep_inline = match mode {
                    Mode::Flat => true,
                    Mode::Flexible => {
                        *extra_indent == 0
                            || flat.len() <= width.saturating_sub(column)
                                && fits(
                                    width.saturating_sub(column + flat.len()),
                                    commands.clone(),
                                    Breaks::Delimiters,
                                )
                    }
                    Mode::Broken => false,
                };
                if keep_inline {
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
                if matches!(mode, Mode::Flat) {
                    commands.push((indent, mode, body));
                    continue;
                }
                let indent = if *anchored {
                    indent.max(if output.is_empty() || output.ends_with('\n') {
                        pending_indent
                    } else {
                        line_indent
                    })
                } else {
                    indent
                };
                let remaining = width.saturating_sub(column);
                let mode = if *anchored {
                    // Keep the whole group on one line or break its own delimiters.
                    // Leave later delimiters and continuation points available
                    // so an overflowing suffix can wrap independently.
                    if body
                        .remaining_after_flat(remaining)
                        .is_some_and(|remaining| {
                            fits(remaining, commands.clone(), Breaks::Continuations)
                        })
                    {
                        Mode::Flat
                    } else {
                        Mode::Broken
                    }
                } else {
                    let mut probe = commands.clone();
                    probe.push((indent, Mode::Flexible, body));
                    let breaks = if *break_nested {
                        Breaks::Delimiters
                    } else {
                        Breaks::Fixed
                    };
                    if fits(remaining, probe, breaks) {
                        Mode::Flexible
                    } else {
                        Mode::Broken
                    }
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

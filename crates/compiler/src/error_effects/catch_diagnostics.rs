//! Diagnostics for the residual Error language. Replay evidence never feeds
//! back into catch coverage or the inferred function summaries.
use super::*;
use language::{
    Count, Mismatch, Origin, OriginKind, Part, PartKind, Reception, Sequence, Shape,
    VISIBLE_ERRORS, Word,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const COVER_ALL: &str =
    "cover every possible ordered list, or add a whole-list catch and handle its Errors";

pub(super) fn pending(range: SourceRange, remaining: &Language) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::NonExhaustiveCatch,
        range,
        format!(
            "catch patterns do not cover the possible Error list {}",
            display_word(remaining.first().unwrap())
        ),
        COVER_ALL,
    )
}

pub(crate) fn shadowed_by_binding(handler: &CatchClause, earlier: &CatchClause) -> Diagnostic {
    unreachable(handler, std::slice::from_ref(earlier), vec![(0, None)])
}

pub(super) fn unreachable(
    handler: &CatchClause,
    earlier: &[CatchClause],
    proof: Vec<(usize, Option<Shape>)>,
) -> Diagnostic {
    let mut covering = BTreeMap::<_, Vec<_>>::new();
    for (index, shape) in proof {
        covering.entry(index).or_default().extend(shape);
    }
    let mut diagnostic = Diagnostic::error(
        DiagnosticCode::InvalidCatchPattern,
        handler.pattern.range(),
        "this catch is unreachable because earlier patterns cover every list it can match",
        "move the more specific catch first if it should handle these failures",
    );
    for (index, shapes) in covering {
        let pattern = &earlier[index].pattern;
        let message = if !shapes.is_empty() {
            let lengths = shapes
                .iter()
                .map(|shape| {
                    count_text(if shape.longer {
                        Count::AtLeast(shape.length + 1)
                    } else {
                        Count::Exact(shape.length)
                    })
                })
                .collect::<Vec<_>>()
                .join(" or ");
            format!("this catch covers the matching lists with {lengths}")
        } else if match pattern {
            CatchPattern::Binding(_) => true,
            CatchPattern::List { elements, .. } => elements.is_empty(),
        } {
            "this whole-list catch already matches those lists".into()
        } else {
            "this earlier pattern already matches all those lists".into()
        };
        diagnostic = diagnostic.with_secondary(pattern.range(), message);
    }
    diagnostic
}

pub(super) struct Replay {
    targets: HashSet<SourceRange>,
    catches: HashMap<SourceRange, Catch>,
    receptions: HashMap<SourceRange, Arc<Reception>>,
}

struct Catch {
    remaining: Language,
    patterns: Vec<Pattern>,
    clauses: Vec<(SourceRange, CatchPattern)>,
}

impl Replay {
    pub fn new(targets: HashSet<SourceRange>) -> Self {
        Self {
            targets,
            catches: HashMap::new(),
            receptions: HashMap::new(),
        }
    }

    pub fn has_targets(&self) -> bool {
        !self.targets.is_empty()
    }

    pub fn targets(&self, range: SourceRange) -> bool {
        self.targets.contains(&range)
    }

    pub fn record(
        &mut self,
        range: SourceRange,
        remaining: &Language,
        handlers: &[CatchClause],
        patterns: Vec<Pattern>,
    ) {
        self.catches
            .entry(range)
            .and_modify(|catch| catch.remaining.extend(remaining.clone()))
            .or_insert_with(|| Catch {
                remaining: remaining.clone(),
                patterns,
                clauses: handlers
                    .iter()
                    .map(|handler| (handler.range, handler.pattern.clone()))
                    .collect(),
            });
    }

    pub fn direct(&mut self, at: SourceRange, callee: &FunctionDecl) -> Arc<Reception> {
        self.receptions
            .entry(at)
            .or_insert_with(|| {
                Arc::new(Reception {
                    label: format!("{}()", contracts::display_identity(&callee.name)),
                    callee: callee.error_contract.is_none().then(|| callee.name.clone()),
                    contract: callee
                        .error_contract
                        .as_ref()
                        .map(|contract| contract.range),
                    fresh_task: callee.is_async,
                })
            })
            .clone()
    }

    pub fn callback(
        &mut self,
        checked: &CheckedModule,
        at: SourceRange,
        name: &str,
    ) -> Arc<Reception> {
        self.receptions
            .entry(at)
            .or_insert_with(|| {
                Arc::new(Reception {
                    label: format!(
                        "{}()",
                        checked
                            .indirect_fields
                            .get(&at)
                            .map_or(name, String::as_str)
                    ),
                    callee: None,
                    contract: checked
                        .indirect_signatures
                        .get(&at)
                        .and_then(|call| call.contract),
                    fresh_task: false,
                })
            })
            .clone()
    }

    pub fn diagnostic(
        &self,
        range: SourceRange,
        mut expand: impl FnMut(&str, &Word) -> Option<Sequence>,
    ) -> Option<Diagnostic> {
        let catch = self.catches.get(&range)?;
        let covered = |word: &Word| {
            catch
                .patterns
                .iter()
                .any(|pattern| pattern.matches(word) != Match::Never)
        };
        let selected = best_sequence(catch.remaining.iter().filter_map(|word| {
            let witness = catch.remaining.sequence(word)?;
            (word.contains(&witness.word) && !covered(&witness.word)).then_some(witness)
        }));
        let word = selected.map_or_else(|| catch.remaining.first().unwrap(), |seq| &seq.word);
        let evidence = selected.map(|seq| Shown::new(seq, &mut expand));
        let display = evidence
            .as_ref()
            .filter(|evidence| evidence.count.is_some())
            .map_or_else(|| display_word(word), |evidence| evidence.list.clone());
        let count = evidence
            .as_ref()
            .map_or(Some(Count::of(word)), |evidence| evidence.count);
        let contractual = selected.is_some_and(|seq| {
            seq.parts.iter().any(|part| {
            matches!(&part.kind, PartKind::Received(_, Some(site)) if site.contract.is_some())
        })
        });
        let (index, mismatch) =
            catch
                .patterns
                .iter()
                .enumerate()
                .find_map(|(index, pattern)| {
                    pattern.mismatch(word).map(|mismatch| (index, mismatch))
                })?;
        let clause = &catch.clauses[index].1;
        let prefix = evidence
            .as_ref()
            .map_or(&[][..], |evidence| evidence.prefix.as_slice());
        let (at, reason) = explain(clause, mismatch, word, prefix, count, &display, contractual);
        let multiple = catch.clauses.len() > 1;
        let mut diagnostic = if multiple {
            let mut keyword = catch.clauses[0].0;
            keyword.end_byte = keyword.start_byte + "catch".len() as u32;
            let message = if covered(word) {
                format!("the catch patterns do not cover every Error list shaped like {display}")
            } else {
                format!("no catch pattern covers the possible Error list {display}")
            };
            Diagnostic::error(
                DiagnosticCode::NonExhaustiveCatch,
                keyword,
                message,
                COVER_ALL,
            )
            .with_secondary(at, reason)
        } else {
            let mut help = help(clause, &catch.patterns[index], mismatch, word);
            if catch.remaining.len() > 1 {
                help.push_str(". Cover the other possible Error lists as well");
            }
            Diagnostic::error(DiagnosticCode::NonExhaustiveCatch, at, reason, help)
        };
        if let Some(evidence) = evidence {
            diagnostic.secondary.extend(evidence.receipts);
            diagnostic.secondary.extend(evidence.details);
        } else if let Some(origins) = catch.remaining.origins(word) {
            let origin = origins
                .prefix
                .iter()
                .flatten()
                .copied()
                .next()
                .or_else(|| origins.tail.values().copied().next());
            if let Some(origin) = origin {
                diagnostic = diagnostic.with_secondary(
                    origin.range,
                    match self.receptions.get(&origin.range) {
                        Some(site) => format!(
                            "`{}` can propagate an Error list shaped like {display} here",
                            site.label
                        ),
                        None => "the Error list can reach this catch from here".into(),
                    },
                );
                if let Some(contract) = self
                    .receptions
                    .get(&origin.range)
                    .and_then(|site| site.contract)
                {
                    diagnostic = diagnostic
                        .with_secondary(contract, "the Error type upper bound is declared here");
                }
            }
        }
        Some(diagnostic)
    }
}

pub(super) fn best_sequence<'a>(
    sequences: impl Iterator<Item = &'a Sequence>,
) -> Option<&'a Sequence> {
    sequences.reduce(|best, next| if next.precedes(best) { next } else { best })
}

fn explain(
    clause: &CatchPattern,
    mismatch: Mismatch,
    word: &Word,
    prefix: &[Tag],
    count: Option<Count>,
    display: &str,
    contractual: bool,
) -> (SourceRange, String) {
    let CatchPattern::List {
        elements,
        rest,
        range,
    } = clause
    else {
        unreachable!("a whole-list catch covers every list")
    };
    let required = elements.len();
    match mismatch {
        Mismatch::TooLong => {
            let explanation = match count {
                Some(count) if contractual => {
                    format!("the Error bound permits lists of {}", count_text(count))
                }
                Some(count) => format!("this path can produce {}", count_text(count)),
                None => format!("the analysis leaves longer lists shaped like {display}"),
            };
            (
                *range,
                format!(
                    "this catch requires exactly {}, and {explanation}",
                    count_text(Count::Exact(required))
                ),
            )
        }
        Mismatch::TooShort => (
            *range,
            format!(
                "this catch requires {}{}, and the possible list {display} has {}",
                if rest.is_some() {
                    "at least "
                } else {
                    "exactly "
                },
                count_text(Count::Exact(required)),
                count_text(Count::of(word))
            ),
        ),
        Mismatch::Type(index) | Mismatch::Open(index) => {
            let ty = &elements[index].ty;
            let expected = &elements[index].type_spelling;
            let actual = display_tag(prefix.get(index).unwrap_or(&word.prefix[index]));
            let position = index + 1;
            let message = if matches!(mismatch, Mismatch::Open(_)) {
                format!(
                    "the type of Error {position} in {display} is only known to be {actual}. The catch expects {expected} at that position"
                )
            } else {
                format!(
                    "Error {position} is {actual} in the possible list {display}. The catch expects {expected} at that position"
                )
            };
            (ty.range, message)
        }
    }
}

fn help(clause: &CatchPattern, pattern: &Pattern, mismatch: Mismatch, word: &Word) -> String {
    match mismatch {
        Mismatch::TooLong if (Pattern { prefix: pattern.prefix.clone(), rest: true }).matches(word) == Match::All => {
            let CatchPattern::List { elements, .. } = clause else { unreachable!() };
            let mut suffix = "rest".to_owned();
            while elements.iter().any(|e| e.binding.as_ref().is_some_and(|b| b.name == suffix)) {
                suffix.push('_');
            }
            let mut pieces = elements.iter().map(|element| {
                let ty = &element.type_spelling;
                element.binding.as_ref().map_or_else(|| ty.to_string(), |b| format!("{}: {ty}", b.name))
            }).collect::<Vec<_>>();
            pieces.push(format!("...{suffix}"));
            format!("match all Errors explicitly, or use `[{}]` and handle the remaining Errors", pieces.join(", "))
        }
        Mismatch::TooShort => "add a catch for the shorter list, or bind the whole list and handle its Errors".into(),
        Mismatch::Open(_) => "accept `Error` at that position, or bind the whole list and handle the possible Error types".into(),
        _ => COVER_ALL.into(),
    }
}

fn count_text(count: Count) -> String {
    let (prefix, n) = match count {
        Count::Exact(n) => ("", n),
        Count::AtLeast(n) => ("at least ", n),
    };
    format!("{prefix}{n} Error{}", if n == 1 { "" } else { "s" })
}

pub(super) fn display_tag(tag: &Tag) -> String {
    match tag {
        Tag::Named(name) => contracts::display_identity(name),
        Tag::Set(names) => names
            .iter()
            .map(|name| contracts::display_identity(name))
            .collect::<Vec<_>>()
            .join(" or "),
        Tag::Any | Tag::Parameter(_) | Tag::FutureElement(..) => "Error".into(),
    }
}

fn display_word(word: &Word) -> String {
    let mut list = word
        .prefix
        .iter()
        .take(VISIBLE_ERRORS)
        .map(display_tag)
        .collect::<Vec<_>>();
    if word.longer || word.prefix.len() > VISIBLE_ERRORS {
        list.push("...".into());
    }
    format!("[{}]", list.join(", "))
}

struct Shown {
    count: Option<Count>,
    list: String,
    prefix: Vec<Tag>,
    receipts: Vec<(SourceRange, String)>,
    details: Vec<(SourceRange, String)>,
}

struct DisplayParts {
    shown: Shown,
    index: Option<usize>,
    tags: Vec<String>,
}

impl Shown {
    fn new(seq: &Sequence, expand: &mut impl FnMut(&str, &Word) -> Option<Sequence>) -> Self {
        let expanded = seq
            .parts
            .iter()
            .map(|part| {
                let PartKind::Received(word, Some(site)) = &part.kind else {
                    return None;
                };
                expand(site.callee.as_deref()?, word)
            })
            .collect::<Vec<_>>();
        let count = if seq.count.is_some() && !seq.omitted && expanded.iter().any(Option::is_some) {
            seq.parts
                .iter()
                .zip(&expanded)
                .try_fold(Count::Exact(0), |total, (part, inner)| {
                    Some(total.add(if let Some(inner) = inner {
                        inner.count?
                    } else {
                        part_count(part)?
                    }))
                })
        } else {
            seq.count
        };
        let mut display = DisplayParts {
            shown: Self {
                count,
                list: String::new(),
                prefix: Vec::new(),
                receipts: Vec::new(),
                details: Vec::new(),
            },
            index: Some(0),
            tags: Vec::new(),
        };
        display.parts(seq, &expanded, None);
        display.shown.list = if display.tags.is_empty() {
            display_word(&seq.word)
        } else {
            format!("[{}]", display.tags.join(", "))
        };
        display.shown
    }
}

fn part_count(part: &Part) -> Option<Count> {
    match &part.kind {
        PartKind::Element(_) => Some(Count::Exact(1)),
        PartKind::Received(word, _) => Some(Count::of(word)),
        PartKind::Observation | PartKind::Unordered => None,
    }
}

impl DisplayParts {
    fn tag(&mut self, tag: &Tag) {
        if self.tags.len() < VISIBLE_ERRORS {
            if self.shown.prefix.len() == self.tags.len() {
                self.shown.prefix.push(tag.clone());
            }
            self.tags.push(display_tag(tag));
        } else {
            self.ellipsis();
        }
    }

    fn ellipsis(&mut self) {
        if self.tags.len() <= VISIBLE_ERRORS && self.tags.last().is_none_or(|tag| tag != "...") {
            self.tags.push("...".into());
        }
    }

    fn parts(
        &mut self,
        sequence: &Sequence,
        expanded: &[Option<Sequence>],
        outer_cleanup: Option<SourceRange>,
    ) {
        for (i, part) in sequence.parts.iter().enumerate() {
            let cleanup = part.cleanup.or(outer_cleanup);
            match &part.kind {
                PartKind::Element(tag) => {
                    self.tag(tag);
                    if let Some(index) = self.index.filter(|index| *index < VISIBLE_ERRORS) {
                        let origin = part.origin.unwrap();
                        let position = match self.shown.count {
                            Some(Count::Exact(n)) => format!("Error {} of {n}", index + 1),
                            _ => format!("Error {}", index + 1),
                        };
                        let source = if cleanup.is_some() {
                            "added by `finally`"
                        } else if origin.kind == OriginKind::Throw {
                            "from the try body"
                        } else {
                            "from this operation"
                        };
                        self.shown.details.push((
                            origin.range,
                            format!("{position} is {} {source}", display_tag(tag)),
                        ));
                        self.index = Some(index + 1);
                    } else if let Some(cleanup) = cleanup {
                        self.cleanup(cleanup);
                    }
                }
                PartKind::Received(word, reception) => {
                    let inner = expanded.get(i).and_then(Option::as_ref);
                    let list = inner.map_or_else(
                        || display_word(word),
                        |inner| Shown::new(inner, &mut |_, _| None).list,
                    );
                    self.receipt(part.origin.unwrap(), reception.as_deref(), &list);
                    if let Some(inner) = inner {
                        self.parts(inner, &[], cleanup);
                    } else {
                        for tag in &word.prefix {
                            self.tag(tag);
                        }
                        if word.longer {
                            self.ellipsis();
                            self.index = None;
                        } else {
                            self.index = self
                                .index
                                .map(|index| index.saturating_add(word.prefix.len()));
                        }
                        if let Some(cleanup) = cleanup {
                            self.cleanup(cleanup);
                        }
                    }
                }
                PartKind::Observation | PartKind::Unordered => {
                    self.ellipsis();
                    self.index = None;
                    if let Some(origin) = part.origin {
                        self.shown.receipts.push((
                            origin.range,
                            "an Error can be observed at this await".into(),
                        ));
                    }
                    if let Some(cleanup) = cleanup {
                        self.cleanup(cleanup);
                    }
                }
            }
        }
        if sequence.omitted {
            self.ellipsis();
            self.index = match (self.index, sequence.count) {
                (_, Some(Count::Exact(n))) if n >= VISIBLE_ERRORS => Some(VISIBLE_ERRORS),
                _ => None,
            };
        }
    }

    fn receipt(&mut self, origin: Origin, reception: Option<&Reception>, list: &str) {
        let message = if let Some(site) = reception {
            let verb = if origin.kind == OriginKind::Await {
                "observe"
            } else {
                "propagate"
            };
            if site.contract.is_some() {
                format!("the Error bound of `{}` permits {list} here", site.label)
            } else if origin.kind == OriginKind::Await {
                format!("this await can {verb} {list} from `{}` here", site.label)
            } else {
                format!("`{}` can {verb} {list} here", site.label)
            }
        } else if origin.kind == OriginKind::Await {
            format!("this await can observe {list} here")
        } else {
            format!("this operation can yield {list} here")
        };
        self.shown.receipts.push((origin.range, message));
        if let Some(contract) = reception.and_then(|site| site.contract) {
            self.shown.receipts.push((
                contract,
                "the Error type upper bound is declared here".into(),
            ));
        }
    }

    fn cleanup(&mut self, at: SourceRange) {
        if !self.shown.details.iter().any(|(range, _)| *range == at) {
            self.shown.details.push((
                at,
                "this `finally` can add Errors after the incoming list".into(),
            ));
        }
    }
}

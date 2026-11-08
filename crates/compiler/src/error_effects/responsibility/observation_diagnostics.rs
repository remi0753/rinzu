//! Source evidence collected on a diagnostic-only replay of responsibility.
use super::*;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Replay {
    direct_calls: BTreeMap<Site, String>,
    witnesses: HashMap<Requirement, Witness>,
    escaping: Option<BTreeMap<(language::Word, language::Guards), Site>>,
    actual_escape: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Trace(Option<Box<PathEvidence>>);

// Evidence must never affect semantic equality or loop convergence.
impl PartialEq for Trace {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for Trace {}

#[derive(Clone, Debug, Default)]
struct PathEvidence {
    exit: Option<Site>,
    error_origin: Option<Site>,
    error_groups: Option<Vec<Option<Site>>>,
    replacements: BTreeMap<Token, Replacement>,
    // A call, an opaque store or a saved return may keep an alias outside
    // the local environment.
    handed_off: BTreeSet<Token>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Candidate {
    path: String,
    future: Token,
}

#[derive(Clone, Debug)]
struct Replacement {
    at: Site,
    local: String,
    candidates: Vec<Candidate>,
    awaited: Option<(Site, Candidate)>,
}

struct Witness {
    exit: Option<Site>,
    origin_call: Option<String>,
    replacement: Option<Replacement>,
}

impl Trace {
    pub fn enabled(boundary: bool) -> Self {
        Self(Some(Box::new(PathEvidence {
            error_groups: boundary.then(Vec::new),
            ..PathEvidence::default()
        })))
    }

    pub fn is_enabled(&self) -> bool {
        self.0.is_some()
    }

    pub fn exit(&self) -> Option<Site> {
        self.0.as_ref()?.exit
    }

    pub fn set_exit(&mut self, exit: Option<Site>) {
        if let Some(path) = &mut self.0 {
            path.exit = exit;
            path.error_origin = None;
        }
    }

    pub fn sites(&self) -> (Option<Site>, Option<Site>) {
        self.0
            .as_ref()
            .map_or((None, None), |path| (path.exit, path.error_origin))
    }

    pub fn restore_sites(&mut self, (exit, error_origin): (Option<Site>, Option<Site>)) {
        if let Some(path) = &mut self.0 {
            path.exit = exit;
            path.error_origin = error_origin;
        }
    }

    pub fn error_origin(&mut self, at: Site) {
        if let Some(path) = &mut self.0 {
            path.error_origin = Some(at);
        }
    }

    pub fn enter_group(&mut self) {
        if let Some(groups) = self.0.as_mut().and_then(|path| path.error_groups.as_mut()) {
            groups.push(None);
        }
    }

    pub fn collect_error(&mut self, previous: &Language, word: &language::Word) {
        if let Some(path) = &mut self.0
            && let Some(origin) = path
                .error_groups
                .as_mut()
                .and_then(|groups| groups.last_mut())
        {
            *origin = path.error_origin.filter(|_| {
                !word.longer
                    && word.prefix.len() == 1
                    && previous
                        .iter()
                        .all(|word| word.prefix.is_empty() && !word.longer)
            });
        }
    }

    pub fn leave_group(&mut self) -> Option<Site> {
        self.0.as_mut()?.error_groups.as_mut()?.pop().flatten()
    }

    pub fn prefer_exact(&mut self, other: &Self) {
        if let (Some(left), Some(right)) = (&mut self.0, &other.0)
            && right.rank() < left.rank()
        {
            left.clone_from(right);
        }
    }

    pub fn intersect(&mut self, other: &Self) {
        let (Some(left), Some(right)) = (&mut self.0, &other.0) else {
            return;
        };
        if left.exit != right.exit {
            left.exit = None;
        }
        if left.error_origin != right.error_origin {
            left.error_origin = None;
        }
        if let (Some(left), Some(right)) = (&mut left.error_groups, &right.error_groups) {
            for (saved, next) in left.iter_mut().zip(right) {
                if saved != next {
                    *saved = None;
                }
            }
        }
        left.handed_off.extend(right.handed_off.iter().cloned());
        left.replacements.retain(|token, replacement| {
            let Some(common) = right
                .replacements
                .get(token)
                .filter(|common| common.at == replacement.at && common.local == replacement.local)
            else {
                return false;
            };
            replacement
                .candidates
                .retain(|candidate| common.candidates.contains(candidate));
            if replacement.awaited != common.awaited {
                replacement.awaited = None;
            }
            !replacement.candidates.is_empty()
        });
    }

    pub fn forget_obligation(&mut self, token: &Token) {
        if let Some(path) = &mut self.0 {
            path.replacements.remove(token);
        }
    }

    pub fn forget_identity(&mut self, token: &Token) {
        if let Some(path) = &mut self.0 {
            path.handed_off.remove(token);
            path.replacements.remove(token);
            path.replacements.retain(|_, replacement| {
                replacement
                    .candidates
                    .retain(|candidate| &candidate.future != token);
                if replacement
                    .awaited
                    .as_ref()
                    .is_some_and(|(_, candidate)| &candidate.future == token)
                {
                    replacement.awaited = None;
                }
                !replacement.candidates.is_empty()
            });
        }
    }
}

impl State {
    pub fn record_handoff<'a>(&mut self, values: impl IntoIterator<Item = &'a Tracked>) {
        if self.trace.is_enabled() {
            let tokens: BTreeSet<_> = values
                .into_iter()
                .flat_map(|value| self.freeze(value).tokens())
                .map(|token| receivers::canonical(&token))
                .filter(|token| matches!(token, Token::Local(_, _, false)))
                .collect();
            let path = self.trace.0.as_mut().unwrap();
            path.replacements.retain(|token, _| !tokens.contains(token));
            path.handed_off.extend(tokens);
        }
    }

    pub fn record_replacement(&mut self, name: &str, value: &Tracked, at: Site) {
        let (Some(path), Some(previous)) = (&self.trace.0, self.env.get(name)) else {
            return;
        };
        let mut candidates = BTreeMap::new();
        replaced_futures(previous, value, name, &mut candidates);
        candidates.retain(|token, _| {
            self.pending.contains_key(token) && !path.handed_off.contains(token)
        });
        if candidates.is_empty() {
            return;
        }

        // Test the roots as they will be after the write. Heap fields,
        // external stores and deferred observers conservatively count as
        // possible aliases, as do tokens in opaque or widened values.
        let retained: BTreeSet<_> = self
            .env
            .iter()
            .filter(|(local, _)| *local != name)
            .map(|(_, value)| value)
            .chain(std::iter::once(value))
            .flat_map(Tracked::tokens)
            .chain(self.object_tokens())
            .chain(self.watch_tokens())
            .chain(self.receiver_tokens())
            .map(|token| receivers::canonical(&token))
            .collect();
        let path = self.trace.0.as_mut().unwrap();
        for (token, candidates) in candidates {
            if !retained.contains(&token) {
                path.replacements
                    .entry(token)
                    .or_insert_with(|| Replacement {
                        at,
                        local: name.to_owned(),
                        candidates,
                        awaited: None,
                    });
            }
        }
    }

    pub fn record_await(&mut self, awaited: &Token, at: Site) {
        if let Some(path) = &mut self.trace.0 {
            for (token, replacement) in &mut path.replacements {
                if replacement.awaited.is_none()
                    && self.pending.contains_key(token)
                    && let Some(candidate) =
                        replacement.candidates.iter().find(|c| &c.future == awaited)
                {
                    replacement.awaited = Some((at, candidate.clone()));
                }
            }
        }
    }
}

fn replaced_futures(
    before: &Tracked,
    after: &Tracked,
    path: &str,
    candidates: &mut BTreeMap<Token, Vec<Candidate>>,
) {
    match (before, after) {
        (Tracked::Future { token: before, .. }, Tracked::Future { token: after, .. })
            if before != after
                && local_source(before).is_some()
                && local_source(after).is_some() =>
        {
            candidates
                .entry(before.clone())
                .or_default()
                .push(Candidate {
                    path: path.to_owned(),
                    future: after.clone(),
                });
        }
        (Tracked::Record { fields: before, .. }, Tracked::Record { fields: after, .. }) => {
            for (name, before) in before {
                if let Some(after) = after.get(name) {
                    replaced_futures(before, after, &format!("{path}.{name}"), candidates);
                }
            }
        }
        _ => {}
    }
}

fn local_source(token: &Token) -> Option<Site> {
    match token {
        Token::Local(at, _, false) if at.4 == 0 => Some(*at),
        _ => None,
    }
}

type Rank = (u8, Option<Site>, Option<Site>, bool, Option<Site>);

fn rank(replacement: Option<&Replacement>, exit: Option<Site>) -> Rank {
    let at = replacement.map(|replacement| replacement.at);
    let awaited =
        replacement.and_then(|replacement| replacement.awaited.as_ref().map(|(at, _)| *at));
    let quality = match (at, awaited, exit) {
        (_, Some(_), _) => 0,
        (Some(_), None, _) => 1,
        (None, None, Some(_)) => 2,
        _ => 3,
    };
    (quality, at, awaited, exit.is_none(), exit)
}

impl PathEvidence {
    fn rank(&self) -> Rank {
        self.replacements
            .values()
            .map(|replacement| rank(Some(replacement), self.exit))
            .min()
            .unwrap_or_else(|| rank(None, self.exit))
    }
}

impl Replay {
    pub fn new(boundary: bool) -> Self {
        Self {
            escaping: boundary.then(BTreeMap::new),
            ..Self::default()
        }
    }

    pub fn tracks_boundary(&self) -> bool {
        self.escaping.is_some()
    }

    pub fn record_escape(&mut self, exit: &Exit, origin: Option<Site>) {
        if let Some(escaping) = &mut self.escaping
            && let Err(Stop::Error(word, _)) = &exit.result
        {
            self.actual_escape = true;
            if !word.longer
                && word.prefix.len() == 1
                && let Some(origin) = origin.filter(|origin| origin.4 == 0)
            {
                escaping
                    .entry(((**word).clone(), exit.guards.clone()))
                    .and_modify(|at| *at = (*at).min(origin))
                    .or_insert(origin);
            }
        }
    }

    pub fn main_diagnostic(&self, function: &FunctionDecl, contract: &Contract) -> Diagnostic {
        let header = SourceRange {
            end_byte: function.body.range.start_byte,
            ..function.range
        };
        let errors = || {
            contract.exits.iter().filter_map(|exit| {
                if let Err(Stop::Error(word, _)) = &exit.result {
                    Some((word, &exit.guards))
                } else {
                    None
                }
            })
        };
        let origin = errors()
            .filter_map(|(word, guards)| {
                self.escaping
                    .as_ref()?
                    .get(&((**word).clone(), guards.clone()))
                    .map(|&at| (at, &word.prefix[0]))
            })
            .min_by_key(|(at, tag)| (*at, *tag));
        let declared = function
            .error_contract
            .as_ref()
            .filter(|_| !self.actual_escape);
        let primary = origin
            .map(|(at, _)| range(at))
            .or_else(|| declared.map(|c| c.range))
            .unwrap_or(header);
        let tag = origin
            .map(|(_, tag)| tag)
            .or_else(|| declared.and_then(|_| errors().find_map(|(word, _)| word.prefix.first())));
        let subject = tag
            .filter(|tag| matches!(tag, Tag::Named(_) | Tag::Set(_)))
            .map_or_else(
                || "an ordinary Error".into(),
                super::super::catch_diagnostics::display_tag,
            );
        let mut diagnostic = Diagnostic::error(
            DiagnosticCode::ErrorEscapesBoundary,
            primary,
            format!("{subject} can escape the Main entry"),
            "handle the Error before it leaves `main`",
        );
        if primary != header {
            diagnostic = diagnostic.with_secondary(
                header,
                "`main` must handle every ordinary Error before it exits",
            );
        }
        diagnostic
    }

    pub fn direct_call(&mut self, at: Site, target: &[String]) {
        // Package linking and specialization can rewrite the target. Display
        // a name only when the target still has a source spelling.
        if target
            .iter()
            .all(|part| part.chars().all(|c| c.is_alphanumeric() || c == '_'))
        {
            self.direct_calls
                .entry(at)
                .or_insert_with(|| format!("{}()", target.join(".")));
        }
    }

    fn direct_name(&self, token: &Token) -> Option<&String> {
        match token {
            Token::Local(at, 0, false) => self.direct_calls.get(at),
            _ => None,
        }
    }

    pub fn record(
        &mut self,
        requirement: Requirement,
        state: &State,
        token: &Token,
        exit: Option<Site>,
    ) {
        let replacement = state
            .trace
            .0
            .as_ref()
            .and_then(|path| path.replacements.get(token));
        if self.witnesses.get(&requirement).is_some_and(|current| {
            rank(current.replacement.as_ref(), current.exit) <= rank(replacement, exit)
        }) {
            return;
        }
        let witness = Witness {
            exit,
            origin_call: self
                .direct_name(token)
                .filter(|_| local_source(token) == Some(requirement.origin))
                .cloned(),
            replacement: replacement.cloned(),
        };
        self.witnesses.insert(requirement, witness);
    }

    pub fn diagnostic(&self, requirement: &Requirement) -> Diagnostic {
        let witness = self.witnesses.get(requirement);
        let Some((witness, replacement)) =
            witness.and_then(|witness| Some((witness, witness.replacement.as_ref()?)))
        else {
            let exit = witness
                .and_then(|witness| witness.exit)
                .unwrap_or(requirement.exit);
            return Diagnostic::error(
                DiagnosticCode::FutureDiscarded,
                range(requirement.origin),
                "this task can fail after its observation path is lost",
                "await an alias on every managed exit, return the Future, or pass it to a proven observer",
            )
            .with_secondary(range(exit), "this exit does not preserve an observation path");
        };
        let candidate = replacement
            .awaited
            .as_ref()
            .map_or(&replacement.candidates[0], |(_, candidate)| candidate);
        let mut diagnostic = Diagnostic::error(
            DiagnosticCode::FutureDiscarded,
            range(replacement.at),
            format!(
                "replacing `{}` leaves the Future previously held in `{}` unobserved",
                replacement.local, candidate.path,
            ),
            "keep and await the original Future, or create it only on paths that can observe it",
        )
        .with_secondary(
            range(requirement.origin),
            witness.origin_call.as_ref().map_or_else(
                || "this expression yields the Future that can fail".to_owned(),
                |call| format!("`{call}` creates the Future that can fail"),
            ),
        );
        let source = local_source(&candidate.future).expect("recorded replacement source");
        let call = self.direct_name(&candidate.future);
        if source.0 != replacement.at.0
            || source.1 < replacement.at.1
            || source.2 > replacement.at.2
        {
            diagnostic = diagnostic.with_secondary(
                range(source),
                call.map_or_else(
                    || "the replacement Future is obtained here".to_owned(),
                    |call| format!("`{call}` creates the replacement Future"),
                ),
            );
        }
        if let Some((awaited, _)) = &replacement.awaited {
            diagnostic = diagnostic.with_secondary(
                range(*awaited),
                call.map_or_else(
                    || "this `await` observes the replacement Future".to_owned(),
                    |call| format!("this `await` observes the replacement Future from `{call}`"),
                ),
            );
        } else if let Some(exit) = witness.exit {
            diagnostic = diagnostic.with_secondary(
                range(exit),
                "this exit does not preserve an observation path",
            );
        }
        diagnostic
    }
}

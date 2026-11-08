//! Declared effects are public upper bounds, never assertions about actual failure.
use super::*;

pub(super) fn allowed(checked: &CheckedModule, contract: &ErrorContract) -> BTreeSet<Tag> {
    contract
        .types
        .iter()
        .filter_map(|source| {
            checked
                .type_references
                .get(&source.range)
                .and_then(|ty| tag(checked, ty))
                .or_else(|| match &source.kind {
                    TypeRefKind::Named(path) if path.join(".") == "Error" => Some(Tag::Any),
                    TypeRefKind::Named(path) => Some(Tag::Named(path.join("."))),
                    _ => None,
                })
        })
        .collect()
}

pub(super) fn declared(
    checked: &CheckedModule,
    function: &FunctionDecl,
    bound: usize,
) -> Option<Language> {
    let contract = function.error_contract.as_ref()?;
    let allowed = allowed(checked, contract);
    Some(language_for_tags(allowed, bound))
}

pub(super) fn for_bound(types: &[String], bound: usize) -> Language {
    language_for_tags(
        types
            .iter()
            .map(|name| {
                if name == "Error" {
                    Tag::Any
                } else {
                    Tag::Named(name.clone())
                }
            })
            .collect(),
        bound,
    )
}

fn language_for_tags(allowed: BTreeSet<Tag>, bound: usize) -> Language {
    if allowed.is_empty() {
        return Language::new();
    }
    let tag = if allowed.contains(&Tag::Any) {
        Tag::Any
    } else if allowed.len() == 1 {
        allowed.first().unwrap().clone()
    } else {
        Tag::Set(
            allowed
                .into_iter()
                .filter_map(|t| if let Tag::Named(n) = t { Some(n) } else { None })
                .collect(),
        )
    };
    language::unknown(bound)
        .into_iter()
        .map(|mut word| {
            word.prefix.fill(tag.clone());
            if word.longer {
                word.tail = BTreeSet::from([tag.clone()]);
            }
            word
        })
        .collect()
}

pub(super) fn violation(
    checked: &CheckedModule,
    function: &FunctionDecl,
    actual: &Summary,
) -> Option<BTreeSet<Tag>> {
    let contract = function.error_contract.as_ref()?;
    let allowed = allowed(checked, contract);
    (!allowed.contains(&Tag::Any)
        && actual
            .errors
            .iter()
            .flat_map(|word| word.prefix.iter().chain(&word.tail))
            .any(|tag| language::outside(tag, &allowed)))
    .then_some(allowed)
}

pub(super) fn diagnostic(
    function: &FunctionDecl,
    actual: &Summary,
    allowed: BTreeSet<Tag>,
    traced: &Language,
) -> Diagnostic {
    use language::{OriginKind, outside};
    let contract = function.error_contract.as_ref().unwrap();
    let first = actual
        .errors
        .iter()
        .flat_map(|word| word.prefix.iter().chain(&word.tail))
        .find(|tag| outside(tag, &allowed))
        .unwrap();
    debug_assert_eq!(
        traced, &actual.errors,
        "tracing changed the escaping Error language"
    );
    let traced_violation = traced.iter().find_map(|word| {
        let origins = traced.origins(word)?;
        word.prefix
            .iter()
            .zip(origins.prefix.iter().copied())
            .chain(
                word.tail
                    .iter()
                    .map(|tag| (tag, origins.tail.get(tag).copied())),
            )
            .find_map(|(tag, origin)| {
                outside(tag, &allowed)
                    .then_some(origin)
                    .flatten()
                    .map(|origin| (tag, origin))
            })
    });
    let (invalid, origin) =
        traced_violation.map_or((first, None), |(tag, origin)| (tag, Some(origin)));
    let interface = function.name.split_once(".$interface_");
    let name = interface.map_or_else(
        || display_identity(&function.name),
        |(owner, generated)| {
            let member = generated
                .split_once('_')
                .map_or(generated, |(_, member)| member);
            display_identity(&format!("{owner}.{member}"))
        },
    );
    let declared = display_bound(
        &contract
            .types
            .iter()
            .map(crate::interfaces::spelling)
            .collect::<Vec<_>>(),
    );
    let types = match invalid {
        Tag::Named(name) => Some(display_identity(name)),
        Tag::Set(names) => Some(
            names
                .iter()
                .filter(|name| !allowed.contains(&Tag::Named((*name).clone())))
                .map(|name| display_identity(name))
                .collect::<Vec<_>>()
                .join(" or "),
        ),
        _ => None,
    };
    let subject = types
        .as_deref()
        .unwrap_or("an ordinary Error of unknown type");
    let contract_owner = if interface.is_some() {
        format!("whose interface requires `{declared}`")
    } else {
        format!("which declares `{declared}`")
    };
    let mut message = format!("{subject} can escape `{name}`, {contract_owner}");
    if origin.is_some_and(|origin| origin.kind == OriginKind::UnknownCallback) {
        message.push_str("; the ordinary Error bound of this callback cannot be established");
    }
    let recover = match origin {
        Some(origin) if origin.in_finally => "recover the Error in `finally`",
        Some(origin) => match origin.kind {
            OriginKind::Call | OriginKind::UnknownCallback => "recover the Error from this call",
            OriginKind::Await => "recover the Error from this await",
            OriginKind::Throw => "handle the Error before it leaves this function",
            OriginKind::Operation => "recover the Error from this operation",
        },
        None => "recover the Error inside the function",
    };
    let escaping = public_types(actual);
    let help = if escaping == ["Error"] {
        format!("{recover}; the full ordinary Error bound could not be established")
    } else {
        let widened = allowed
            .into_iter()
            .filter_map(|tag| {
                if let Tag::Named(name) = tag {
                    Some(name)
                } else {
                    None
                }
            })
            .chain(escaping)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let widened = display_bound(&widened);
        if interface.is_some() {
            format!("{recover} to satisfy the interface's `{declared}` contract")
        } else {
            format!("{recover}, or use `{widened}` if callers should handle it")
        }
    };
    let primary = origin.map_or(contract.range, |origin| origin.range);
    let mut diagnostic =
        Diagnostic::error(DiagnosticCode::InvalidErrorContract, primary, message, help);
    if primary != contract.range {
        let note = if interface.is_some() {
            format!("the interface requires `{declared}` here")
        } else {
            format!("`{name}` declares `{declared}` here")
        };
        diagnostic = diagnostic.with_secondary(contract.range, note);
    }
    diagnostic
}

pub(super) fn display_identity(name: &str) -> String {
    name.replace("::", ".")
}

pub(crate) fn display_bound(types: &[String]) -> String {
    if types.is_empty() {
        return "nothrow".into();
    }
    let mut names = types
        .iter()
        .map(|name| display_identity(name))
        .collect::<Vec<_>>();
    names.sort();
    format!("throws({})", names.join(", "))
}

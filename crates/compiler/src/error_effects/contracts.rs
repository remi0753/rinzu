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

pub(super) fn validate(
    checked: &CheckedModule,
    function: &FunctionDecl,
    actual: &Summary,
) -> Vec<Diagnostic> {
    let Some(contract) = &function.error_contract else {
        return Vec::new();
    };
    let allowed = allowed(checked, contract);
    if allowed.contains(&Tag::Any) {
        return Vec::new();
    }
    let Some(invalid) = actual.errors.iter().find(|word| {
        word.prefix.iter().chain(&word.tail).any(|tag| match tag {
            Tag::Named(_) => !allowed.contains(tag),
            Tag::Set(names) => names
                .iter()
                .any(|n| !allowed.contains(&Tag::Named(n.clone()))),
            _ => true,
        })
    }) else {
        return Vec::new();
    };
    let (name, primary, secondary, explanation) =
        if let Some((owner, generated)) = function.name.split_once(".$interface_") {
            let member = generated
                .split_once('_')
                .map_or(generated, |(_, member)| member);
            (
                format!("{owner}.{member}"),
                function.range,
                contract.range,
                "the interface requires this Error upper bound",
            )
        } else {
            (
                function.name.clone(),
                contract.range,
                function.body.range,
                "the body, catch handlers and finally must all satisfy this contract",
            )
        };
    let types: BTreeSet<_> = invalid
        .prefix
        .iter()
        .chain(&invalid.tail)
        .flat_map(|tag| match tag {
            Tag::Named(name) => vec![name.clone()],
            Tag::Set(names) => names.iter().cloned().collect(),
            _ => vec!["Error".into()],
        })
        .collect();
    let mut diagnostic = Diagnostic::error(
        DiagnosticCode::InvalidErrorContract,
        primary,
        format!("function `{name}` can propagate {} in {} outside its declared Error contract",
            types.iter().cloned().collect::<Vec<_>>().join(", "), invalid.display()),
        "recover the Error inside the function, or widen its throws contract; nothrow requires no escaping ordinary Errors",
    ).with_secondary(secondary, explanation);
    let body = function.body.range;
    if let Some((range, _)) = checked
        .indirect_signatures
        .iter()
        .filter(|(range, signature)| {
            range.file_id == body.file_id
                && range.instance == body.instance
                && body.start_byte <= range.start_byte
                && range.end_byte <= body.end_byte
                && signature.error_bound.as_ref().is_some_and(|bound| {
                    bound.iter().any(|name| {
                        !allowed.contains(&Tag::Named(name.clone()))
                            && (name == "Error" || types.contains("Error") || types.contains(name))
                    })
                })
        })
        .min_by_key(|(range, _)| (range.start_byte, range.end_byte))
    {
        diagnostic = diagnostic.with_secondary(
            *range,
            "this callback type permits an Error outside the declared bound",
        );
    }
    vec![diagnostic]
}

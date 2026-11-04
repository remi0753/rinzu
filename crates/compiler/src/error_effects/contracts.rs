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
    if allowed.is_empty() {
        return Some(Language::new());
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
    Some(
        language::unknown(bound)
            .into_iter()
            .map(|mut word| {
                word.prefix.fill(tag.clone());
                if word.longer {
                    word.tail = BTreeSet::from([tag.clone()]);
                }
                word
            })
            .collect(),
    )
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
    let invalid = actual.errors.iter().find(|word| {
        word.prefix.iter().chain(&word.tail).any(|tag| match tag {
            Tag::Named(_) => !allowed.contains(tag),
            Tag::Set(names) => names
                .iter()
                .any(|n| !allowed.contains(&Tag::Named(n.clone()))),
            _ => true,
        })
    });
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
    invalid.map(|word| {
        let types=word.prefix.iter().chain(&word.tail).flat_map(|t|match t {
            Tag::Named(n)=>vec![n.clone()],
            Tag::Set(names)=>names.iter().cloned().collect(),
            _=>vec!["Error".into()],
        }).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", ");
        Diagnostic::error(
        DiagnosticCode::InvalidErrorContract,
        primary,
        format!("function `{name}` can propagate {types} in {} outside its declared Error contract", word.display()),
        "recover the Error inside the function, or widen its throws contract; nothrow requires no escaping ordinary Errors",
    ).with_secondary(secondary, explanation)}).into_iter().collect()
}

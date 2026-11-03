//! Private versioned consumption proof in source-package build inputs.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const KEY: &str = "rinzu.callback-consumption.v1";
pub const RULE: &str = "rinzu.closed-consumer.v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub rule: String,
    pub analyzer: Option<String>,
    pub functions: BTreeMap<String, Proof>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    pub symbol: String,
    pub parameters: Vec<usize>,
    pub region: String,
    pub ir_digest: Option<String>,
    pub object_digest: Option<String>,
    pub closed: bool,
    pub reasons: Vec<String>,
}

fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn decode(inputs: &BTreeMap<String, String>) -> Result<Option<Report>, String> {
    let Some(text) = inputs.get(KEY) else {
        return Ok(None);
    };
    let report: Report = serde_json::from_str(text)
        .map_err(|error| format!("invalid consumption proof: {error}"))?;
    if report.rule != RULE || report.analyzer.as_deref().is_some_and(|hash| !digest(hash)) {
        return Err("unsupported consumption rule or analyzer digest".into());
    }
    for (identity, proof) in &report.functions {
        let sorted = proof.parameters.windows(2).all(|pair| pair[0] < pair[1]);
        if identity.is_empty()
            || proof.symbol.is_empty()
            || proof.parameters.is_empty()
            || !sorted
            || proof.region != "native-call"
            || proof.ir_digest.as_deref().is_some_and(|hash| !digest(hash))
            || proof
                .object_digest
                .as_deref()
                .is_some_and(|hash| !digest(hash))
            || (proof.closed
                && (report.analyzer.is_none()
                    || proof.ir_digest.is_none()
                    || proof.object_digest.is_none()
                    || !proof.reasons.is_empty()))
            || (!proof.closed && proof.reasons.is_empty())
        {
            return Err(format!("invalid consumption proof for {identity}"));
        }
    }
    Ok(Some(report))
}

/// Validate report identities even before a particular application's IR exists.
pub fn validate(interface: &crate::source_package::SourceInterface) -> Result<(), String> {
    if let Some(report) = decode(&interface.package.build_inputs)? {
        for (identity, proof) in report.functions {
            let function = interface
                .functions
                .iter()
                .find(|function| function.identity == identity)
                .ok_or_else(|| format!("consumption proof has unknown identity {identity}"))?;
            if function.kind != crate::source_package::FunctionKind::Native
                || proof
                    .parameters
                    .iter()
                    .any(|index| *index >= function.parameter_requirements.len())
            {
                return Err(format!(
                    "consumption proof has invalid native parameters: {identity}"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrupted_or_partial_proofs_never_authorize_consumption() {
        let proof = Proof {
            symbol: "consumer".into(),
            parameters: vec![0],
            region: "native-call".into(),
            ir_digest: Some("a".repeat(64)),
            object_digest: Some("b".repeat(64)),
            closed: true,
            reasons: vec![],
        };
        let report = Report {
            rule: RULE.into(),
            analyzer: Some("c".repeat(64)),
            functions: BTreeMap::from([("sdk::consumer".into(), proof)]),
        };
        let value = serde_json::to_value(report).unwrap();
        let decode_value =
            |value: &serde_json::Value| decode(&BTreeMap::from([(KEY.into(), value.to_string())]));
        assert!(decode_value(&value).is_ok());
        for (field, replacement) in [
            ("rule", serde_json::json!("old-rule")),
            ("analyzer", serde_json::Value::Null),
            ("unknown-field", serde_json::json!(1)),
        ] {
            let mut changed = value.clone();
            changed[field] = replacement;
            assert!(decode_value(&changed).is_err());
        }
        for (field, replacement) in [
            ("parameters", serde_json::json!([0, 0])),
            ("ir_digest", serde_json::json!("broken")),
            ("object_digest", serde_json::Value::Null),
            ("region", serde_json::json!("callback-return")),
            ("reasons", serde_json::json!(["unknown"])),
        ] {
            let mut changed = value.clone();
            changed["functions"]["sdk::consumer"][field] = replacement;
            assert!(decode_value(&changed).is_err());
        }
        assert!(decode(&BTreeMap::new()).unwrap().is_none());
    }
}

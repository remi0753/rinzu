//! Private checked-input identity. Generated output digests belong in a separate receipt.
use std::collections::BTreeMap;

pub const KEY: &str = "compilation.input-v1";
pub const DIGEST_KEY: &str = "compilation.input-sha256";
pub const SCHEMA: &str = "rinzu.compilation-fingerprint-v1";

pub fn encode(
    inputs: &mut BTreeMap<String, String>,
    value: serde_json::Value,
) -> Result<String, String> {
    let wire = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    let digest = crate::source_package::sha256(wire.as_bytes());
    inputs.insert(KEY.into(), wire);
    inputs.insert(DIGEST_KEY.into(), digest.clone());
    validate(inputs)?;
    Ok(digest)
}

pub fn validate(inputs: &BTreeMap<String, String>) -> Result<(), String> {
    let Some(wire) = inputs.get(KEY) else {
        return if inputs.contains_key(DIGEST_KEY) {
            Err("compilation fingerprint digest has no input record".into())
        } else {
            Ok(())
        };
    };
    let value: serde_json::Value = serde_json::from_str(wire).map_err(|error| error.to_string())?;
    if value["schema"] != SCHEMA
        || !value["graph"].is_array()
        || !value["scope"].is_object()
        || !value["inputs"].is_object()
        || value["inputs"].get(KEY).is_some()
        || value["inputs"].get(DIGEST_KEY).is_some()
        || inputs.get(DIGEST_KEY) != Some(&crate::source_package::sha256(wire.as_bytes()))
        || serde_json::to_string(&value).map_err(|error| error.to_string())? != *wire
    {
        return Err("unsupported or invalid compilation fingerprint".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_receipt_rejects_orphan_digest_tampering_unknown_revision_and_self_input() {
        let value = serde_json::json!({ "schema": SCHEMA, "graph": [], "scope": {}, "inputs": {} });
        let mut inputs = BTreeMap::new();
        encode(&mut inputs, value.clone()).unwrap();
        inputs.insert(DIGEST_KEY.into(), "a".repeat(64));
        assert!(validate(&inputs).is_err());
        inputs.remove(KEY);
        assert!(validate(&inputs).is_err());
        let mut unknown = value.clone();
        unknown["schema"] = "rinzu.compilation-fingerprint-v0".into();
        assert!(encode(&mut BTreeMap::new(), unknown).is_err());
        let mut recursive = value;
        recursive["inputs"][KEY] = "self".into();
        assert!(encode(&mut BTreeMap::new(), recursive).is_err());
    }
}

//! Private exact-provider metadata; the public source table ABI is unchanged.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub const KEY: &str = "native.binding-plan-v1";
pub const DIGEST_KEY: &str = "native.binding-plan-sha256";
pub const INITIALIZER: &str = "__rinzu_bind_native_v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBindingPlan {
    pub version: u32,
    pub target: String,
    pub images: Vec<NativeProviderImage>,
    pub slots: Vec<NativeBindingSlot>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_package::SourceInterface;

    fn fixture() -> (NativeBindingPlan, SourceInterface) {
        let interface: SourceInterface = serde_json::from_str(include_str!(
            "../../../tests/fixtures/source-package/interface.json"
        ))
        .unwrap();
        let function = &interface.functions[0];
        let image = NativeProviderImage {
            path: "provider.dylib".into(),
            bundled: true,
            sha256: Some("a".repeat(64)),
            uuid: "b".repeat(32),
        };
        (
            NativeBindingPlan {
                version: 1,
                target: "aarch64-apple-darwin".into(),
                images: vec![image.clone()],
                slots: vec![NativeBindingSlot {
                    declaration: function.identity.clone(),
                    symbol: "add".into(),
                    resolver: Some("__rinzu_native_address_fixture".into()),
                    signature: function.source_type.clone(),
                    definition: "object:fixture:add".into(),
                    image: 0,
                    owner_path: image.path,
                    owner_uuid: image.uuid,
                    owner_sha256: image.sha256,
                }],
            },
            interface,
        )
    }

    #[test]
    fn plan_digest_paths_and_definition_transaction_are_checked() {
        let (plan, _) = fixture();
        let mut inputs = BTreeMap::new();
        plan.encode(&mut inputs).unwrap();
        assert_eq!(
            NativeBindingPlan::decode(&inputs).unwrap(),
            Some(plan.clone())
        );
        inputs.get_mut(KEY).unwrap().push(' ');
        assert!(
            NativeBindingPlan::decode(&inputs)
                .unwrap_err()
                .contains("digest")
        );
        for invalid in [
            NativeBindingPlan {
                version: 3,
                ..plan.clone()
            },
            NativeBindingPlan {
                target: "x86_64-apple-darwin".into(),
                ..plan.clone()
            },
            NativeBindingPlan {
                slots: vec![plan.slots[0].clone(), plan.slots[0].clone()],
                ..plan.clone()
            },
        ] {
            assert!(invalid.validate().is_err());
        }
        let mut invalid = plan.clone();
        invalid.images[0].path = "../escape.dylib".into();
        assert!(invalid.validate().is_err());
        let mut invalid = plan.clone();
        invalid.slots[0].owner_uuid = "c".repeat(32);
        assert!(invalid.validate().is_err());
        assert!(
            NativeBindingPlan::decode(&BTreeMap::from([(DIGEST_KEY.into(), "a".repeat(64))]))
                .is_err()
        );
    }

    #[test]
    fn slots_must_cover_exact_checked_native_signatures() {
        let (plan, mut interface) = fixture();
        plan.validate_functions(&interface.functions).unwrap();
        interface.functions[0].source_type = "native(CDouble)->CDouble".into();
        assert!(plan.validate_functions(&interface.functions).is_err());
        assert!(plan.validate_functions(&[]).is_err());
    }

    #[test]
    fn deployed_direct_and_reexported_owners_are_checked() {
        let (mut plan, _) = fixture();
        plan.version = 2;
        plan.slots[0].resolver = None;
        plan.validate().unwrap();
        let mut owner = plan.images[0].clone();
        owner.path = "terminal.dylib".into();
        owner.uuid = "c".repeat(32);
        plan.slots[0].owner_path = owner.path.clone();
        plan.slots[0].owner_uuid = owner.uuid.clone();
        assert!(plan.validate().is_err());
        plan.images.push(owner);
        plan.validate().unwrap();
        let mut inputs = BTreeMap::new();
        plan.encode(&mut inputs).unwrap();
        assert_eq!(
            NativeBindingPlan::decode(&inputs).unwrap(),
            Some(plan.clone())
        );
        plan.slots[0].owner_sha256 = Some("d".repeat(64));
        assert!(plan.validate().is_err());
        plan.slots[0].owner_path = "../terminal.dylib".into();
        assert!(plan.validate().is_err());
        plan.slots[0].owner_path = "/tmp/external.dylib".into();
        assert!(plan.validate().is_err());
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeProviderImage {
    /// A confined leaf name for bundled providers, an absolute path otherwise.
    pub path: String,
    pub bundled: bool,
    pub sha256: Option<String>,
    /// Mach-O UUID, including for images in the system shared cache.
    pub uuid: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBindingSlot {
    pub declaration: String,
    pub symbol: String,
    /// Provider-owned address getter for an object/archive definition.
    pub resolver: Option<String>,
    pub signature: String,
    pub definition: String,
    pub image: usize,
    /// A bundled provider's leaf name, or the selected re-export terminal path.
    pub owner_path: String,
    pub owner_uuid: String,
    pub owner_sha256: Option<String>,
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn image_path(path: &str, bundled: bool) -> bool {
    !path.is_empty()
        && !path.contains(['\0', '\n', '\r'])
        && if bundled {
            !matches!(path, "." | "..") && !path.contains(['/', '\\'])
        } else {
            std::path::Path::new(path).is_absolute()
        }
}

fn os_image(path: &str) -> bool {
    let path = std::path::Path::new(path);
    path.starts_with("/usr/lib") || path.starts_with("/System/Library")
}

impl NativeBindingPlan {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.version, 1 | 2)
            || self.target != "aarch64-apple-darwin"
            || self.slots.is_empty()
        {
            return Err("unsupported or empty native binding plan".into());
        }
        let mut declarations = BTreeSet::new();
        for image in &self.images {
            if !image_path(&image.path, image.bundled)
                || !hex(&image.uuid, 32)
                || image.sha256.as_ref().is_some_and(|value| !hex(value, 64))
                || (image.bundled && image.sha256.is_none())
                || (self.version == 2 && !image.bundled && !os_image(&image.path))
            {
                return Err("invalid native provider path/digest/UUID".into());
            }
        }
        for slot in &self.slots {
            let image = self
                .images
                .get(slot.image)
                .ok_or("native slot image is absent")?;
            let mut symbol = slot.symbol.bytes();
            if !symbol
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                || !symbol.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || slot.resolver.as_ref().is_some_and(|value| {
                    !value.starts_with("__rinzu_native_address_")
                        || !value
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                })
                || (self.version == 1 && image.bundled != slot.resolver.is_some())
                || (slot.resolver.is_some() && !image.bundled)
                || slot.declaration.is_empty()
                || slot.signature.is_empty()
                || slot.definition.is_empty()
                || !declarations.insert(&slot.declaration)
                || !image_path(&slot.owner_path, self.owner_bundled(slot))
                || (self.version == 2 && !self.owner_bundled(slot) && !os_image(&slot.owner_path))
                || !hex(&slot.owner_uuid, 32)
                || slot
                    .owner_sha256
                    .as_ref()
                    .is_some_and(|value| !hex(value, 64))
                || (slot.resolver.is_some()
                    && (slot.owner_path != image.path
                        || slot.owner_uuid != image.uuid
                        || slot.owner_sha256 != image.sha256))
            {
                return Err("invalid native slot identity/symbol/provider".into());
            }
            if self.version == 2
                && self.owner_bundled(slot)
                && !self.images.iter().any(|owner| {
                    owner.bundled
                        && owner.path == slot.owner_path
                        && owner.uuid == slot.owner_uuid
                        && owner.sha256 == slot.owner_sha256
                })
            {
                return Err("deployed native slot owner is not in the image inventory".into());
            }
        }
        Ok(())
    }

    pub fn owner_bundled(&self, slot: &NativeBindingSlot) -> bool {
        if self.version == 1 {
            self.images
                .get(slot.image)
                .is_some_and(|image| image.bundled)
        } else {
            !std::path::Path::new(&slot.owner_path).is_absolute()
        }
    }

    pub fn validate_functions(
        &self,
        functions: &[crate::source_package::FunctionRecord],
    ) -> Result<(), String> {
        self.validate()?;
        if functions
            .iter()
            .filter(|function| function.symbol.is_some())
            .count()
            != self.slots.len()
        {
            return Err("native binding slots disagree with descriptor functions".into());
        }
        for slot in &self.slots {
            if !functions.iter().any(|function| {
                function.identity == slot.declaration
                    && function.symbol.is_some()
                    && function.source_type == slot.signature
            }) {
                return Err(format!(
                    "native slot signature disagrees with descriptor: {}",
                    slot.declaration
                ));
            }
        }
        Ok(())
    }

    pub fn encode(&self, inputs: &mut BTreeMap<String, String>) -> Result<String, String> {
        self.validate()?;
        let wire = serde_json::to_string(self).map_err(|error| error.to_string())?;
        let digest = crate::source_package::sha256(wire.as_bytes());
        inputs.insert(KEY.into(), wire);
        inputs.insert(DIGEST_KEY.into(), digest.clone());
        Ok(digest)
    }

    pub fn decode(inputs: &BTreeMap<String, String>) -> Result<Option<Self>, String> {
        let Some(wire) = inputs.get(KEY) else {
            if inputs.contains_key(DIGEST_KEY) {
                return Err("native plan digest has no plan".into());
            }
            return Ok(None);
        };
        if inputs.get(DIGEST_KEY) != Some(&crate::source_package::sha256(wire.as_bytes())) {
            return Err("native binding plan digest mismatch".into());
        }
        let plan: Self = serde_json::from_str(wire).map_err(|error| error.to_string())?;
        plan.validate()?;
        Ok(Some(plan))
    }
}

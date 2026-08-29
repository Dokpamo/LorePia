use lorepia_domain::{
    CharacterRuntimeProfile, CoreError, CoreErrorCode, CoreResult, PortableRuntimeCapability,
};
use serde_json::{Map, Value};

const CAPABILITY_CATALOG_SIZE: usize = 10;

pub(crate) const LEGACY_RUNTIME_CAPABILITY_CEILING: [PortableRuntimeCapability; 2] = [
    PortableRuntimeCapability::RuntimeCallbacks,
    PortableRuntimeCapability::UiWrite,
];

pub(crate) fn parse_runtime_capabilities(
    object: &Map<String, Value>,
) -> CoreResult<Option<Vec<PortableRuntimeCapability>>> {
    let camel = object.get("requiredCapabilities");
    let snake = object.get("required_capabilities");
    let (field, value) = match (camel, snake) {
        (Some(_), Some(_)) => {
            return Err(unsupported(
                "runtime capability declaration must use exactly one supported field name",
            ));
        }
        (Some(value), None) => ("requiredCapabilities", value),
        (None, Some(value)) => ("required_capabilities", value),
        (None, None) => return Ok(Some(legacy_runtime_capabilities())),
    };
    if value.is_null() {
        return Err(unsupported(format!(
            "runtime {field} must be an array and cannot be null"
        )));
    }
    let values = value.as_array().ok_or_else(|| {
        unsupported(format!(
            "runtime {field} must be an array of capability names"
        ))
    })?;
    if values.len() > CAPABILITY_CATALOG_SIZE {
        return Err(unsupported(format!(
            "runtime {field} exceeds the supported capability catalog"
        )));
    }
    let mut capabilities = values
        .iter()
        .map(|value| {
            let name = value.as_str().ok_or_else(|| {
                unsupported("runtime capability declarations must contain only strings")
            })?;
            capability_from_name(name).ok_or_else(|| {
                unsupported(format!(
                    "runtime capability declaration is not supported: {name}"
                ))
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    canonicalize_runtime_capabilities(&mut capabilities)?;
    Ok(Some(capabilities))
}

pub(crate) fn normalize_runtime_profile_capabilities(
    profile: &mut CharacterRuntimeProfile,
) -> CoreResult<()> {
    if profile.required_capabilities.is_none() && runtime_profile_is_contributor(profile) {
        profile.required_capabilities = Some(legacy_runtime_capabilities());
    }
    if let Some(capabilities) = profile.required_capabilities.as_mut() {
        canonicalize_runtime_capabilities(capabilities)?;
        if profile.scripts.iter().any(|script| script.elevated_access)
            && !capabilities.contains(&PortableRuntimeCapability::Elevated)
        {
            return Err(unsupported(
                "elevated runtime scripts require an explicit elevated capability declaration",
            ));
        }
    }
    Ok(())
}

pub(crate) fn intersect_runtime_profile_capabilities(
    target: &mut CharacterRuntimeProfile,
    incoming: &mut CharacterRuntimeProfile,
) -> CoreResult<()> {
    normalize_runtime_profile_capabilities(target)?;
    normalize_runtime_profile_capabilities(incoming)?;
    target.required_capabilities = match (
        target.required_capabilities.take(),
        incoming.required_capabilities.take(),
    ) {
        (Some(mut target), Some(incoming)) => {
            target.retain(|capability| incoming.binary_search(capability).is_ok());
            Some(target)
        }
        (Some(target), None) => Some(target),
        (None, Some(incoming)) => Some(incoming),
        (None, None) => None,
    };
    Ok(())
}

pub(crate) fn legacy_runtime_capabilities() -> Vec<PortableRuntimeCapability> {
    LEGACY_RUNTIME_CAPABILITY_CEILING.to_vec()
}

fn runtime_profile_is_contributor(profile: &CharacterRuntimeProfile) -> bool {
    profile.source_id.is_some()
        || profile.transform_set_id.is_some()
        || !profile.transforms.is_empty()
        || !profile.scripts.is_empty()
        || profile.required_capabilities.is_some()
        || !profile.background_markup.is_empty()
        || !profile.additional_text.is_empty()
        || !profile.toggle_schema.is_empty()
        || !profile.initial_variables.is_empty()
        || !profile.metadata.is_empty()
}

fn canonicalize_runtime_capabilities(
    capabilities: &mut Vec<PortableRuntimeCapability>,
) -> CoreResult<()> {
    capabilities.sort_unstable();
    if capabilities.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(unsupported(
            "runtime capability declarations must not contain duplicates",
        ));
    }
    Ok(())
}

fn capability_from_name(name: &str) -> Option<PortableRuntimeCapability> {
    match name {
        "runtime:callbacks" => Some(PortableRuntimeCapability::RuntimeCallbacks),
        "chat:read" => Some(PortableRuntimeCapability::ChatRead),
        "chat:write" => Some(PortableRuntimeCapability::ChatWrite),
        "state:readwrite" => Some(PortableRuntimeCapability::StateReadWrite),
        "profile:read" => Some(PortableRuntimeCapability::ProfileRead),
        "lore:read" => Some(PortableRuntimeCapability::LoreRead),
        "ui:write" => Some(PortableRuntimeCapability::UiWrite),
        "model:primary" => Some(PortableRuntimeCapability::ModelPrimary),
        "model:auxiliary" => Some(PortableRuntimeCapability::ModelAuxiliary),
        "elevated" => Some(PortableRuntimeCapability::Elevated),
        _ => None,
    }
}

fn unsupported(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use lorepia_domain::{
        CharacterRuntimeProfile, PortableRuntimeCapability, PortableRuntimeScript,
    };
    use serde_json::{Map, Value, json};

    use super::{
        intersect_runtime_profile_capabilities, legacy_runtime_capabilities,
        normalize_runtime_profile_capabilities, parse_runtime_capabilities,
    };

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    fn script(id: &str, elevated_access: bool) -> PortableRuntimeScript {
        PortableRuntimeScript {
            id: id.to_owned(),
            name: String::new(),
            event: "load".to_owned(),
            language: "javascript".to_owned(),
            source: "return true".to_owned(),
            elevated_access,
            metadata: BTreeMap::new(),
        }
    }

    fn profile(
        id: &str,
        capabilities: Option<Vec<PortableRuntimeCapability>>,
        elevated_access: bool,
    ) -> CharacterRuntimeProfile {
        CharacterRuntimeProfile {
            source_id: Some(id.to_owned()),
            scripts: vec![script(id, elevated_access)],
            required_capabilities: capabilities,
            ..CharacterRuntimeProfile::default()
        }
    }

    #[test]
    fn missing_declaration_gets_the_fixed_legacy_ceiling() {
        assert_eq!(
            parse_runtime_capabilities(&Map::new()).expect("legacy declaration"),
            Some(legacy_runtime_capabilities())
        );
    }

    #[test]
    fn both_aliases_parse_to_the_same_canonical_declaration() {
        let expected = Some(vec![
            PortableRuntimeCapability::RuntimeCallbacks,
            PortableRuntimeCapability::UiWrite,
            PortableRuntimeCapability::ModelPrimary,
        ]);
        for field in ["requiredCapabilities", "required_capabilities"] {
            let declaration = object(json!({
                (field): ["model:primary", "ui:write", "runtime:callbacks"]
            }));
            assert_eq!(
                parse_runtime_capabilities(&declaration).expect("valid declaration"),
                expected
            );
        }
    }

    #[test]
    fn explicit_empty_array_remains_an_explicit_deny_all_ceiling() {
        let declaration = object(json!({ "requiredCapabilities": [] }));
        assert_eq!(
            parse_runtime_capabilities(&declaration).expect("deny-all declaration"),
            Some(Vec::new())
        );
    }

    #[test]
    fn malformed_or_ambiguous_declarations_fail_closed() {
        for declaration in [
            json!({ "requiredCapabilities": null }),
            json!({ "required_capabilities": null }),
            json!({ "requiredCapabilities": "chat:read" }),
            json!({ "requiredCapabilities": ["chat:read", 1] }),
            json!({ "requiredCapabilities": ["network:direct"] }),
            json!({ "requiredCapabilities": ["chat:read", "chat:read"] }),
            json!({
                "requiredCapabilities": ["chat:read"],
                "required_capabilities": ["ui:write"]
            }),
        ] {
            assert!(
                parse_runtime_capabilities(&object(declaration)).is_err(),
                "malformed capability declaration must be rejected"
            );
        }
    }

    #[test]
    fn capability_declarations_are_canonical_and_closed() {
        let declaration = object(json!({
            "requiredCapabilities": ["ui:write", "runtime:callbacks"]
        }));
        assert_eq!(
            parse_runtime_capabilities(&declaration).expect("declared"),
            Some(vec![
                PortableRuntimeCapability::RuntimeCallbacks,
                PortableRuntimeCapability::UiWrite,
            ])
        );
        assert!(
            parse_runtime_capabilities(&object(json!({
                "requiredCapabilities": ["network:direct"]
            })))
            .is_err(),
            "unknown host authority must fail closed"
        );
    }

    #[test]
    fn empty_target_is_identity_for_a_declared_runtime_profile() {
        let mut target = CharacterRuntimeProfile::default();
        let mut incoming = profile(
            "declared",
            Some(vec![
                PortableRuntimeCapability::RuntimeCallbacks,
                PortableRuntimeCapability::ModelPrimary,
            ]),
            false,
        );
        intersect_runtime_profile_capabilities(&mut target, &mut incoming)
            .expect("merge declared profile");
        assert_eq!(
            target.required_capabilities,
            Some(vec![
                PortableRuntimeCapability::RuntimeCallbacks,
                PortableRuntimeCapability::ModelPrimary,
            ])
        );
    }

    #[test]
    fn undeclared_script_intersects_instead_of_downgrading_declared_authority() {
        let mut target = profile(
            "declared",
            Some(vec![
                PortableRuntimeCapability::RuntimeCallbacks,
                PortableRuntimeCapability::UiWrite,
                PortableRuntimeCapability::ModelPrimary,
            ]),
            false,
        );
        let mut incoming = profile("legacy", None, false);
        intersect_runtime_profile_capabilities(&mut target, &mut incoming)
            .expect("merge legacy profile");
        assert_eq!(
            target.required_capabilities,
            Some(vec![
                PortableRuntimeCapability::RuntimeCallbacks,
                PortableRuntimeCapability::UiWrite,
            ]),
            "legacy authority must be a fixed ceiling, never a wildcard or None"
        );
    }

    #[test]
    fn runtime_capability_intersection_is_archive_order_independent() {
        let declared = profile(
            "declared",
            Some(vec![
                PortableRuntimeCapability::RuntimeCallbacks,
                PortableRuntimeCapability::UiWrite,
                PortableRuntimeCapability::ModelPrimary,
            ]),
            false,
        );
        let legacy = profile("legacy", None, false);

        let mut declared_first = CharacterRuntimeProfile::default();
        let mut declared_incoming = declared.clone();
        intersect_runtime_profile_capabilities(&mut declared_first, &mut declared_incoming)
            .expect("first declaration");
        let mut legacy_incoming = legacy.clone();
        intersect_runtime_profile_capabilities(&mut declared_first, &mut legacy_incoming)
            .expect("then legacy");

        let mut legacy_first = CharacterRuntimeProfile::default();
        let mut legacy_incoming = legacy;
        intersect_runtime_profile_capabilities(&mut legacy_first, &mut legacy_incoming)
            .expect("first legacy");
        let mut declared_incoming = declared;
        intersect_runtime_profile_capabilities(&mut legacy_first, &mut declared_incoming)
            .expect("then declaration");

        assert_eq!(
            declared_first.required_capabilities,
            legacy_first.required_capabilities
        );
        assert_eq!(
            declared_first.required_capabilities,
            Some(vec![
                PortableRuntimeCapability::RuntimeCallbacks,
                PortableRuntimeCapability::UiWrite,
            ])
        );
    }

    #[test]
    fn merged_elevated_script_cannot_outlive_the_capability_intersection() {
        let mut target = profile(
            "elevated",
            Some(vec![
                PortableRuntimeCapability::RuntimeCallbacks,
                PortableRuntimeCapability::Elevated,
            ]),
            true,
        );
        let mut incoming = profile("legacy", None, false);
        intersect_runtime_profile_capabilities(&mut target, &mut incoming)
            .expect("both contributors are independently valid");
        target.scripts.append(&mut incoming.scripts);
        assert!(
            normalize_runtime_profile_capabilities(&mut target).is_err(),
            "a contributor without elevated authority must clamp the merged profile"
        );
    }
}

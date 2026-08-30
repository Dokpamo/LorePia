#[test]
fn character_content_metadata_does_not_duplicate_large_asset_lists() {
    let content = CharacterContentV1 {
        assets: (0..1_411)
            .map(|index| {
                let digest = test_digest(&format!("character-asset-{index}"));
                AssetDescriptor {
                    id: lorepia_domain::AssetId::from(format!("sha256:{}", digest.as_str())),
                    sha256: digest,
                    media_type: "image/png".to_owned(),
                    role: AssetRole::Expression,
                    name: format!("expression-{index}.png"),
                    size_bytes: 12,
                    width: None,
                    height: None,
                    duration_ms: None,
                    source: lorepia_domain::AssetSource {
                        kind: lorepia_domain::AssetSourceKind::CharxPackage,
                        source_sha256: None,
                        logical_path: Some(format!("assets/expressions/{index:04}.png")),
                    },
                }
            })
            .collect(),
        ..CharacterContentV1::default()
    };

    let metadata = character_content_metadata_json(&content, Some(test_digest("plan").as_str()))
        .expect("encode bounded character metadata");
    let value: serde_json::Value =
        serde_json::from_str(&metadata).expect("decode character metadata");
    assert_eq!(value["asset_count"], 1_411);
    assert!(value.get("assets").is_none());
    assert!(metadata.len() < 262_144);
}

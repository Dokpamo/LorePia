use lorepia_domain::ContentCapability;

pub(super) fn supported_capabilities() -> Vec<ContentCapability> {
    vec![
        ContentCapability::PromptFragments,
        ContentCapability::Knowledge,
        ContentCapability::Variables,
        ContentCapability::Transforms,
        ContentCapability::DeclarativeInteractions,
        ContentCapability::PortableRuntime,
        ContentCapability::ImageAssets,
        ContentCapability::AudioAssets,
        ContentCapability::VideoAssets,
        ContentCapability::AttachmentAssets,
    ]
}

pub(super) const fn capability_rank(capability: ContentCapability) -> u8 {
    match capability {
        ContentCapability::PromptFragments => 0,
        ContentCapability::Knowledge => 1,
        ContentCapability::Variables => 2,
        ContentCapability::Transforms => 3,
        ContentCapability::DeclarativeInteractions => 4,
        ContentCapability::PortableRuntime => 5,
        ContentCapability::ImageAssets => 6,
        ContentCapability::AudioAssets => 7,
        ContentCapability::VideoAssets => 8,
        ContentCapability::AttachmentAssets => 9,
        ContentCapability::HighRiskAssets => 10,
    }
}

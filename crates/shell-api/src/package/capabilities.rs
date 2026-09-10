use lorepia_core::{ContentCapability, PackageCapability};

use super::{
    ApprovableContentPackageCapabilityDto, ContentPackageCapabilityDto, ShellResult,
    storage_corrupted,
};

impl From<ContentCapability> for ContentPackageCapabilityDto {
    fn from(value: ContentCapability) -> Self {
        match value {
            ContentCapability::PromptFragments => Self::PromptFragments,
            ContentCapability::Knowledge => Self::Knowledge,
            ContentCapability::Variables => Self::Variables,
            ContentCapability::Transforms => Self::Transforms,
            ContentCapability::DeclarativeInteractions => Self::DeclarativeInteractions,
            ContentCapability::PortableRuntime => Self::PortableRuntime,
            ContentCapability::ImageAssets => Self::ImageAssets,
            ContentCapability::AudioAssets => Self::AudioAssets,
            ContentCapability::VideoAssets => Self::VideoAssets,
            ContentCapability::AttachmentAssets => Self::AttachmentAssets,
            ContentCapability::HighRiskAssets => Self::HighRiskAssets,
        }
    }
}

impl From<PackageCapability> for ContentPackageCapabilityDto {
    fn from(value: PackageCapability) -> Self {
        match value {
            PackageCapability::PromptFragments => Self::PromptFragments,
            PackageCapability::Knowledge => Self::Knowledge,
            PackageCapability::Variables => Self::Variables,
            PackageCapability::Transforms => Self::Transforms,
            PackageCapability::DeclarativeInteractions => Self::DeclarativeInteractions,
            PackageCapability::PortableRuntime => Self::PortableRuntime,
            PackageCapability::ImageAssets => Self::ImageAssets,
            PackageCapability::AudioAssets => Self::AudioAssets,
            PackageCapability::VideoAssets => Self::VideoAssets,
            PackageCapability::AttachmentAssets => Self::AttachmentAssets,
            PackageCapability::HighRiskAssets => Self::HighRiskAssets,
            PackageCapability::ExternalUrls => Self::ExternalUrls,
            PackageCapability::Html => Self::Html,
            PackageCapability::Script => Self::Script,
            PackageCapability::NativeCode => Self::NativeCode,
            PackageCapability::Network => Self::Network,
            PackageCapability::Filesystem => Self::Filesystem,
            PackageCapability::Shell => Self::Shell,
            PackageCapability::Credentials => Self::Credentials,
        }
    }
}

impl From<ApprovableContentPackageCapabilityDto> for PackageCapability {
    fn from(value: ApprovableContentPackageCapabilityDto) -> Self {
        match value {
            ApprovableContentPackageCapabilityDto::Transforms => Self::Transforms,
            ApprovableContentPackageCapabilityDto::DeclarativeInteractions => {
                Self::DeclarativeInteractions
            }
            ApprovableContentPackageCapabilityDto::PortableRuntime => Self::PortableRuntime,
        }
    }
}

pub(super) fn project_approved_capability(
    value: PackageCapability,
) -> ShellResult<ApprovableContentPackageCapabilityDto> {
    match value {
        PackageCapability::Transforms => Ok(ApprovableContentPackageCapabilityDto::Transforms),
        PackageCapability::DeclarativeInteractions => {
            Ok(ApprovableContentPackageCapabilityDto::DeclarativeInteractions)
        }
        PackageCapability::PortableRuntime => {
            Ok(ApprovableContentPackageCapabilityDto::PortableRuntime)
        }
        _ => Err(storage_corrupted(
            "stored package approval contains a non-approvable capability",
        )),
    }
}

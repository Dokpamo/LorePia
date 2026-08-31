//! Core-owned staging and approval boundary for `LorePia` content packages.
//!
//! Native callers may provide a source path exactly once, during inspection.
//! Every later operation accepts only the opaque import identifier and
//! hash/revision expectations returned by this module. Reviewed bytes are
//! promoted to Core-owned content-addressed storage before durable inspection
//! state is created and are re-inspected immediately before later transitions.

mod commit;
mod inspect;
mod lifecycle;
mod review;

pub use commit::{ContentPackageCommitReceipt, ContentPackageCommitRequest};
pub use inspect::ContentPackageImportInspection;
pub use lifecycle::ContentPackageDiscardRequest;
pub use review::{
    ContentPackageApprovalReceipt, ContentPackageApprovalRequest,
    ContentPackageImportApprovalReview, ContentPackageImportReview,
    ContentPackageImportSelectionReview, ContentPackageSelectionReceipt,
    ContentPackageSelectionRequest,
};

#[cfg(test)]
use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::{Path, PathBuf},
};

#[cfg(test)]
use commit::normalize_prepared_document;
#[cfg(test)]
use inspect::{
    package_capability_review, package_snapshot_path, remove_owned_snapshot, stage_content_package,
};
#[cfg(test)]
use lorepia_content::{ContentPackageComponentKind, PreparedContentDocument};
#[cfg(test)]
use lorepia_domain::{
    AssetId, ContentCapability, CoreErrorCode, CoreResult, ImportLimits, Provenance, Sha256Digest,
    SourceKind,
};
#[cfg(test)]
use lorepia_orchestration::PackageComponentKind;
#[cfg(test)]
use lorepia_storage::{
    PackageCapability, PackageCommitDocument, PackageImportStatus, PackageUpdateTargetConfirmation,
    built_in_prompt_presets, package_update_target_confirmations_sha256,
};
#[cfg(test)]
use uuid::Uuid;

#[cfg(test)]
use crate::Core;

#[cfg(test)]
pub(crate) fn discard_content_package_snapshot(
    import_id: &str,
    staging_dir: &Path,
) -> CoreResult<()> {
    let snapshot = package_snapshot_path(staging_dir, import_id)?;
    remove_owned_snapshot(&snapshot, staging_dir, import_id)
}

#[cfg(test)]
mod tests {
    include!("content_package/tests/support.rs");
    include!("content_package/tests/canonical_and_module_authority.rs");
    include!("content_package/tests/durability_and_atomicity.rs");
    include!("content_package/tests/prompt_and_snapshot_security.rs");
}

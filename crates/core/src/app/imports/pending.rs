use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

use lorepia_content::{
    StagedAsset, extract_single_character_transport, prepare_external_import, prepare_import,
    sha256_file,
};
use lorepia_domain::{
    Character, CharacterContentV1, ContentCapability, ContentKind, CoreError, CoreErrorCode,
    CoreResult, ImportInspection, ImportLimits, InspectionId,
};
use lorepia_storage::{
    PackageCapability, PackageDocumentTargetDisposition, PackageUpdateTargetConfirmation,
    StagedAssetImport,
};

use super::staging::{remove_snapshot, snapshot_import_source};
use crate::{
    ContentPackageApprovalRequest, ContentPackageCommitRequest, ContentPackageDiscardRequest,
    ContentPackageSelectionRequest, app::Core,
};

const USER_APPROVED_LARGE_IMPORT_LIMITS: ImportLimits = ImportLimits {
    max_source_bytes: 16 * 1024 * 1024 * 1024,
    max_entries: 8_192,
    max_entry_bytes: 16 * 1024 * 1024 * 1024,
    max_total_uncompressed_bytes: 32 * 1024 * 1024 * 1024,
    max_compression_ratio: 100,
};

pub(in crate::app) type PendingImportRegistry = RwLock<HashMap<InspectionId, PendingImport>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportCommitResult {
    Character(Character),
    Content(ImportedContentSummary),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedContentSummary {
    pub kind: ContentKind,
    pub import_id: String,
    pub display_name: String,
    pub document_count: u32,
    pub asset_count: u32,
}

#[derive(Clone)]
pub(in crate::app) struct PendingImport {
    path: PathBuf,
    limits: ImportLimits,
    inspection: ImportInspection,
    plan_hash: String,
    payload: PendingImportPayload,
}

#[derive(Clone)]
enum PendingImportPayload {
    Character {
        character_content: Box<CharacterContentV1>,
        staged_assets: Vec<StagedAsset>,
    },
    External {
        normalized_package_path: PathBuf,
        normalized_package_sha256: String,
        document_count: u32,
    },
}

impl Core {
    pub fn inspect_import(&self, staged_path: impl AsRef<Path>) -> CoreResult<ImportInspection> {
        self.inspect_import_with_limits(staged_path.as_ref(), ImportLimits::default())
    }

    /// Repeats import inspection with the fixed, larger byte envelope selected
    /// by a foreground user. Non-resource archive checks are unchanged.
    pub fn inspect_import_user_approved_large(
        &self,
        staged_path: impl AsRef<Path>,
    ) -> CoreResult<ImportInspection> {
        self.inspect_import_with_limits(staged_path.as_ref(), USER_APPROVED_LARGE_IMPORT_LIMITS)
    }

    fn inspect_import_with_limits(
        &self,
        staged_path: &Path,
        limits: ImportLimits,
    ) -> CoreResult<ImportInspection> {
        let original_snapshot = snapshot_import_source(
            staged_path,
            &self.inner.storage.staging_dir(),
            limits.max_source_bytes,
        )?;
        let snapshot = match extract_single_character_transport(
            &original_snapshot,
            limits,
            &self.inner.storage.staging_dir(),
        ) {
            Ok(Some(extracted)) => {
                remove_snapshot(&original_snapshot, &self.inner.storage.staging_dir())?;
                extracted
            }
            Ok(None) => original_snapshot,
            Err(error) => {
                let _ = remove_snapshot(&original_snapshot, &self.inner.storage.staging_dir());
                return Err(error);
            }
        };

        let pending =
            match prepare_external_import(&snapshot, limits, &self.inner.storage.staging_dir()) {
                Ok(Some(prepared)) => PendingImport {
                    path: snapshot,
                    limits,
                    inspection: prepared.inspection,
                    plan_hash: prepared.plan_hash,
                    payload: PendingImportPayload::External {
                        normalized_package_path: prepared.normalized_package_path,
                        normalized_package_sha256: prepared.normalized_package_sha256,
                        document_count: prepared.document_count,
                    },
                },
                Ok(None) => {
                    let prepared = match prepare_import(
                        &snapshot,
                        limits,
                        &self.inner.storage.staging_dir(),
                    ) {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            let _ = remove_snapshot(&snapshot, &self.inner.storage.staging_dir());
                            return Err(error);
                        }
                    };
                    PendingImport {
                        path: snapshot,
                        limits,
                        inspection: prepared.inspection,
                        plan_hash: prepared.plan_hash,
                        payload: PendingImportPayload::Character {
                            character_content: Box::new(prepared.character_content),
                            staged_assets: prepared.staged_assets,
                        },
                    }
                }
                Err(error) => {
                    let _ = remove_snapshot(&snapshot, &self.inner.storage.staging_dir());
                    return Err(error);
                }
            };
        let inspection = pending.inspection.clone();
        self.inner
            .pending_imports
            .write()
            .map_err(|_| CoreError::internal("pending import lock was poisoned"))?
            .insert(inspection.id.clone(), pending);
        Ok(inspection)
    }

    /// Commits a character-card import while preserving the original Core API.
    pub fn commit_import(&self, inspection_id: &InspectionId) -> CoreResult<Character> {
        let pending = self.claim_pending_import(inspection_id)?;
        if !matches!(pending.payload, PendingImportPayload::Character { .. }) {
            self.restore_pending_import(inspection_id.clone(), pending)?;
            return Err(CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "this inspection contains a content module or preset; use the compatible import commit",
                false,
            ));
        }
        self.commit_character_import(inspection_id, pending)
    }

    /// Commits either a character card or a normalized declarative content import.
    pub fn commit_compatible_import(
        &self,
        inspection_id: &InspectionId,
    ) -> CoreResult<ImportCommitResult> {
        let pending = self.claim_pending_import(inspection_id)?;
        match &pending.payload {
            PendingImportPayload::Character { .. } => self
                .commit_character_import(inspection_id, pending)
                .map(ImportCommitResult::Character),
            PendingImportPayload::External { .. } => self
                .commit_external_import(inspection_id, pending)
                .map(ImportCommitResult::Content),
        }
    }

    fn claim_pending_import(&self, inspection_id: &InspectionId) -> CoreResult<PendingImport> {
        self.inner
            .pending_imports
            .write()
            .map_err(|_| CoreError::internal("pending import lock was poisoned"))?
            .remove(inspection_id)
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "inspection was not found", false)
            })
    }

    fn commit_character_import(
        &self,
        inspection_id: &InspectionId,
        pending: PendingImport,
    ) -> CoreResult<Character> {
        if !pending.inspection.is_allowed() {
            let error = CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "blocked import cannot be committed",
                false,
            );
            self.restore_pending_import(inspection_id.clone(), pending)?;
            return Err(error);
        }
        let PendingImportPayload::Character {
            character_content,
            staged_assets,
        } = &pending.payload
        else {
            return Err(CoreError::internal(
                "character import payload changed after claim",
            ));
        };
        let Ok(verified) = prepare_import(
            &pending.path,
            pending.limits,
            &self.inner.storage.staging_dir(),
        ) else {
            self.restore_pending_import(inspection_id.clone(), pending)?;
            return Err(CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "import source changed or became unsafe after inspection",
                false,
            ));
        };
        let verification_matches = verified.plan_hash == pending.plan_hash
            && verified.character_content == **character_content
            && verified.inspection.source_sha256 == pending.inspection.source_sha256
            && verified.inspection.source_size == pending.inspection.source_size
            && verified.inspection.kind == pending.inspection.kind;
        for asset in &verified.staged_assets {
            let _ = remove_snapshot(&asset.staged_path, &self.inner.storage.staging_dir());
        }
        if !verification_matches {
            let error = CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "import source or normalized inspection plan changed before commit",
                false,
            );
            self.restore_pending_import(inspection_id.clone(), pending)?;
            return Err(error);
        }
        let mut character = Character::new(
            &pending.inspection.display_name,
            &pending.inspection.description,
            &pending.inspection.source_sha256,
        );
        character.avatar_asset_hash =
            reviewed_avatar_asset_hash(&pending.inspection, staged_assets);
        let staged_asset_imports = staged_assets
            .iter()
            .map(|asset| StagedAssetImport {
                staged_path: asset.staged_path.clone(),
                sha256: asset.sha256.clone(),
                media_type: asset.media_type.clone(),
                size_bytes: asset.size_bytes,
            })
            .collect::<Vec<_>>();
        let commit = self.inner.storage.commit_character_import_with_content(
            &pending.path,
            &character,
            character_content,
            &pending.plan_hash,
            pending.inspection.source_size,
            &inspection_id.0,
            &staged_asset_imports,
        );
        match commit {
            Ok(()) => {
                let _ = cleanup_pending_import(&pending, &self.inner.storage.staging_dir());
                Ok(character)
            }
            Err(error) => match self.inner.storage.get_character(&character.id) {
                Ok(committed) => {
                    let _ = cleanup_pending_import(&pending, &self.inner.storage.staging_dir());
                    Ok(committed)
                }
                Err(lookup) if lookup.code == CoreErrorCode::NotFound => {
                    self.restore_pending_import(inspection_id.clone(), pending)?;
                    Err(error)
                }
                Err(_) => Err(error),
            },
        }
    }

    fn commit_external_import(
        &self,
        inspection_id: &InspectionId,
        pending: PendingImport,
    ) -> CoreResult<ImportedContentSummary> {
        if !pending.inspection.is_allowed() {
            let error = CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "blocked import cannot be committed",
                false,
            );
            self.restore_pending_import(inspection_id.clone(), pending)?;
            return Err(error);
        }
        let PendingImportPayload::External {
            normalized_package_path,
            normalized_package_sha256,
            document_count,
        } = &pending.payload
        else {
            return Err(CoreError::internal(
                "external import payload changed after claim",
            ));
        };
        let source_matches = fs::metadata(&pending.path).is_ok_and(|metadata| {
            metadata.is_file() && metadata.len() == pending.inspection.source_size
        }) && sha256_file(&pending.path).as_deref()
            == Ok(pending.inspection.source_sha256.as_str());
        let package_matches = sha256_file(normalized_package_path).as_deref()
            == Ok(normalized_package_sha256.as_str());
        if !source_matches || !package_matches {
            let error = CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "import source or normalized content package changed before commit",
                false,
            );
            self.restore_pending_import(inspection_id.clone(), pending)?;
            return Err(error);
        }

        let result = self.commit_normalized_content_package(
            normalized_package_path,
            inspection_id,
            &pending.inspection,
            *document_count,
            pending.limits,
        );
        match result {
            Ok(summary) => {
                let _ = cleanup_pending_import(&pending, &self.inner.storage.staging_dir());
                Ok(summary)
            }
            Err(error) => {
                self.restore_pending_import(inspection_id.clone(), pending)?;
                Err(error)
            }
        }
    }

    fn commit_normalized_content_package(
        &self,
        normalized_package_path: &Path,
        inspection_id: &InspectionId,
        source_inspection: &ImportInspection,
        document_count: u32,
        limits: ImportLimits,
    ) -> CoreResult<ImportedContentSummary> {
        let inspection =
            self.inspect_content_package_import_with_limits(normalized_package_path, limits)?;
        let import_id = inspection.import_id.clone();
        let selected_component_ids = inspection.inspection.selectable_component_ids();
        let selection = match self.select_content_package_import(
            &import_id,
            &ContentPackageSelectionRequest {
                expected_revision: inspection.revision,
                expected_package_plan_hash: inspection.inspection.plan_hash.clone(),
                expected_review_sha256: inspection.review.review_sha256.clone(),
                expected_capability_review_sha256: inspection.capability_review_sha256.clone(),
                selected_component_ids: selected_component_ids.clone(),
            },
        ) {
            Ok(selection) => selection,
            Err(error) => {
                self.discard_normalized_package_best_effort(&import_id);
                return Err(error);
            }
        };
        let confirmed_update_targets = match compatible_import_update_confirmations(&selection) {
            Ok(confirmations) => confirmations,
            Err(error) => {
                self.discard_normalized_package_best_effort(&import_id);
                return Err(error);
            }
        };
        let approved_capabilities =
            required_package_capability_approvals(&selection.import_plan.required_capabilities);
        let approval = match self.approve_content_package_import(
            &import_id,
            &ContentPackageApprovalRequest {
                expected_revision: selection.import.revision,
                expected_package_plan_hash: inspection.inspection.plan_hash.clone(),
                expected_content_selection_plan_hash: selection
                    .content_selection
                    .selection_plan_hash
                    .clone(),
                expected_review_sha256: inspection.review.review_sha256.clone(),
                expected_import_plan_sha256: selection.import_plan.plan_sha256.clone(),
                expected_capability_review_sha256: inspection.capability_review_sha256.clone(),
                expected_normalization_evidence_sha256: selection
                    .normalization_evidence_sha256
                    .clone(),
                expected_target_review_sha256: selection.target_review.target_review_sha256.clone(),
                confirmed_update_targets,
                approval_id: format!("compatible-import-{}", inspection_id.0),
                enable_component_ids: selected_component_ids,
                approved_capabilities,
            },
        ) {
            Ok(approval) => approval,
            Err(error) => {
                self.discard_normalized_package_best_effort(&import_id);
                return Err(error);
            }
        };
        let commit = self.commit_content_package_import(
            &import_id,
            &ContentPackageCommitRequest {
                expected_revision: approval.import.revision,
                expected_package_plan_hash: inspection.inspection.plan_hash,
                expected_content_selection_plan_hash: selection
                    .content_selection
                    .selection_plan_hash,
                expected_review_sha256: inspection.review.review_sha256,
                expected_import_plan_sha256: selection.import_plan.plan_sha256,
                expected_approval_sha256: approval.approved_plan.approval_sha256,
                expected_capability_review_sha256: inspection.capability_review_sha256,
                expected_normalization_evidence_sha256: approval.normalization_evidence_sha256,
            },
        )?;
        Ok(ImportedContentSummary {
            kind: source_inspection.kind,
            import_id: commit.import.id,
            display_name: source_inspection.display_name.clone(),
            document_count,
            asset_count: u32::try_from(commit.asset_ids.len()).unwrap_or(u32::MAX),
        })
    }

    fn discard_normalized_package_best_effort(&self, import_id: &str) {
        let Ok(review) = self.get_content_package_import_review(import_id) else {
            return;
        };
        let _ = self.discard_content_package_import(
            import_id,
            &ContentPackageDiscardRequest {
                expected_revision: review.revision,
                expected_review_sha256: review.review_sha256,
                expected_import_plan_sha256: review
                    .selection
                    .map(|selection| selection.import_plan_sha256),
                expected_capability_review_sha256: review.capability_review_sha256,
            },
        );
    }

    pub fn discard_import(&self, inspection_id: &InspectionId) -> CoreResult<()> {
        let pending = self.claim_pending_import(inspection_id)?;
        cleanup_pending_import(&pending, &self.inner.storage.staging_dir())
    }

    fn restore_pending_import(
        &self,
        inspection_id: InspectionId,
        pending: PendingImport,
    ) -> CoreResult<()> {
        let mut imports = self
            .inner
            .pending_imports
            .write()
            .map_err(|_| CoreError::internal("pending import lock was poisoned"))?;
        if let std::collections::hash_map::Entry::Vacant(entry) = imports.entry(inspection_id) {
            entry.insert(pending);
            Ok(())
        } else {
            Err(CoreError::internal(
                "inspection claim collided while restoring a retryable import",
            ))
        }
    }
}

fn compatible_import_update_confirmations(
    selection: &crate::ContentPackageSelectionReceipt,
) -> CoreResult<Vec<PackageUpdateTargetConfirmation>> {
    selection
        .target_review
        .documents
        .iter()
        .filter(|document| document.disposition == PackageDocumentTargetDisposition::Update)
        .map(|document| {
            Ok(PackageUpdateTargetConfirmation {
                source_component_id: document.source_component_id.clone(),
                component_document_ordinal: document.component_document_ordinal,
                target_object_id: document.target_object_id.clone(),
                expected_target_revision_id: document
                    .expected_target_revision_id
                    .clone()
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "reviewed compatible-import update has no immutable revision",
                            false,
                        )
                    })?,
                expected_target_state_revision: document
                    .expected_target_state_revision
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "reviewed compatible-import update has no state revision",
                            false,
                        )
                    })?,
            })
        })
        .collect()
}

fn required_package_capability_approvals(
    capabilities: &[ContentCapability],
) -> Vec<PackageCapability> {
    let mut approvals = capabilities
        .iter()
        .filter_map(|capability| match capability {
            ContentCapability::Transforms => Some(PackageCapability::Transforms),
            ContentCapability::DeclarativeInteractions => {
                Some(PackageCapability::DeclarativeInteractions)
            }
            ContentCapability::PortableRuntime => Some(PackageCapability::PortableRuntime),
            _ => None,
        })
        .collect::<Vec<_>>();
    approvals.sort_unstable();
    approvals.dedup();
    approvals
}

fn reviewed_avatar_asset_hash(
    inspection: &ImportInspection,
    staged_assets: &[StagedAsset],
) -> Option<String> {
    let reviewed_representative = inspection.representative_image.as_ref().and_then(|image| {
        staged_assets.iter().find(|asset| {
            asset.original_path == image.logical_asset_id
                && asset.signature_valid
                && asset.media_type.starts_with("image/")
        })
    });
    reviewed_representative
        .or_else(|| {
            staged_assets
                .iter()
                .find(|asset| asset.signature_valid && asset.media_type.starts_with("image/"))
        })
        .map(|asset| asset.sha256.clone())
}

fn cleanup_pending_import(pending: &PendingImport, staging_dir: &Path) -> CoreResult<()> {
    let mut first_error = remove_snapshot(&pending.path, staging_dir).err();
    match &pending.payload {
        PendingImportPayload::Character { staged_assets, .. } => {
            for asset in staged_assets {
                if let Err(error) = remove_snapshot(&asset.staged_path, staging_dir)
                    && first_error.is_none()
                {
                    first_error = Some(error);
                }
            }
        }
        PendingImportPayload::External {
            normalized_package_path,
            ..
        } => {
            if let Err(error) = remove_snapshot(normalized_package_path, staging_dir)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write as _};

    use tempfile::tempdir;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use super::*;
    use crate::CoreConfig;

    #[test]
    fn reviewed_resource_limits_are_reused_for_commit_revalidation() {
        let default_limits = ImportLimits::default();
        assert!(
            std::hint::black_box(USER_APPROVED_LARGE_IMPORT_LIMITS.max_source_bytes)
                >= 10 * 1024 * 1024 * 1024
        );
        assert_eq!(
            USER_APPROVED_LARGE_IMPORT_LIMITS.max_entries,
            default_limits.max_entries
        );
        assert_eq!(
            USER_APPROVED_LARGE_IMPORT_LIMITS.max_compression_ratio,
            default_limits.max_compression_ratio
        );
        let root = tempdir().expect("data root");
        let source = root.path().join("resource-review.charx");
        let card = br#"{"spec":"chara_card_v3","data":{"name":"Resource review"}}"#;
        let mut asset = b"\x89PNG\r\n\x1a\n".to_vec();
        asset.resize(card.len() + 1, 0);
        let mut archive = ZipWriter::new(File::create(&source).expect("source"));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive.start_file("card.json", options).expect("card");
        archive.write_all(card).expect("card bytes");
        archive
            .start_file("assets/large.png", options)
            .expect("asset");
        archive.write_all(&asset).expect("asset bytes");
        archive.finish().expect("finish archive");

        let core = Core::open(CoreConfig::new(root.path().join("library"))).expect("core");
        let standard = ImportLimits {
            max_entry_bytes: card.len() as u64,
            ..ImportLimits::default()
        };
        let error = core
            .inspect_import_with_limits(&source, standard)
            .expect_err("asset exceeds the simulated standard limit");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
        assert!(error.recoverable);

        let approved = ImportLimits {
            max_entry_bytes: asset.len() as u64,
            ..standard
        };
        let inspection = core
            .inspect_import_with_limits(&source, approved)
            .expect("approved resource inspection");
        let character = core
            .commit_import(&inspection.id)
            .expect("commit must reuse approved limits");
        let content = core.get_character_content(&character.id).expect("content");
        assert_eq!(content.value.assets.len(), 1);
        assert_eq!(content.value.assets[0].size_bytes, asset.len() as u64);
    }

    #[test]
    fn large_resource_envelope_does_not_allow_archive_path_escape() {
        let root = tempdir().expect("data root");
        let source = root.path().join("traversal.charx");
        let mut archive = ZipWriter::new(File::create(&source).expect("source"));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive.start_file("card.json", options).expect("card");
        archive
            .write_all(br#"{"spec":"chara_card_v3","data":{"name":"Traversal"}}"#)
            .expect("card bytes");
        archive
            .start_file("../escape.png", options)
            .expect("escape entry");
        archive
            .write_all(b"\x89PNG\r\n\x1a\n")
            .expect("escape bytes");
        archive.finish().expect("finish archive");

        let core = Core::open(CoreConfig::new(root.path().join("library"))).expect("core");
        let error = core
            .inspect_import_user_approved_large(&source)
            .expect_err("path escape remains blocked");
        assert_eq!(error.code, CoreErrorCode::UnsafeArchive);
    }
}

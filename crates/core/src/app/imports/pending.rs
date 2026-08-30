use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

use lorepia_content::{StagedAsset, prepare_import};
use lorepia_domain::{
    Character, CharacterContentV1, CoreError, CoreErrorCode, CoreResult, ImportInspection,
    ImportLimits, InspectionId,
};
use lorepia_storage::StagedAssetImport;

use super::staging::{remove_snapshot, snapshot_import_source};
use crate::app::Core;

pub(in crate::app) type PendingImportRegistry = RwLock<HashMap<InspectionId, PendingImport>>;

#[derive(Clone)]
pub(in crate::app) struct PendingImport {
    path: PathBuf,
    inspection: ImportInspection,
    character_content: CharacterContentV1,
    plan_hash: String,
    staged_assets: Vec<StagedAsset>,
}

impl Core {
    pub fn inspect_import(&self, staged_path: impl AsRef<Path>) -> CoreResult<ImportInspection> {
        let limits = ImportLimits::default();
        let snapshot = snapshot_import_source(
            staged_path.as_ref(),
            &self.inner.storage.staging_dir(),
            limits.max_source_bytes,
        )?;
        let prepared = match prepare_import(&snapshot, limits, &self.inner.storage.staging_dir()) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = fs::remove_file(&snapshot);
                return Err(error);
            }
        };
        let inspection = prepared.inspection;
        self.inner
            .pending_imports
            .write()
            .map_err(|_| CoreError::internal("pending import lock was poisoned"))?
            .insert(
                inspection.id.clone(),
                PendingImport {
                    path: snapshot,
                    inspection: inspection.clone(),
                    character_content: prepared.character_content,
                    plan_hash: prepared.plan_hash,
                    staged_assets: prepared.staged_assets,
                },
            );
        Ok(inspection)
    }

    pub fn commit_import(&self, inspection_id: &InspectionId) -> CoreResult<Character> {
        let pending = self
            .inner
            .pending_imports
            .write()
            .map_err(|_| CoreError::internal("pending import lock was poisoned"))?
            .remove(inspection_id)
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "inspection was not found", false)
            })?;
        if !pending.inspection.is_allowed() {
            let error = CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "blocked import cannot be committed",
                false,
            );
            self.restore_pending_import(inspection_id.clone(), pending)?;
            return Err(error);
        }
        let Ok(verified) = prepare_import(
            &pending.path,
            ImportLimits::default(),
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
            && verified.character_content == pending.character_content
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
            reviewed_avatar_asset_hash(&pending.inspection, &pending.staged_assets);
        let staged_assets = pending
            .staged_assets
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
            &pending.character_content,
            &pending.plan_hash,
            pending.inspection.source_size,
            &inspection_id.0,
            &staged_assets,
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

    pub fn discard_import(&self, inspection_id: &InspectionId) -> CoreResult<()> {
        let pending = self
            .inner
            .pending_imports
            .write()
            .map_err(|_| CoreError::internal("pending import lock was poisoned"))?
            .remove(inspection_id)
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "inspection was not found", false)
            })?;
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
    for asset in &pending.staged_assets {
        if let Err(error) = remove_snapshot(&asset.staged_path, staging_dir)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

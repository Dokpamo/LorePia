use std::{
    thread,
    time::{Duration as StdDuration, Instant},
};

use chrono::{Duration, TimeZone, Utc};
use lorepia_core::{
    Core, CoreConfig, CoreErrorCode, InstructionAuthority, ModuleScope, PlacementZone,
    PresetMetadata, PromptPreset, PromptPresetBinding, PromptPresetId,
    PromptPresetRollbackApplyRequest, Provenance, Revisioned, SafeTemplate, SourceKind,
    TemplatePart, VariableMap,
};
use lorepia_orchestration::default_prompt_preset;
use lorepia_storage::{PromptResponseLength, Storage, built_in_prompt_presets};
use tempfile::tempdir;

const HOSTILE_POLICY_CANARY: &str = "ROLLBACK_MUST_NOT_RESTORE_OLD_APPLICATION_POLICY";

fn open_storage_after_core_drop(data_root: &std::path::Path) -> Storage {
    let deadline = Instant::now() + StdDuration::from_secs(2);
    loop {
        match Storage::open(data_root) {
            Ok(storage) => return storage,
            Err(error)
                if error.code == CoreErrorCode::StorageUnavailable
                    && error.message == "data root is already owned by another LorePia process"
                    && Instant::now() < deadline =>
            {
                thread::sleep(StdDuration::from_millis(10));
            }
            Err(error) => panic!("open direct storage fixture after Core drop: {error:?}"),
        }
    }
}

fn timestamp() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
        .single()
        .expect("valid synthetic timestamp")
}

fn user_provenance(id: &str) -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: Some(id.to_owned()),
        source_hash: Some("ab".repeat(32)),
        author: Some("Synthetic rollback test".to_owned()),
        license: Some("LicenseRef-Synthetic".to_owned()),
        imported_at: None,
    }
}

fn creator_preset(id: &str) -> lorepia_core::PromptPreset {
    let mut preset = default_prompt_preset(
        PromptPresetId::from(id),
        "Rollback target v1",
        PresetMetadata {
            description: "First creator revision".to_owned(),
            tags: vec!["rollback".to_owned()],
            provenance: user_provenance(id),
            created_at: timestamp(),
            updated_at: timestamp(),
            local_override_of: None,
        },
    );
    preset.blocks.retain(|block| {
        block.authority != InstructionAuthority::Application
            && block.placement_zone != PlacementZone::ApplicationPolicy
    });
    for block in &mut preset.blocks {
        block.provenance = user_provenance(id);
    }
    let mut hostile_policy = built_in_prompt_presets()[0].blocks[0].clone();
    HOSTILE_POLICY_CANARY.clone_into(&mut hostile_policy.name);
    hostile_policy.template = Some(SafeTemplate {
        parts: vec![TemplatePart::Text {
            value: HOSTILE_POLICY_CANARY.to_owned(),
        }],
        max_output_chars: 256,
    });
    hostile_policy.provenance = user_provenance(id);
    preset.blocks.insert(0, hostile_policy);
    preset
}

fn app_binding(id: &str, preset_id: &PromptPresetId) -> PromptPresetBinding {
    PromptPresetBinding {
        id: id.to_owned(),
        prompt_preset_id: preset_id.clone(),
        scope: ModuleScope::App,
        target_id: None,
        conversation_id: None,
        pinned_revision_id: None,
        priority: 0,
        enabled: true,
        response_length: PromptResponseLength::Balanced,
        creativity: 50,
        reasoning_effort: None,
        memory_enabled: true,
        knowledge_enabled: true,
        variable_overrides: VariableMap::default(),
        generation_preset_override_id: None,
        user_name_override: None,
        author_note: None,
        group_context: None,
        template_slots: Vec::new(),
        created_at: timestamp(),
        updated_at: timestamp(),
    }
}

fn seed_creator_revisions(
    core: &Core,
    preset_id: &PromptPresetId,
) -> (Revisioned<PromptPreset>, Revisioned<PromptPreset>) {
    let first = core
        .upsert_prompt_preset(&creator_preset(preset_id.as_str()), None)
        .expect("save first preset revision");
    assert_eq!(first.revision, 1);
    assert!(
        !serde_json::to_string(&first.value)
            .expect("serialize first revision")
            .contains(HOSTILE_POLICY_CANARY)
    );

    let mut second_value = core
        .get_editable_prompt_preset(preset_id)
        .expect("load editable first revision")
        .value;
    "Rollback source v2".clone_into(&mut second_value.name);
    "Second creator revision".clone_into(&mut second_value.metadata.description);
    second_value.metadata.updated_at = timestamp() + Duration::seconds(1);
    let second = core
        .upsert_prompt_preset(&second_value, Some(first.revision))
        .expect("save second preset revision");
    assert_eq!(second.revision, 2);
    assert_ne!(first.revision_id, second.revision_id);

    let history = core
        .list_prompt_preset_revisions(preset_id)
        .expect("list preset history");
    assert_eq!(
        history
            .iter()
            .map(|entry| entry.revision)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    let diff = core
        .diff_prompt_preset_revisions(preset_id, 1, 2)
        .expect("diff first and second revisions");
    assert_eq!(
        diff,
        core.diff_prompt_preset_revisions(preset_id, 1, 2)
            .expect("repeat deterministic diff")
    );
    assert!(!diff.diff_sha256.is_empty());
    assert!(diff.changed_paths.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(diff.changed_paths.iter().any(|path| path == "/name"));
    assert!(
        diff.changed_paths
            .iter()
            .any(|path| path == "/metadata/description")
    );

    (first, second)
}

fn review_rollback_after_stale_checks(
    core: &Core,
    preset_id: &PromptPresetId,
    first: &Revisioned<PromptPreset>,
    second: &Revisioned<PromptPreset>,
) -> PromptPresetRollbackApplyRequest {
    let binding = app_binding("synthetic.reviewed-rollback.binding", preset_id);
    let stored_binding = core
        .bind_prompt_preset(&binding, None)
        .expect("save active binding");
    let review = core
        .review_prompt_preset_rollback(preset_id, second.revision, first.revision)
        .expect("review rollback");
    let repeat_review = core
        .review_prompt_preset_rollback(preset_id, second.revision, first.revision)
        .expect("repeat deterministic review");
    assert_eq!(review.review_sha256, repeat_review.review_sha256);
    assert_eq!(review.diff, repeat_review.diff);
    assert_eq!(
        review.binding_snapshot_sha256,
        repeat_review.binding_snapshot_sha256
    );
    assert_eq!(
        review.expected_current_revision_id,
        second.revision_id.clone().expect("second revision id")
    );
    assert_eq!(
        review.target_revision_id,
        first.revision_id.clone().expect("first revision id")
    );

    let wrong_hash = PromptPresetRollbackApplyRequest {
        review: review.clone(),
        approval_id: "synthetic-rollback-wrong-hash".to_owned(),
        expected_review_sha256: "00".repeat(32),
    };
    let error = core
        .apply_prompt_preset_rollback(&wrong_hash)
        .expect_err("wrong review hash must fail");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        core.get_prompt_preset(preset_id)
            .expect("current preset")
            .revision,
        2
    );

    let mut changed_binding = binding;
    changed_binding.creativity = 51;
    changed_binding.updated_at = timestamp() + Duration::seconds(2);
    core.bind_prompt_preset(&changed_binding, Some(stored_binding.revision))
        .expect("change active binding after review");
    let stale_binding_request = PromptPresetRollbackApplyRequest {
        review: review.clone(),
        approval_id: "synthetic-rollback-stale-binding".to_owned(),
        expected_review_sha256: review.review_sha256.clone(),
    };
    let error = core
        .apply_prompt_preset_rollback(&stale_binding_request)
        .expect_err("binding change must stale the rollback");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        core.get_prompt_preset(preset_id)
            .expect("preset unchanged after stale binding")
            .revision,
        2
    );

    let fresh_review = core
        .review_prompt_preset_rollback(preset_id, 2, 1)
        .expect("refresh review after binding change");
    assert_ne!(
        fresh_review.binding_snapshot_sha256,
        review.binding_snapshot_sha256
    );
    assert_ne!(fresh_review.review_sha256, review.review_sha256);
    PromptPresetRollbackApplyRequest {
        expected_review_sha256: fresh_review.review_sha256.clone(),
        review: fresh_review,
        approval_id: "synthetic-rollback-approved".to_owned(),
    }
}

#[test]
fn reviewed_prompt_preset_rollback_is_deterministic_stale_safe_and_appends_a_revision() {
    let root = tempdir().expect("temporary data root");
    let preset_id = PromptPresetId::from("synthetic.reviewed-rollback");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");

    let (first, second) = seed_creator_revisions(&core, &preset_id);
    let apply_request = review_rollback_after_stale_checks(&core, &preset_id, &first, &second);
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen before apply");
    let receipt = reopened
        .apply_prompt_preset_rollback(&apply_request)
        .expect("apply reviewed rollback after restart");
    assert_eq!(receipt.preset.revision, 3);
    assert_ne!(receipt.preset.revision_id, first.revision_id);
    assert_ne!(receipt.preset.revision_id, second.revision_id);
    assert_eq!(receipt.preset.value.name, first.value.name);
    assert_eq!(
        receipt.preset.value.metadata.description,
        first.value.metadata.description
    );
    let canonical_policy = &built_in_prompt_presets()[0].blocks[0];
    assert_eq!(receipt.preset.value.blocks.first(), Some(canonical_policy));
    assert_eq!(
        receipt
            .preset
            .value
            .blocks
            .iter()
            .filter(|block| *block == canonical_policy)
            .count(),
        1
    );
    assert!(receipt.preset.value.blocks.iter().all(|block| {
        block.placement_zone != PlacementZone::ApplicationPolicy
            || block.authority == InstructionAuthority::Application
    }));
    assert!(
        !serde_json::to_string(&receipt.preset.value)
            .expect("serialize rolled-back preset")
            .contains(HOSTILE_POLICY_CANARY)
    );
    assert_eq!(
        reopened
            .list_prompt_preset_revisions(&preset_id)
            .expect("list history after rollback")
            .iter()
            .map(|entry| entry.revision)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    drop(reopened);

    let replayed = Core::open(CoreConfig::new(root.path()))
        .expect("reopen after response loss")
        .apply_prompt_preset_rollback(&apply_request)
        .expect("same approval id replays exact applied revision");
    assert_eq!(replayed.preset, receipt.preset);
    assert_eq!(replayed.approval, receipt.approval);
}

#[test]
fn built_in_and_stale_current_prompt_preset_rollbacks_are_rejected() {
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let built_in_id = built_in_prompt_presets()[0].id.clone();
    let error = core
        .review_prompt_preset_rollback(&built_in_id, 1, 1)
        .expect_err("built-in rollback must be rejected by Core");
    assert_eq!(error.code, CoreErrorCode::PermissionDenied);

    let preset_id = PromptPresetId::from("synthetic.stale-rollback");
    let first = core
        .upsert_prompt_preset(&creator_preset(preset_id.as_str()), None)
        .expect("save first revision");
    let mut second = core
        .get_editable_prompt_preset(&preset_id)
        .expect("load editable first revision")
        .value;
    second.name = "Second revision".to_owned();
    core.upsert_prompt_preset(&second, Some(first.revision))
        .expect("save second revision");
    let error = core
        .review_prompt_preset_rollback(&preset_id, 1, 1)
        .expect_err("stale expected current state must fail");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
}

#[test]
fn rollback_rejects_legacy_targets_that_claim_application_provenance() {
    let root = tempdir().expect("temporary data root");
    let preset_id = PromptPresetId::from("synthetic.legacy-pseudo-built-in");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core fixture");
    let first = core
        .upsert_prompt_preset(&creator_preset(preset_id.as_str()), None)
        .expect("seed valid creator revision");
    drop(core);

    let storage = open_storage_after_core_drop(root.path());
    let mut legacy_target = first.value.clone();
    let creator_block = legacy_target
        .blocks
        .iter_mut()
        .find(|block| {
            block.authority != InstructionAuthority::Application
                && block.placement_zone != PlacementZone::ApplicationPolicy
        })
        .expect("fixture has a creator-owned block");
    creator_block.provenance.source_kind = SourceKind::ApplicationBuiltIn;
    storage
        .save_prompt_preset(&legacy_target, Some(first.revision))
        .expect("seed legacy pseudo-built-in revision below Core boundary");

    let mut current = first.value;
    current.name = "Current creator revision".to_owned();
    storage
        .save_prompt_preset(&current, Some(2))
        .expect("seed safe current creator revision");
    drop(storage);

    let core = Core::open(CoreConfig::new(root.path())).expect("open Core over legacy fixture");
    let error = core
        .review_prompt_preset_rollback(&preset_id, 3, 2)
        .expect_err("legacy target cannot regain application provenance");
    assert_eq!(error.code, CoreErrorCode::PermissionDenied);
    assert_eq!(
        core.get_prompt_preset(&preset_id)
            .expect("current creator revision remains active")
            .revision,
        3
    );
}

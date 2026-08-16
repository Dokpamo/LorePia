use chrono::{TimeZone, Utc};
use lorepia_domain::{
    ComponentHash, ContentCapability, ContentModule, ContentModuleId, ContentModuleRevision,
    ConversationId, KnowledgeBookId, LocalUserId, ModuleBinding, ModuleBindingId,
    ModuleComponentRef, ModuleConflictResolution, ModuleRevisionId, ModuleRevisionResolutionMode,
    ModuleScope, PackageMetadata, Provenance, Sha256Digest, SourceKind, VariableMap,
};
use lorepia_orchestration::{
    IgnoredModuleBindingReason, ModuleComponentChangeKind, ModuleMergeError,
    ModuleMergeResolutionSet, ModuleResolutionContext, ModuleRevisionSnapshot,
    ModuleRollbackPolicy, diff_module_revisions, prepare_module_rollback, resolve_module_merge,
    review_module_merge, review_module_rollback,
};

fn digest(byte: &str) -> Sha256Digest {
    Sha256Digest::parse(byte.repeat(32)).expect("synthetic digest")
}

fn provenance() -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: None,
        source_hash: None,
        author: Some("Synthetic Author".to_owned()),
        license: Some("LicenseRef-Private".to_owned()),
        imported_at: None,
    }
}

fn snapshot(
    module_id: &str,
    revision_id: &str,
    previous_revision_id: Option<&str>,
    version: &str,
    component_hash: &str,
) -> ModuleRevisionSnapshot {
    let knowledge_id = KnowledgeBookId::from("synthetic.shared-knowledge");
    let timestamp = Utc
        .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
        .single()
        .expect("valid timestamp");
    ModuleRevisionSnapshot {
        module: ContentModule {
            id: ContentModuleId::from(module_id),
            name: format!("Synthetic {module_id}"),
            version: version.to_owned(),
            schema_version: 1,
            prompt_fragments: Vec::new(),
            knowledge_book_ids: vec![knowledge_id.clone()],
            control_specs: Vec::new(),
            transform_set_ids: Vec::new(),
            interaction_rule_set_ids: Vec::new(),
            asset_ids: Vec::new(),
            imported_components_enabled: false,
            required_capabilities: vec![ContentCapability::Knowledge],
            metadata: PackageMetadata {
                author: Some("Synthetic Author".to_owned()),
                license: "LicenseRef-Private".to_owned(),
                redistribution_allowed: false,
                homepage: None,
                description: "Synthetic module".to_owned(),
                tags: Vec::new(),
                provenance: provenance(),
            },
        },
        revision: ContentModuleRevision {
            id: ModuleRevisionId::from(revision_id),
            module_id: ContentModuleId::from(module_id),
            version: version.to_owned(),
            source_hash: digest(if version == "1.0.0" { "a1" } else { "a2" }),
            previous_revision_id: previous_revision_id.map(ModuleRevisionId::from),
            component_hashes: vec![ComponentHash {
                component: ModuleComponentRef::KnowledgeBook { id: knowledge_id },
                sha256: digest(component_hash),
            }],
            created_at: timestamp,
        },
        import_approval: None,
    }
}

fn binding(
    id: &str,
    module_id: &str,
    revision_id: &str,
    scope: ModuleScope,
    target_id: Option<&str>,
    approved: bool,
) -> ModuleBinding {
    ModuleBinding {
        id: ModuleBindingId::from(id),
        module_id: ContentModuleId::from(module_id),
        scope,
        target_id: target_id.map(str::to_owned),
        conversation_id: (scope == ModuleScope::Branch)
            .then(|| ConversationId("synthetic.conversation".to_owned())),
        priority: 0,
        resolution_mode: ModuleRevisionResolutionMode::Active,
        pinned_revision_id: None,
        enabled: true,
        approved,
        package_import_approval_id: None,
        activation_approval_id: approved.then(|| format!("approval-{id}")),
        activation_review_sha256: approved.then(|| digest("e1")),
        activation_plan_sha256: approved.then(|| digest("e2")),
        variable_overrides: VariableMap::default(),
        revision_id: ModuleRevisionId::from(revision_id),
        created_at: Utc
            .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
            .single()
            .expect("valid timestamp"),
    }
}

fn context() -> ModuleResolutionContext {
    ModuleResolutionContext {
        local_user_id: LocalUserId::from("synthetic.local-user"),
        persona_id: None,
        character_id: Some("synthetic.character".to_owned()),
        conversation_id: Some("synthetic.conversation".to_owned()),
        branch_id: Some("synthetic.branch".to_owned()),
        supported_capabilities: vec![ContentCapability::Knowledge],
    }
}

#[test]
fn module_merge_is_deterministic_inert_before_approval_and_conflict_explicit() {
    let app = snapshot("synthetic.app", "synthetic.rev-app", None, "1.0.0", "11");
    let branch = snapshot(
        "synthetic.branch-module",
        "synthetic.rev-branch",
        None,
        "1.0.0",
        "22",
    );
    let app_binding = binding(
        "synthetic.binding-app",
        "synthetic.app",
        "synthetic.rev-app",
        ModuleScope::App,
        None,
        true,
    );
    let branch_binding = binding(
        "synthetic.binding-branch",
        "synthetic.branch-module",
        "synthetic.rev-branch",
        ModuleScope::Branch,
        Some("synthetic.branch"),
        true,
    );
    let pending_binding = binding(
        "synthetic.binding-pending",
        "synthetic.branch-module",
        "synthetic.rev-branch",
        ModuleScope::Branch,
        Some("synthetic.branch"),
        false,
    );

    let first = review_module_merge(
        41,
        &context(),
        &[
            branch_binding.clone(),
            pending_binding.clone(),
            app_binding.clone(),
        ],
        &[branch.clone(), app.clone()],
    )
    .expect("module review");
    let second = review_module_merge(
        41,
        &context(),
        &[app_binding, pending_binding, branch_binding],
        &[app, branch],
    )
    .expect("permuted review");

    assert_eq!(first, second);
    first.verify().expect("review hash");
    assert!(first.ignored_bindings.iter().any(|ignored| {
        ignored.binding_id.as_str() == "synthetic.binding-pending"
            && ignored.reason == IgnoredModuleBindingReason::AwaitingApproval
    }));
    assert_eq!(first.conflicts.len(), 1);
    assert!(matches!(
        resolve_module_merge(
            &first,
            &ModuleMergeResolutionSet {
                expected_review_sha256: first.review_sha256.clone(),
                resolutions: Vec::new(),
            }
        ),
        Err(ModuleMergeError::UnresolvedConflict(_))
    ));

    let conflict = &first.conflicts[0];
    let selected = conflict
        .candidates
        .iter()
        .find(|candidate| candidate.module_id.as_str() == "synthetic.branch-module")
        .expect("branch candidate")
        .clone();
    let plan = resolve_module_merge(
        &first,
        &ModuleMergeResolutionSet {
            expected_review_sha256: first.review_sha256.clone(),
            resolutions: vec![ModuleConflictResolution {
                component: conflict.component.clone(),
                expected_candidates: conflict.candidates.clone(),
                selected: Some(selected),
            }],
        },
    )
    .expect("explicit conflict resolution");

    assert_eq!(plan.expected_state_revision, 41);
    assert_eq!(plan.components.len(), 1);
    assert_eq!(plan.components[0].sha256, digest("22"));
    plan.verify().expect("resolved plan hash");
}

#[test]
fn module_update_has_verifiable_diff_and_hash_bound_rollback() {
    let target = snapshot("synthetic.module", "synthetic.rev-1", None, "1.0.0", "11");
    let current = snapshot(
        "synthetic.module",
        "synthetic.rev-2",
        Some("synthetic.rev-1"),
        "2.0.0",
        "22",
    );
    let diff = diff_module_revisions(&target, &current).expect("revision diff");
    diff.verify().expect("diff hash");
    assert_eq!(diff.component_changes.len(), 1);
    assert_eq!(
        diff.component_changes[0].kind,
        ModuleComponentChangeKind::Modified
    );
    assert!(diff.metadata_changed_fields.contains(&"version".to_owned()));

    let binding = binding(
        "synthetic.binding",
        "synthetic.module",
        "synthetic.rev-2",
        ModuleScope::User,
        None,
        true,
    );
    let rollback_review = review_module_rollback(
        &binding,
        &current,
        &target,
        &[],
        &ModuleRollbackPolicy {
            state_revision: 42,
            maximum_module_schema_version: 1,
            scope_target_exists: true,
            available_asset_ids: Vec::new(),
            supported_capabilities: vec![ContentCapability::Knowledge],
            quarantined_revision_ids: Vec::new(),
            unresolved_components: Vec::new(),
        },
    )
    .expect("rollback review");
    assert!(rollback_review.eligible);
    rollback_review.verify().expect("rollback review hash");

    assert_eq!(
        prepare_module_rollback(&rollback_review, &digest("ff")),
        Err(ModuleMergeError::StaleRollbackReview)
    );
    let plan = prepare_module_rollback(&rollback_review, &rollback_review.review_sha256)
        .expect("rollback plan");
    assert_eq!(plan.expected_state_revision, 42);
    assert_eq!(plan.expected_current_revision_id, current.revision.id);
    assert_eq!(plan.target_revision_id, target.revision.id);
    plan.verify().expect("rollback plan hash");
}

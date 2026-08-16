//! Deterministic content-module composition, review, and rollback planning.

use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{
    AssetId, ComponentHash, ContentCapability, ContentModule, ContentModuleId,
    ContentModuleRevision, LocalUserId, ModuleBinding, ModuleBindingId, ModuleComponentRef,
    ModuleConflict, ModuleConflictCandidate, ModuleConflictResolution, ModuleRevisionId,
    ModuleRevisionResolutionMode, ModuleScope, PackageId, PersonaId, Sha256Digest, SourceKind,
    ValidateOrchestration, VariableMap,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_MODULE_BINDINGS_PER_RESOLUTION: usize = 4_096;
pub const MAX_MODULE_REVISIONS_PER_RESOLUTION: usize = 4_096;
pub const MAX_MODULE_ACTIVATION_APPROVAL_ID_BYTES: usize = 256;

/// Canonical composition identity when no content module applies.
///
/// Prompt planning, interaction policy, generation attempts, and persistence
/// use this shared semantic value instead of subsystem-owned sentinels.
///
/// # Panics
///
/// Panics only if the SHA-256 implementation stops producing a canonical
/// 64-character hexadecimal digest.
#[must_use]
pub fn no_applied_module_runtime_plan_sha256() -> Sha256Digest {
    Sha256Digest::parse(hex::encode(Sha256::digest(
        b"lorepia.applied-module-runtime-plan.none.v1",
    )))
    .expect("literal SHA-256 output is canonical")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleResolutionContext {
    pub local_user_id: LocalUserId,
    pub persona_id: Option<PersonaId>,
    pub character_id: Option<String>,
    pub conversation_id: Option<String>,
    pub branch_id: Option<String>,
    pub supported_capabilities: Vec<ContentCapability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleRevisionSnapshot {
    pub module: ContentModule,
    pub revision: ContentModuleRevision,
    /// Exact completed-package authority, derived by Core or storage. Callers
    /// never supply this value through the activation request.
    #[serde(default)]
    pub import_approval: Option<ModuleImportApprovalEvidence>,
}

/// Exact package commit evidence authorizing one immutable module component.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleImportComponentAuthority {
    pub component: ModuleComponentRef,
    pub component_sha256: Sha256Digest,
    pub package_component_id: String,
    pub package_component_sha256: Sha256Digest,
    pub committed_target_object_id: String,
    pub committed_target_revision_id: String,
    pub committed_result_sha256: Sha256Digest,
    /// Exact asset CAS digest. Document-backed components leave this absent.
    #[serde(default)]
    pub committed_content_sha256: Option<Sha256Digest>,
}

/// Completed package-import authority for one exact module revision.
///
/// Core constructs this only after loading and revalidating the durable
/// completed import, approval payload, selection, capability review, and
/// component commit records. The pure resolver then verifies exact coverage
/// and includes this evidence in both review and plan hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleImportApprovalEvidence {
    pub approval_id: String,
    pub approval_sha256: Sha256Digest,
    pub import_id: String,
    pub import_revision: u64,
    pub package_id: PackageId,
    pub package_source_sha256: Sha256Digest,
    pub selection_sha256: Sha256Digest,
    pub capability_review_sha256: Sha256Digest,
    pub module_id: ContentModuleId,
    pub module_revision_id: ModuleRevisionId,
    pub module_revision_source_sha256: Sha256Digest,
    pub module_package_component_id: String,
    pub module_package_component_sha256: Sha256Digest,
    pub module_commit_result_sha256: Sha256Digest,
    pub selected_package_component_ids: Vec<String>,
    pub authorized_capabilities: Vec<ContentCapability>,
    pub component_authorities: Vec<ModuleImportComponentAuthority>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedModuleImportApproval {
    pub binding_id: ModuleBindingId,
    pub evidence: ModuleImportApprovalEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IgnoredModuleBindingReason {
    Disabled,
    AwaitingApproval,
    DifferentTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IgnoredModuleBinding {
    pub binding_id: ModuleBindingId,
    pub reason: IgnoredModuleBindingReason,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleCandidateSource {
    pub binding_id: ModuleBindingId,
    pub module_id: ContentModuleId,
    pub revision_id: ModuleRevisionId,
    pub revision_source_sha256: Sha256Digest,
    pub scope: ModuleScope,
    pub target_id: Option<String>,
    pub conversation_id: Option<String>,
    pub priority: i32,
    pub module_ordinal: u32,
    /// Declarative runtime intent from this exact immutable module revision.
    /// It remains inert until copied into an approved resolved plan.
    #[serde(default)]
    pub runtime_enabled_intent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedModuleCandidate {
    /// Deterministic representative of every source with this exact hash.
    pub candidate: ModuleConflictCandidate,
    pub sources: Vec<ModuleCandidateSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedModuleComponent {
    pub component: ModuleComponentRef,
    /// One candidate per distinct component hash.
    pub candidates: Vec<ReviewedModuleCandidate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleMergeReview {
    pub review_sha256: Sha256Digest,
    pub state_revision: u64,
    pub context: ModuleResolutionContext,
    /// Pending bindings included only for an explicit activation review.
    pub activation_binding_ids: Vec<ModuleBindingId>,
    pub ordered_bindings: Vec<ModuleBinding>,
    pub ignored_bindings: Vec<IgnoredModuleBinding>,
    pub components: Vec<ReviewedModuleComponent>,
    pub conflicts: Vec<ModuleConflict>,
    #[serde(default)]
    pub import_approvals: Vec<ReviewedModuleImportApproval>,
    pub effective_variable_overrides: VariableMap,
}

impl ModuleMergeReview {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        let expected = module_merge_review_sha256(ModuleMergeReviewDigest {
            state_revision: self.state_revision,
            context: &self.context,
            activation_binding_ids: &self.activation_binding_ids,
            ordered_bindings: &self.ordered_bindings,
            ignored_bindings: &self.ignored_bindings,
            components: &self.components,
            conflicts: &self.conflicts,
            import_approvals: &self.import_approvals,
            effective_variable_overrides: &self.effective_variable_overrides,
        })?;
        if expected != self.review_sha256 {
            return Err(ModuleMergeError::ReviewHashMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleMergeResolutionSet {
    pub expected_review_sha256: Sha256Digest,
    pub resolutions: Vec<ModuleConflictResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedModuleComponent {
    pub component: ModuleComponentRef,
    pub sha256: Sha256Digest,
    pub selected_source: ModuleCandidateSource,
    pub coalesced_sources: Vec<ModuleCandidateSource>,
    /// Approved ephemeral enable overlay. Stored imported documents remain
    /// quarantine-disabled.
    #[serde(default)]
    pub runtime_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedModulePlan {
    pub plan_sha256: Sha256Digest,
    pub review_sha256: Sha256Digest,
    pub expected_state_revision: u64,
    pub activation_binding_ids: Vec<ModuleBindingId>,
    pub ordered_binding_ids: Vec<ModuleBindingId>,
    pub components: Vec<ResolvedModuleComponent>,
    pub omitted_components: Vec<ModuleComponentRef>,
    #[serde(default)]
    pub import_approvals: Vec<ReviewedModuleImportApproval>,
    pub effective_variable_overrides: VariableMap,
}

/// Hash-bound activation review over one pending binding and the complete
/// effective module composition.
pub type ModuleActivationReview = ModuleMergeReview;

/// Hash-bound activation plan derived from [`ModuleActivationReview`].
pub type ModuleActivationPlan = ResolvedModulePlan;

impl ResolvedModulePlan {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        let expected = resolved_module_plan_sha256(
            &self.review_sha256,
            self.expected_state_revision,
            &self.activation_binding_ids,
            &self.ordered_binding_ids,
            &self.components,
            &self.omitted_components,
            &self.import_approvals,
            &self.effective_variable_overrides,
        )?;
        if expected != self.plan_sha256 {
            return Err(ModuleMergeError::PlanHashMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleActivationApproval {
    pub approval_id: String,
    pub expected_review_sha256: Sha256Digest,
    pub expected_plan_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedModuleActivationPlan {
    pub approval_sha256: Sha256Digest,
    pub approval_id: String,
    pub plan: ModuleActivationPlan,
}

impl ApprovedModuleActivationPlan {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        self.plan.verify()?;
        validate_activation_approval_id(&self.approval_id)?;
        if self.plan.activation_binding_ids.len() != 1 {
            return Err(ModuleMergeError::ActivationPlanRequired);
        }
        let expected = module_activation_approval_sha256(&self.approval_id, &self.plan)?;
        if expected != self.approval_sha256 {
            return Err(ModuleMergeError::ActivationApprovalHashMismatch);
        }
        Ok(())
    }
}

/// Exact, hash-bound module composition that trusted runtime code may apply.
///
/// The user-approved activation remains the authority for every selected
/// component. A runtime materialization re-resolves that authority against a
/// no-pending-binding review for one concrete room context. Branch inheritance
/// therefore never fabricates a second user approval: it records the exact
/// applied parent plan it was derived from and rehashes the child context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedModuleRuntimePlan {
    pub applied_plan_sha256: Sha256Digest,
    pub source_approval: ApprovedModuleActivationPlan,
    #[serde(default)]
    pub derived_from_plan_sha256: Option<Sha256Digest>,
    pub review: ModuleMergeReview,
    pub plan: ResolvedModulePlan,
}

impl AppliedModuleRuntimePlan {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        self.source_approval.verify()?;
        self.review.verify()?;
        self.plan.verify()?;
        if !self.review.activation_binding_ids.is_empty()
            || !self.plan.activation_binding_ids.is_empty()
            || self.plan.review_sha256 != self.review.review_sha256
            || self.plan.expected_state_revision != self.review.state_revision
        {
            return Err(ModuleMergeError::InvalidRuntimeMaterialization(
                "runtime plans must resolve one no-pending-binding review".to_owned(),
            ));
        }
        let resolutions = module_merge_resolution_set_from_plan(&self.review, &self.plan)?;
        let reconstructed = resolve_module_merge(&self.review, &resolutions)?;
        if reconstructed != self.plan {
            return Err(ModuleMergeError::InvalidRuntimeMaterialization(
                "runtime plan differs from its reviewed resolution".to_owned(),
            ));
        }
        let expected = applied_module_runtime_plan_sha256(
            &self.source_approval,
            self.derived_from_plan_sha256.as_ref(),
            &self.review,
            &self.plan,
        )?;
        if expected != self.applied_plan_sha256 {
            return Err(ModuleMergeError::RuntimePlanHashMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModuleMergeError {
    #[error("module input exceeds the configured {kind} limit")]
    LimitExceeded { kind: &'static str },
    #[error("module context is invalid: {0}")]
    InvalidContext(String),
    #[error("module binding {binding_id} has an invalid scope target")]
    InvalidBindingTarget { binding_id: String },
    #[error("module binding {binding_id} is invalid: {message}")]
    InvalidBinding { binding_id: String, message: String },
    #[error("duplicate module binding identifier: {0}")]
    DuplicateBinding(String),
    #[error("duplicate module revision identifier: {0}")]
    DuplicateRevision(String),
    #[error("enabled module binding references an unknown revision: {0}")]
    MissingRevision(String),
    #[error("module binding and immutable revision disagree: {0}")]
    BindingRevisionMismatch(String),
    #[error("module revision snapshot is invalid: {0}")]
    InvalidSnapshot(String),
    #[error("imported-package module binding {binding_id} has no completed package authority")]
    MissingImportApproval { binding_id: String },
    #[error("module binding {binding_id} has invalid package authority: {message}")]
    InvalidImportApproval { binding_id: String, message: String },
    #[error("module requires an unsupported capability: {0:?}")]
    UnsupportedCapability(ContentCapability),
    #[error("module review could not be encoded deterministically: {0}")]
    CanonicalEncoding(String),
    #[error("module review hash does not match its canonical payload")]
    ReviewHashMismatch,
    #[error("module review is internally inconsistent: {0}")]
    InvalidReview(String),
    #[error("module resolution references a stale review")]
    StaleReview,
    #[error("module conflict has no explicit resolution: {0:?}")]
    UnresolvedConflict(ModuleComponentRef),
    #[error("module conflict resolution is duplicated: {0:?}")]
    DuplicateResolution(ModuleComponentRef),
    #[error("module conflict resolution targets an unknown component: {0:?}")]
    UnknownResolution(ModuleComponentRef),
    #[error("module conflict candidate set changed after review: {0:?}")]
    StaleConflictCandidates(ModuleComponentRef),
    #[error("selected module conflict candidate was not reviewed: {0:?}")]
    UnknownConflictCandidate(ModuleComponentRef),
    #[error("resolved module plan hash does not match its canonical payload")]
    PlanHashMismatch,
    #[error("module activation draft is invalid: {0}")]
    InvalidActivationDraft(String),
    #[error("module activation requires a plan with exactly one pending binding")]
    ActivationPlanRequired,
    #[error("module activation approval identifier is invalid")]
    InvalidActivationApprovalId,
    #[error("module activation approval references a stale review or plan")]
    StaleActivationApproval,
    #[error("module activation approval hash does not match its canonical payload")]
    ActivationApprovalHashMismatch,
    #[error("module runtime materialization is invalid: {0}")]
    InvalidRuntimeMaterialization(String),
    #[error("module runtime materialization hash does not match its canonical payload")]
    RuntimePlanHashMismatch,
    #[error("module runtime plan cannot be inherited because its reviewed inputs changed")]
    RuntimeDerivationChanged,
    #[error("module runtime plan cannot be inherited across a target branch override")]
    RuntimeTargetBranchOverride,
    #[error("module revisions belong to different modules")]
    DifferentModules,
    #[error("a revision identifier points to different immutable content")]
    CorruptRevisionIdentity,
    #[error("module revision diff hash does not match its canonical payload")]
    DiffHashMismatch,
    #[error("rollback review hash does not match its canonical payload")]
    RollbackReviewHashMismatch,
    #[error("rollback request references a stale review")]
    StaleRollbackReview,
    #[error("module rollback is not eligible")]
    RollbackBlocked,
    #[error("rollback plan hash does not match its canonical payload")]
    RollbackPlanHashMismatch,
    #[error("module rollback activation does not match the reviewed rollback target")]
    RollbackActivationMismatch,
    #[error("approved module rollback hash does not match its canonical payload")]
    RollbackApprovalHashMismatch,
}

/// Create a deterministic merge review for all bindings that apply to a context.
pub fn review_module_merge(
    state_revision: u64,
    context: &ModuleResolutionContext,
    bindings: &[ModuleBinding],
    revisions: &[ModuleRevisionSnapshot],
) -> Result<ModuleMergeReview, ModuleMergeError> {
    review_module_merge_internal(state_revision, context, bindings, revisions, &[])
}

/// Reviews one disabled, unapproved binding as an explicit activation
/// candidate without making it effective in durable state.
pub fn review_module_activation(
    expected_binding_revision: Option<u64>,
    context: &ModuleResolutionContext,
    current_bindings: &[ModuleBinding],
    proposed_binding: &ModuleBinding,
    revisions: &[ModuleRevisionSnapshot],
) -> Result<ModuleMergeReview, ModuleMergeError> {
    if expected_binding_revision == Some(0) {
        return Err(ModuleMergeError::InvalidActivationDraft(
            "an existing binding revision must be positive".to_owned(),
        ));
    }
    proposed_binding
        .validate()
        .map_err(|error| ModuleMergeError::InvalidActivationDraft(error.to_string()))?;
    if proposed_binding.enabled || proposed_binding.approved {
        return Err(ModuleMergeError::InvalidActivationDraft(
            "a proposed activation must remain disabled and unapproved until commit".to_owned(),
        ));
    }
    let current_position = current_bindings
        .iter()
        .position(|binding| binding.id == proposed_binding.id);
    if expected_binding_revision.is_some() != current_position.is_some() {
        return Err(ModuleMergeError::InvalidActivationDraft(
            "expected binding revision does not match current binding existence".to_owned(),
        ));
    }
    let mut reviewed_bindings = current_bindings.to_vec();
    if let Some(position) = current_position {
        reviewed_bindings[position] = proposed_binding.clone();
    } else {
        reviewed_bindings.push(proposed_binding.clone());
    }
    review_module_merge_internal(
        expected_binding_revision.unwrap_or(0),
        context,
        &reviewed_bindings,
        revisions,
        std::slice::from_ref(&proposed_binding.id),
    )
}

#[allow(clippy::too_many_lines)] // The review deliberately keeps all validation before hashing.
fn review_module_merge_internal(
    state_revision: u64,
    context: &ModuleResolutionContext,
    bindings: &[ModuleBinding],
    revisions: &[ModuleRevisionSnapshot],
    activation_binding_ids: &[ModuleBindingId],
) -> Result<ModuleMergeReview, ModuleMergeError> {
    validate_resolution_context(context)?;
    if bindings.len() > MAX_MODULE_BINDINGS_PER_RESOLUTION {
        return Err(ModuleMergeError::LimitExceeded { kind: "binding" });
    }
    if revisions.len() > MAX_MODULE_REVISIONS_PER_RESOLUTION {
        return Err(ModuleMergeError::LimitExceeded { kind: "revision" });
    }
    let activation_binding_count = activation_binding_ids.len();
    let activation_binding_ids = activation_binding_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if activation_binding_ids.len() != activation_binding_count {
        return Err(ModuleMergeError::InvalidActivationDraft(
            "activation binding identifiers must be unique".to_owned(),
        ));
    }

    let mut binding_ids = BTreeSet::new();
    for binding in bindings {
        if !binding_ids.insert(binding.id.clone()) {
            return Err(ModuleMergeError::DuplicateBinding(
                binding.id.as_str().to_owned(),
            ));
        }
        validate_binding_target(binding)?;
        binding
            .validate()
            .map_err(|error| ModuleMergeError::InvalidBinding {
                binding_id: binding.id.as_str().to_owned(),
                message: error.to_string(),
            })?;
    }
    if !activation_binding_ids.is_subset(&binding_ids) {
        return Err(ModuleMergeError::InvalidActivationDraft(
            "activation review references an unknown binding".to_owned(),
        ));
    }
    let revision_map = validated_revision_map(revisions)?;

    let mut ordered_bindings = Vec::new();
    let mut import_approvals = Vec::new();
    let mut ignored_bindings = Vec::new();
    for binding in bindings {
        let activation_candidate = activation_binding_ids.contains(&binding.id);
        let ignored_reason = if !binding.enabled && !activation_candidate {
            Some(IgnoredModuleBindingReason::Disabled)
        } else if !binding.approved && !activation_candidate {
            Some(IgnoredModuleBindingReason::AwaitingApproval)
        } else if !binding_applies(binding, context) {
            Some(IgnoredModuleBindingReason::DifferentTarget)
        } else {
            None
        };
        if let Some(reason) = ignored_reason {
            ignored_bindings.push(IgnoredModuleBinding {
                binding_id: binding.id.clone(),
                reason,
            });
        } else {
            let snapshot = revision_map.get(&binding.revision_id).ok_or_else(|| {
                ModuleMergeError::MissingRevision(binding.revision_id.as_str().to_owned())
            })?;
            if snapshot.module.id != binding.module_id
                || snapshot.revision.module_id != binding.module_id
            {
                return Err(ModuleMergeError::BindingRevisionMismatch(
                    binding.id.as_str().to_owned(),
                ));
            }
            for capability in &snapshot.module.required_capabilities {
                if !context.supported_capabilities.contains(capability) {
                    return Err(ModuleMergeError::UnsupportedCapability(*capability));
                }
            }
            if let Some(evidence) = validate_module_import_approval(binding, snapshot)? {
                import_approvals.push(ReviewedModuleImportApproval {
                    binding_id: binding.id.clone(),
                    evidence: evidence.clone(),
                });
            }
            ordered_bindings.push(binding.clone());
        }
    }
    ordered_bindings.sort_by(binding_order);
    import_approvals.sort_by(|left, right| left.binding_id.cmp(&right.binding_id));
    ignored_bindings.sort_by(|left, right| {
        left.binding_id
            .cmp(&right.binding_id)
            .then_with(|| left.reason.cmp(&right.reason))
    });

    let mut grouped =
        BTreeMap::<ModuleComponentRef, BTreeMap<Sha256Digest, Vec<ModuleCandidateSource>>>::new();
    for binding in &ordered_bindings {
        let snapshot = revision_map.get(&binding.revision_id).ok_or_else(|| {
            ModuleMergeError::MissingRevision(binding.revision_id.as_str().to_owned())
        })?;
        let ordinals = module_component_ordinals(&snapshot.module)?;
        for component in canonical_component_hashes(snapshot)? {
            let ordinal = *ordinals.get(&component.component).ok_or_else(|| {
                ModuleMergeError::InvalidSnapshot(
                    "revision hash has no declared module component".to_owned(),
                )
            })?;
            let runtime_enabled_intent = snapshot.module.imported_components_enabled
                && matches!(
                    &component.component,
                    ModuleComponentRef::TransformSet { .. }
                        | ModuleComponentRef::InteractionRuleSet { .. }
                );
            grouped
                .entry(component.component)
                .or_default()
                .entry(component.sha256)
                .or_default()
                .push(ModuleCandidateSource {
                    binding_id: binding.id.clone(),
                    module_id: binding.module_id.clone(),
                    revision_id: binding.revision_id.clone(),
                    revision_source_sha256: snapshot.revision.source_hash.clone(),
                    scope: binding.scope,
                    target_id: binding.target_id.clone(),
                    conversation_id: binding
                        .conversation_id
                        .as_ref()
                        .map(|conversation_id| conversation_id.0.clone()),
                    priority: binding.priority,
                    module_ordinal: ordinal,
                    runtime_enabled_intent,
                });
        }
    }

    let mut components = Vec::with_capacity(grouped.len());
    let mut conflicts = Vec::new();
    for (component, hashes) in grouped {
        let mut candidates = hashes
            .into_iter()
            .map(|(component_hash, mut sources)| {
                sources.sort_by(candidate_source_order);
                let representative = sources.last().ok_or_else(|| {
                    ModuleMergeError::InvalidSnapshot(
                        "component candidate has no binding source".to_owned(),
                    )
                })?;
                Ok(ReviewedModuleCandidate {
                    candidate: ModuleConflictCandidate {
                        module_id: representative.module_id.clone(),
                        revision_id: representative.revision_id.clone(),
                        component_hash,
                    },
                    sources,
                })
            })
            .collect::<Result<Vec<_>, ModuleMergeError>>()?;
        candidates.sort_by(|left, right| left.candidate.cmp(&right.candidate));
        if candidates.len() > 1 {
            conflicts.push(ModuleConflict {
                component: component.clone(),
                candidates: candidates
                    .iter()
                    .map(|candidate| candidate.candidate.clone())
                    .collect(),
                reason: "same component identifier has distinct immutable hashes".to_owned(),
            });
        }
        components.push(ReviewedModuleComponent {
            component,
            candidates,
        });
    }
    components.sort_by(|left, right| left.component.cmp(&right.component));
    conflicts.sort_by(|left, right| left.component.cmp(&right.component));

    let mut canonical_context = context.clone();
    canonical_context.supported_capabilities.sort();
    canonical_context.supported_capabilities.dedup();
    let activation_binding_ids = activation_binding_ids.into_iter().collect::<Vec<_>>();
    let effective_variable_overrides = compose_variable_overrides(&ordered_bindings);
    let review_sha256 = module_merge_review_sha256(ModuleMergeReviewDigest {
        state_revision,
        context: &canonical_context,
        activation_binding_ids: &activation_binding_ids,
        ordered_bindings: &ordered_bindings,
        ignored_bindings: &ignored_bindings,
        components: &components,
        conflicts: &conflicts,
        import_approvals: &import_approvals,
        effective_variable_overrides: &effective_variable_overrides,
    })?;
    Ok(ModuleMergeReview {
        review_sha256,
        state_revision,
        context: canonical_context,
        activation_binding_ids,
        ordered_bindings,
        ignored_bindings,
        components,
        conflicts,
        import_approvals,
        effective_variable_overrides,
    })
}

#[allow(clippy::too_many_lines)] // Resolution verifies the whole reviewed conflict set atomically.
pub fn resolve_module_merge(
    review: &ModuleMergeReview,
    resolution_set: &ModuleMergeResolutionSet,
) -> Result<ResolvedModulePlan, ModuleMergeError> {
    review.verify()?;
    if resolution_set.expected_review_sha256 != review.review_sha256 {
        return Err(ModuleMergeError::StaleReview);
    }

    let conflict_map = review
        .conflicts
        .iter()
        .map(|conflict| (&conflict.component, conflict))
        .collect::<BTreeMap<_, _>>();
    let mut resolutions = BTreeMap::new();
    for resolution in &resolution_set.resolutions {
        if resolutions
            .insert(&resolution.component, resolution)
            .is_some()
        {
            return Err(ModuleMergeError::DuplicateResolution(
                resolution.component.clone(),
            ));
        }
        if !conflict_map.contains_key(&resolution.component) {
            return Err(ModuleMergeError::UnknownResolution(
                resolution.component.clone(),
            ));
        }
    }

    let mut components = Vec::new();
    let mut omitted_components = Vec::new();
    for reviewed in &review.components {
        let selected = if reviewed.candidates.len() == 1 {
            reviewed.candidates.first()
        } else {
            let conflict = conflict_map.get(&reviewed.component).ok_or_else(|| {
                ModuleMergeError::InvalidReview(
                    "multi-candidate component has no conflict review".to_owned(),
                )
            })?;
            let resolution = resolutions
                .get(&reviewed.component)
                .ok_or_else(|| ModuleMergeError::UnresolvedConflict(reviewed.component.clone()))?;
            let mut expected_candidates = resolution.expected_candidates.clone();
            expected_candidates.sort();
            let mut reviewed_candidates = conflict.candidates.clone();
            reviewed_candidates.sort();
            if expected_candidates != reviewed_candidates {
                return Err(ModuleMergeError::StaleConflictCandidates(
                    reviewed.component.clone(),
                ));
            }
            match &resolution.selected {
                None => None,
                Some(selected) => Some(
                    reviewed
                        .candidates
                        .iter()
                        .find(|candidate| &candidate.candidate == selected)
                        .ok_or_else(|| {
                            ModuleMergeError::UnknownConflictCandidate(reviewed.component.clone())
                        })?,
                ),
            }
        };
        if let Some(selected) = selected {
            let selected_source = selected
                .sources
                .last()
                .ok_or_else(|| {
                    ModuleMergeError::InvalidReview(
                        "reviewed candidate has no binding source".to_owned(),
                    )
                })?
                .clone();
            components.push(ResolvedModuleComponent {
                component: reviewed.component.clone(),
                sha256: selected.candidate.component_hash.clone(),
                runtime_enabled: selected_source.runtime_enabled_intent,
                selected_source,
                coalesced_sources: selected.sources.clone(),
            });
        } else {
            omitted_components.push(reviewed.component.clone());
        }
    }
    if resolutions.len() != review.conflicts.len() {
        let unresolved = review
            .conflicts
            .iter()
            .find(|conflict| !resolutions.contains_key(&conflict.component))
            .ok_or_else(|| {
                ModuleMergeError::InvalidReview(
                    "conflict and resolution counts disagree".to_owned(),
                )
            })?;
        return Err(ModuleMergeError::UnresolvedConflict(
            unresolved.component.clone(),
        ));
    }

    components.sort_by(resolved_component_order);
    omitted_components.sort();
    let ordered_binding_ids = review
        .ordered_bindings
        .iter()
        .map(|binding| binding.id.clone())
        .collect::<Vec<_>>();
    let plan_sha256 = resolved_module_plan_sha256(
        &review.review_sha256,
        review.state_revision,
        &review.activation_binding_ids,
        &ordered_binding_ids,
        &components,
        &omitted_components,
        &review.import_approvals,
        &review.effective_variable_overrides,
    )?;
    Ok(ResolvedModulePlan {
        plan_sha256,
        review_sha256: review.review_sha256.clone(),
        expected_state_revision: review.state_revision,
        activation_binding_ids: review.activation_binding_ids.clone(),
        ordered_binding_ids,
        components,
        omitted_components,
        import_approvals: review.import_approvals.clone(),
        effective_variable_overrides: review.effective_variable_overrides.clone(),
    })
}

pub fn approve_module_activation_plan(
    plan: &ModuleActivationPlan,
    approval: &ModuleActivationApproval,
) -> Result<ApprovedModuleActivationPlan, ModuleMergeError> {
    plan.verify()?;
    validate_activation_approval_id(&approval.approval_id)?;
    if plan.activation_binding_ids.len() != 1 {
        return Err(ModuleMergeError::ActivationPlanRequired);
    }
    if approval.expected_review_sha256 != plan.review_sha256
        || approval.expected_plan_sha256 != plan.plan_sha256
    {
        return Err(ModuleMergeError::StaleActivationApproval);
    }
    let approval_sha256 = module_activation_approval_sha256(&approval.approval_id, plan)?;
    Ok(ApprovedModuleActivationPlan {
        approval_sha256,
        approval_id: approval.approval_id.clone(),
        plan: plan.clone(),
    })
}

/// Rebinds one verified activation approval to an exact no-pending-binding
/// runtime review. The selected immutable components must be identical to the
/// approved activation; otherwise the caller must obtain a new review.
pub fn materialize_approved_module_runtime_plan(
    source_approval: &ApprovedModuleActivationPlan,
    runtime_review: &ModuleMergeReview,
) -> Result<AppliedModuleRuntimePlan, ModuleMergeError> {
    source_approval.verify()?;
    runtime_review.verify()?;
    if !runtime_review.activation_binding_ids.is_empty() {
        return Err(ModuleMergeError::InvalidRuntimeMaterialization(
            "runtime review contains a pending activation".to_owned(),
        ));
    }
    let resolutions =
        module_merge_resolution_set_for_selection(runtime_review, &source_approval.plan)?;
    let plan = resolve_module_merge(runtime_review, &resolutions)?;
    if !same_runtime_plan_selection(&plan, &source_approval.plan) {
        return Err(ModuleMergeError::RuntimeDerivationChanged);
    }
    build_applied_module_runtime_plan(source_approval.clone(), None, runtime_review.clone(), plan)
}

/// Derives an exact context-specific runtime plan from an already verified
/// applied plan.
///
/// The installation identity and supported capabilities remain fixed. All
/// reviewed bindings, immutable candidate sources, conflict sets, selected
/// resolutions, omissions, and variable overrides must remain identical. A
/// branch-scoped binding for the target is an explicit override and therefore
/// always requires a fresh review. This permits app/user-only authority to be
/// materialized for another room while rejecting context-specific composition.
pub fn derive_applied_module_runtime_plan(
    source: &AppliedModuleRuntimePlan,
    target_review: &ModuleMergeReview,
) -> Result<AppliedModuleRuntimePlan, ModuleMergeError> {
    source.verify()?;
    target_review.verify()?;
    if !target_review.activation_binding_ids.is_empty() {
        return Err(ModuleMergeError::InvalidRuntimeMaterialization(
            "derived runtime review contains a pending activation".to_owned(),
        ));
    }
    validate_runtime_derivation_context(&source.review.context, &target_review.context)?;
    if target_review.ordered_bindings.iter().any(|binding| {
        binding.scope == ModuleScope::Branch
            && binding.target_id.as_deref() == target_review.context.branch_id.as_deref()
    }) {
        return Err(ModuleMergeError::RuntimeTargetBranchOverride);
    }
    if source.review.state_revision != target_review.state_revision
        || source.review.ordered_bindings != target_review.ordered_bindings
        || source.review.ignored_bindings != target_review.ignored_bindings
        || source.review.components != target_review.components
        || source.review.conflicts != target_review.conflicts
        || source.review.import_approvals != target_review.import_approvals
        || source.review.effective_variable_overrides != target_review.effective_variable_overrides
    {
        return Err(ModuleMergeError::RuntimeDerivationChanged);
    }
    let resolutions = module_merge_resolution_set_for_selection(target_review, &source.plan)?;
    let plan = resolve_module_merge(target_review, &resolutions)?;
    if !same_runtime_plan_selection(&plan, &source.plan) {
        return Err(ModuleMergeError::RuntimeDerivationChanged);
    }
    build_applied_module_runtime_plan(
        source.source_approval.clone(),
        Some(source.applied_plan_sha256.clone()),
        target_review.clone(),
        plan,
    )
}

/// Reconstructs the explicit conflict choices embedded in a plan that belongs
/// to the supplied review.
pub fn module_merge_resolution_set_from_plan(
    review: &ModuleMergeReview,
    plan: &ResolvedModulePlan,
) -> Result<ModuleMergeResolutionSet, ModuleMergeError> {
    review.verify()?;
    plan.verify()?;
    if plan.review_sha256 != review.review_sha256
        || plan.expected_state_revision != review.state_revision
    {
        return Err(ModuleMergeError::InvalidRuntimeMaterialization(
            "resolved plan does not belong to its review".to_owned(),
        ));
    }
    module_merge_resolution_set_for_selection(review, plan)
}

fn module_merge_resolution_set_for_selection(
    review: &ModuleMergeReview,
    selection: &ResolvedModulePlan,
) -> Result<ModuleMergeResolutionSet, ModuleMergeError> {
    let selected = selection
        .components
        .iter()
        .map(|component| (&component.component, component))
        .collect::<BTreeMap<_, _>>();
    let omitted = selection.omitted_components.iter().collect::<BTreeSet<_>>();
    let mut resolutions = Vec::with_capacity(review.conflicts.len());
    for conflict in &review.conflicts {
        let selected_component = selected.get(&conflict.component).copied();
        let is_omitted = omitted.contains(&conflict.component);
        if selected_component.is_some() == is_omitted {
            return Err(ModuleMergeError::InvalidRuntimeMaterialization(
                "plan does not select or omit each reviewed conflict exactly once".to_owned(),
            ));
        }
        let selected = selected_component.map(|component| ModuleConflictCandidate {
            module_id: component.selected_source.module_id.clone(),
            revision_id: component.selected_source.revision_id.clone(),
            component_hash: component.sha256.clone(),
        });
        if selected
            .as_ref()
            .is_some_and(|candidate| !conflict.candidates.contains(candidate))
        {
            return Err(ModuleMergeError::RuntimeDerivationChanged);
        }
        resolutions.push(ModuleConflictResolution {
            component: conflict.component.clone(),
            expected_candidates: conflict.candidates.clone(),
            selected,
        });
    }
    Ok(ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions,
    })
}

fn validate_runtime_derivation_context(
    source: &ModuleResolutionContext,
    target: &ModuleResolutionContext,
) -> Result<(), ModuleMergeError> {
    if source == target {
        return Err(ModuleMergeError::InvalidRuntimeMaterialization(
            "runtime derivation requires a distinct target context".to_owned(),
        ));
    }
    let mut source_capabilities = source.supported_capabilities.clone();
    source_capabilities.sort();
    source_capabilities.dedup();
    let mut target_capabilities = target.supported_capabilities.clone();
    target_capabilities.sort();
    target_capabilities.dedup();
    if source.local_user_id != target.local_user_id || source_capabilities != target_capabilities {
        return Err(ModuleMergeError::RuntimeDerivationChanged);
    }
    Ok(())
}

fn same_runtime_plan_selection(left: &ResolvedModulePlan, right: &ResolvedModulePlan) -> bool {
    left.ordered_binding_ids == right.ordered_binding_ids
        && left.components == right.components
        && left.omitted_components == right.omitted_components
        && left.import_approvals == right.import_approvals
        && left.effective_variable_overrides == right.effective_variable_overrides
}

fn build_applied_module_runtime_plan(
    source_approval: ApprovedModuleActivationPlan,
    derived_from_plan_sha256: Option<Sha256Digest>,
    review: ModuleMergeReview,
    plan: ResolvedModulePlan,
) -> Result<AppliedModuleRuntimePlan, ModuleMergeError> {
    let applied_plan_sha256 = applied_module_runtime_plan_sha256(
        &source_approval,
        derived_from_plan_sha256.as_ref(),
        &review,
        &plan,
    )?;
    let applied = AppliedModuleRuntimePlan {
        applied_plan_sha256,
        source_approval,
        derived_from_plan_sha256,
        review,
        plan,
    };
    applied.verify()?;
    Ok(applied)
}

fn validate_resolution_context(context: &ModuleResolutionContext) -> Result<(), ModuleMergeError> {
    for (field, value) in [
        ("local_user_id", Some(context.local_user_id.as_str())),
        (
            "persona_id",
            context.persona_id.as_ref().map(PersonaId::as_str),
        ),
        ("character_id", context.character_id.as_deref()),
        ("conversation_id", context.conversation_id.as_deref()),
        ("branch_id", context.branch_id.as_deref()),
    ] {
        if value.is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control))
        {
            return Err(ModuleMergeError::InvalidContext(format!(
                "{field} must be non-empty and free of control characters"
            )));
        }
    }
    if context.branch_id.is_some() && context.conversation_id.is_none() {
        return Err(ModuleMergeError::InvalidContext(
            "a branch context requires a conversation".to_owned(),
        ));
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "package activation verifies one complete immutable authority chain"
)]
fn validate_module_import_approval<'a>(
    binding: &ModuleBinding,
    snapshot: &'a ModuleRevisionSnapshot,
) -> Result<Option<&'a ModuleImportApprovalEvidence>, ModuleMergeError> {
    let imported_package =
        snapshot.module.metadata.provenance.source_kind == SourceKind::ImportedPackage;
    if !imported_package {
        if snapshot.import_approval.is_some() || binding.package_import_approval_id.is_some() {
            return Err(ModuleMergeError::InvalidImportApproval {
                binding_id: binding.id.as_str().to_owned(),
                message: "non-package modules must not carry package approval authority".to_owned(),
            });
        }
        return Ok(None);
    }
    let evidence = snapshot.import_approval.as_ref().ok_or_else(|| {
        ModuleMergeError::MissingImportApproval {
            binding_id: binding.id.as_str().to_owned(),
        }
    })?;
    let invalid = |message: &str| ModuleMergeError::InvalidImportApproval {
        binding_id: binding.id.as_str().to_owned(),
        message: message.to_owned(),
    };
    if binding.package_import_approval_id.as_deref() != Some(evidence.approval_id.as_str()) {
        return Err(invalid(
            "binding approval id does not match completed package authority",
        ));
    }
    for value in [
        evidence.approval_id.as_str(),
        evidence.import_id.as_str(),
        evidence.module_package_component_id.as_str(),
    ] {
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(invalid("package authority contains an invalid identifier"));
        }
    }
    if evidence.import_revision == 0
        || evidence.module_id != snapshot.module.id
        || evidence.module_id != snapshot.revision.module_id
        || evidence.module_revision_id != snapshot.revision.id
        || evidence.module_revision_source_sha256 != snapshot.revision.source_hash
        || snapshot.module.metadata.provenance.source_id.as_deref()
            != Some(evidence.package_id.as_str())
        || snapshot.module.metadata.provenance.source_hash.as_deref()
            != Some(evidence.package_source_sha256.as_str())
    {
        return Err(invalid(
            "package authority does not identify the exact imported module revision",
        ));
    }

    let mut selected_component_ids = evidence.selected_package_component_ids.clone();
    selected_component_ids.sort();
    if selected_component_ids != evidence.selected_package_component_ids
        || selected_component_ids
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        || !selected_component_ids.contains(&evidence.module_package_component_id)
    {
        return Err(invalid(
            "selected package component identifiers are not canonical or omit the module",
        ));
    }
    let mut authorized_capabilities = evidence.authorized_capabilities.clone();
    authorized_capabilities.sort();
    authorized_capabilities.dedup();
    if authorized_capabilities != evidence.authorized_capabilities
        || snapshot
            .module
            .required_capabilities
            .iter()
            .any(|capability| !authorized_capabilities.contains(capability))
    {
        return Err(invalid(
            "completed package authority does not cover required module capabilities",
        ));
    }
    for component in &snapshot.revision.component_hashes {
        let required = match component.component {
            ModuleComponentRef::PromptBlock { .. } => Some(ContentCapability::PromptFragments),
            ModuleComponentRef::Control { .. } => Some(ContentCapability::Variables),
            ModuleComponentRef::KnowledgeBook { .. } => Some(ContentCapability::Knowledge),
            ModuleComponentRef::TransformSet { .. } => Some(ContentCapability::Transforms),
            ModuleComponentRef::InteractionRuleSet { .. } => {
                Some(ContentCapability::DeclarativeInteractions)
            }
            ModuleComponentRef::Asset { .. } => None,
        };
        if required.is_some_and(|capability| !authorized_capabilities.contains(&capability)) {
            return Err(invalid(
                "completed package authority lacks a component capability",
            ));
        }
    }

    let mut component_authorities = evidence.component_authorities.clone();
    component_authorities.sort();
    if component_authorities != evidence.component_authorities
        || component_authorities
            .windows(2)
            .any(|pair| pair[0].component == pair[1].component)
        || component_authorities.len() != snapshot.revision.component_hashes.len()
    {
        return Err(invalid(
            "component package authorities are not canonical and exact",
        ));
    }
    for component in &snapshot.revision.component_hashes {
        let authority = component_authorities
            .iter()
            .find(|authority| authority.component == component.component)
            .ok_or_else(|| invalid("module component has no completed package authority"))?;
        let asset_authority_is_exact = match &component.component {
            ModuleComponentRef::Asset { id } => {
                authority.package_component_id == evidence.module_package_component_id
                    && authority.package_component_sha256
                        == evidence.module_package_component_sha256
                    && authority.committed_target_object_id == id.as_str()
                    && authority.committed_target_revision_id == component.sha256.as_str()
                    && authority.committed_result_sha256 == component.sha256
                    && authority.committed_content_sha256.is_some()
            }
            _ => authority.committed_content_sha256.is_none(),
        };
        if authority.component_sha256 != component.sha256
            || !selected_component_ids.contains(&authority.package_component_id)
            || authority.package_component_id.trim().is_empty()
            || authority.committed_target_object_id.trim().is_empty()
            || authority.committed_target_revision_id.trim().is_empty()
            || !asset_authority_is_exact
        {
            return Err(invalid(
                "module component authority differs from its exact committed revision",
            ));
        }
    }
    Ok(Some(evidence))
}

fn validate_binding_target(binding: &ModuleBinding) -> Result<(), ModuleMergeError> {
    let valid = match binding.scope {
        ModuleScope::App | ModuleScope::User => {
            binding.target_id.is_none() && binding.conversation_id.is_none()
        }
        ModuleScope::Persona | ModuleScope::Character | ModuleScope::Conversation => {
            binding.conversation_id.is_none()
                && binding.target_id.as_ref().is_some_and(|target| {
                    !target.trim().is_empty() && !target.chars().any(char::is_control)
                })
        }
        ModuleScope::Branch => {
            binding.conversation_id.is_some()
                && binding.target_id.as_ref().is_some_and(|target| {
                    !target.trim().is_empty() && !target.chars().any(char::is_control)
                })
        }
    };
    if valid {
        Ok(())
    } else {
        Err(ModuleMergeError::InvalidBindingTarget {
            binding_id: binding.id.as_str().to_owned(),
        })
    }
}

fn binding_applies(binding: &ModuleBinding, context: &ModuleResolutionContext) -> bool {
    match binding.scope {
        ModuleScope::App | ModuleScope::User => true,
        ModuleScope::Persona => {
            binding.target_id.as_deref() == context.persona_id.as_ref().map(PersonaId::as_str)
        }
        ModuleScope::Character => binding.target_id == context.character_id,
        ModuleScope::Conversation => binding.target_id == context.conversation_id,
        ModuleScope::Branch => {
            binding.target_id == context.branch_id
                && binding
                    .conversation_id
                    .as_ref()
                    .map(|conversation_id| conversation_id.0.as_str())
                    == context.conversation_id.as_deref()
        }
    }
}

fn scope_rank(scope: ModuleScope) -> u8 {
    match scope {
        ModuleScope::App => 0,
        ModuleScope::User => 1,
        ModuleScope::Persona => 2,
        ModuleScope::Character => 3,
        ModuleScope::Conversation => 4,
        ModuleScope::Branch => 5,
    }
}

fn binding_order(left: &ModuleBinding, right: &ModuleBinding) -> std::cmp::Ordering {
    scope_rank(left.scope)
        .cmp(&scope_rank(right.scope))
        .then_with(|| left.target_id.cmp(&right.target_id))
        .then_with(|| {
            left.conversation_id
                .as_ref()
                .map(|conversation_id| &conversation_id.0)
                .cmp(
                    &right
                        .conversation_id
                        .as_ref()
                        .map(|conversation_id| &conversation_id.0),
                )
        })
        .then_with(|| left.priority.cmp(&right.priority))
        .then_with(|| left.id.cmp(&right.id))
        .then_with(|| left.module_id.cmp(&right.module_id))
        .then_with(|| left.revision_id.cmp(&right.revision_id))
}

fn compose_variable_overrides(bindings: &[ModuleBinding]) -> VariableMap {
    let mut effective = VariableMap::default();
    for binding in bindings {
        for override_value in &binding.variable_overrides.values {
            effective.insert(
                override_value.variable.clone(),
                override_value.value.clone(),
            );
        }
    }
    effective
}

fn candidate_source_order(
    left: &ModuleCandidateSource,
    right: &ModuleCandidateSource,
) -> std::cmp::Ordering {
    scope_rank(left.scope)
        .cmp(&scope_rank(right.scope))
        .then_with(|| left.target_id.cmp(&right.target_id))
        .then_with(|| left.conversation_id.cmp(&right.conversation_id))
        .then_with(|| left.priority.cmp(&right.priority))
        .then_with(|| left.binding_id.cmp(&right.binding_id))
        .then_with(|| left.module_ordinal.cmp(&right.module_ordinal))
        .then_with(|| left.module_id.cmp(&right.module_id))
        .then_with(|| left.revision_id.cmp(&right.revision_id))
}

fn resolved_component_order(
    left: &ResolvedModuleComponent,
    right: &ResolvedModuleComponent,
) -> std::cmp::Ordering {
    candidate_source_order(&left.selected_source, &right.selected_source)
        .then_with(|| left.component.cmp(&right.component))
}

fn validated_revision_map(
    revisions: &[ModuleRevisionSnapshot],
) -> Result<BTreeMap<ModuleRevisionId, &ModuleRevisionSnapshot>, ModuleMergeError> {
    let mut map = BTreeMap::new();
    for snapshot in revisions {
        validate_revision_snapshot(snapshot)?;
        if map.insert(snapshot.revision.id.clone(), snapshot).is_some() {
            return Err(ModuleMergeError::DuplicateRevision(
                snapshot.revision.id.as_str().to_owned(),
            ));
        }
    }
    Ok(map)
}

fn validate_revision_snapshot(snapshot: &ModuleRevisionSnapshot) -> Result<(), ModuleMergeError> {
    snapshot.module.validate().map_err(|error| {
        ModuleMergeError::InvalidSnapshot(format!("{}: {}", error.path, error.reason))
    })?;
    if snapshot.module.id != snapshot.revision.module_id
        || snapshot.module.version != snapshot.revision.version
    {
        return Err(ModuleMergeError::InvalidSnapshot(
            "module identity or version does not match its immutable revision".to_owned(),
        ));
    }
    if snapshot.revision.previous_revision_id.as_ref() == Some(&snapshot.revision.id) {
        return Err(ModuleMergeError::InvalidSnapshot(
            "revision cannot name itself as its predecessor".to_owned(),
        ));
    }
    let mut declared = module_components(&snapshot.module)?;
    declared.sort();
    let hashed = canonical_component_hashes(snapshot)?;
    let hashed_refs = hashed
        .iter()
        .map(|component| component.component.clone())
        .collect::<Vec<_>>();
    if declared != hashed_refs {
        return Err(ModuleMergeError::InvalidSnapshot(
            "revision hashes do not exactly cover the module components".to_owned(),
        ));
    }
    Ok(())
}

fn module_components(module: &ContentModule) -> Result<Vec<ModuleComponentRef>, ModuleMergeError> {
    let components = module
        .prompt_fragments
        .iter()
        .map(|block| ModuleComponentRef::PromptBlock {
            id: block.id.clone(),
        })
        .chain(
            module
                .control_specs
                .iter()
                .map(|control| ModuleComponentRef::Control {
                    id: control.id.clone(),
                }),
        )
        .chain(
            module
                .knowledge_book_ids
                .iter()
                .map(|id| ModuleComponentRef::KnowledgeBook { id: id.clone() }),
        )
        .chain(
            module
                .transform_set_ids
                .iter()
                .map(|id| ModuleComponentRef::TransformSet { id: id.clone() }),
        )
        .chain(
            module
                .interaction_rule_set_ids
                .iter()
                .map(|id| ModuleComponentRef::InteractionRuleSet { id: id.clone() }),
        )
        .chain(
            module
                .asset_ids
                .iter()
                .map(|id| ModuleComponentRef::Asset { id: id.clone() }),
        )
        .collect::<Vec<_>>();
    if components.iter().collect::<BTreeSet<_>>().len() != components.len() {
        return Err(ModuleMergeError::InvalidSnapshot(
            "module component identifiers must be unique within each kind".to_owned(),
        ));
    }
    Ok(components)
}

fn module_component_ordinals(
    module: &ContentModule,
) -> Result<BTreeMap<ModuleComponentRef, u32>, ModuleMergeError> {
    module_components(module)?
        .into_iter()
        .enumerate()
        .map(|(ordinal, component)| {
            let ordinal = u32::try_from(ordinal).map_err(|_| ModuleMergeError::LimitExceeded {
                kind: "module component",
            })?;
            Ok((component, ordinal))
        })
        .collect()
}

fn canonical_component_hashes(
    snapshot: &ModuleRevisionSnapshot,
) -> Result<Vec<ComponentHash>, ModuleMergeError> {
    let mut hashes = snapshot.revision.component_hashes.clone();
    hashes.sort_by(|left, right| left.component.cmp(&right.component));
    if hashes
        .windows(2)
        .any(|pair| pair[0].component == pair[1].component)
    {
        return Err(ModuleMergeError::InvalidSnapshot(
            "revision component hashes contain a duplicate component".to_owned(),
        ));
    }
    Ok(hashes)
}

#[derive(Serialize)]
struct ModuleMergeReviewDigest<'a> {
    state_revision: u64,
    context: &'a ModuleResolutionContext,
    activation_binding_ids: &'a [ModuleBindingId],
    ordered_bindings: &'a [ModuleBinding],
    ignored_bindings: &'a [IgnoredModuleBinding],
    components: &'a [ReviewedModuleComponent],
    conflicts: &'a [ModuleConflict],
    import_approvals: &'a [ReviewedModuleImportApproval],
    effective_variable_overrides: &'a VariableMap,
}

fn module_merge_review_sha256(
    digest: ModuleMergeReviewDigest<'_>,
) -> Result<Sha256Digest, ModuleMergeError> {
    canonical_sha256(&digest)
}

#[derive(Serialize)]
struct ResolvedModulePlanDigest<'a> {
    review_sha256: &'a Sha256Digest,
    expected_state_revision: u64,
    activation_binding_ids: &'a [ModuleBindingId],
    ordered_binding_ids: &'a [ModuleBindingId],
    components: &'a [ResolvedModuleComponent],
    omitted_components: &'a [ModuleComponentRef],
    import_approvals: &'a [ReviewedModuleImportApproval],
    effective_variable_overrides: &'a VariableMap,
}

#[allow(
    clippy::too_many_arguments,
    reason = "the canonical plan digest binds every independently reviewed input"
)]
fn resolved_module_plan_sha256(
    review_sha256: &Sha256Digest,
    expected_state_revision: u64,
    activation_binding_ids: &[ModuleBindingId],
    ordered_binding_ids: &[ModuleBindingId],
    components: &[ResolvedModuleComponent],
    omitted_components: &[ModuleComponentRef],
    import_approvals: &[ReviewedModuleImportApproval],
    effective_variable_overrides: &VariableMap,
) -> Result<Sha256Digest, ModuleMergeError> {
    canonical_sha256(&ResolvedModulePlanDigest {
        review_sha256,
        expected_state_revision,
        activation_binding_ids,
        ordered_binding_ids,
        components,
        omitted_components,
        import_approvals,
        effective_variable_overrides,
    })
}

#[derive(Serialize)]
struct ModuleActivationApprovalDigest<'a> {
    approval_id: &'a str,
    review_sha256: &'a Sha256Digest,
    plan_sha256: &'a Sha256Digest,
    expected_state_revision: u64,
    activation_binding_ids: &'a [ModuleBindingId],
}

fn module_activation_approval_sha256(
    approval_id: &str,
    plan: &ModuleActivationPlan,
) -> Result<Sha256Digest, ModuleMergeError> {
    canonical_sha256(&ModuleActivationApprovalDigest {
        approval_id,
        review_sha256: &plan.review_sha256,
        plan_sha256: &plan.plan_sha256,
        expected_state_revision: plan.expected_state_revision,
        activation_binding_ids: &plan.activation_binding_ids,
    })
}

#[derive(Serialize)]
struct AppliedModuleRuntimePlanDigest<'a> {
    source_approval_id: &'a str,
    source_approval_sha256: &'a Sha256Digest,
    source_activation_plan_sha256: &'a Sha256Digest,
    derived_from_plan_sha256: Option<&'a Sha256Digest>,
    runtime_review_sha256: &'a Sha256Digest,
    runtime_plan_sha256: &'a Sha256Digest,
}

fn applied_module_runtime_plan_sha256(
    source_approval: &ApprovedModuleActivationPlan,
    derived_from_plan_sha256: Option<&Sha256Digest>,
    review: &ModuleMergeReview,
    plan: &ResolvedModulePlan,
) -> Result<Sha256Digest, ModuleMergeError> {
    canonical_sha256(&AppliedModuleRuntimePlanDigest {
        source_approval_id: &source_approval.approval_id,
        source_approval_sha256: &source_approval.approval_sha256,
        source_activation_plan_sha256: &source_approval.plan.plan_sha256,
        derived_from_plan_sha256,
        runtime_review_sha256: &review.review_sha256,
        runtime_plan_sha256: &plan.plan_sha256,
    })
}

fn validate_activation_approval_id(approval_id: &str) -> Result<(), ModuleMergeError> {
    if approval_id.trim().is_empty()
        || approval_id.len() > MAX_MODULE_ACTIVATION_APPROVAL_ID_BYTES
        || approval_id.chars().any(char::is_control)
    {
        Err(ModuleMergeError::InvalidActivationApprovalId)
    } else {
        Ok(())
    }
}

fn canonical_sha256<T: Serialize>(value: &T) -> Result<Sha256Digest, ModuleMergeError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| ModuleMergeError::CanonicalEncoding(error.to_string()))?;
    Sha256Digest::parse(hex::encode(Sha256::digest(bytes)))
        .map_err(ModuleMergeError::CanonicalEncoding)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleComponentChangeKind {
    Added,
    Modified,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleComponentChange {
    pub component: ModuleComponentRef,
    pub kind: ModuleComponentChangeKind,
    pub previous_sha256: Option<Sha256Digest>,
    pub next_sha256: Option<Sha256Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleCapabilityDiff {
    pub added: Vec<ContentCapability>,
    pub removed: Vec<ContentCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleRevisionDiff {
    pub diff_sha256: Sha256Digest,
    pub module_id: ContentModuleId,
    pub from_revision_id: ModuleRevisionId,
    pub to_revision_id: ModuleRevisionId,
    pub from_source_sha256: Sha256Digest,
    pub to_source_sha256: Sha256Digest,
    pub component_changes: Vec<ModuleComponentChange>,
    pub capability_changes: ModuleCapabilityDiff,
    pub metadata_changed_fields: Vec<String>,
}

impl ModuleRevisionDiff {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        let expected = module_revision_diff_sha256(
            &self.module_id,
            &self.from_revision_id,
            &self.to_revision_id,
            &self.from_source_sha256,
            &self.to_source_sha256,
            &self.component_changes,
            &self.capability_changes,
            &self.metadata_changed_fields,
        )?;
        if expected != self.diff_sha256 {
            return Err(ModuleMergeError::DiffHashMismatch);
        }
        Ok(())
    }
}

#[allow(clippy::too_many_lines)] // A single canonical pass keeps diff ordering and hash input aligned.
pub fn diff_module_revisions(
    from: &ModuleRevisionSnapshot,
    to: &ModuleRevisionSnapshot,
) -> Result<ModuleRevisionDiff, ModuleMergeError> {
    validate_revision_snapshot(from)?;
    validate_revision_snapshot(to)?;
    if from.module.id != to.module.id || from.revision.module_id != to.revision.module_id {
        return Err(ModuleMergeError::DifferentModules);
    }
    if from.revision.id == to.revision.id && from != to {
        return Err(ModuleMergeError::CorruptRevisionIdentity);
    }
    let from_hashes = canonical_component_hashes(from)?
        .into_iter()
        .map(|hash| (hash.component, hash.sha256))
        .collect::<BTreeMap<_, _>>();
    let to_hashes = canonical_component_hashes(to)?
        .into_iter()
        .map(|hash| (hash.component, hash.sha256))
        .collect::<BTreeMap<_, _>>();
    let component_ids = from_hashes
        .keys()
        .chain(to_hashes.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut component_changes = Vec::new();
    for component in component_ids {
        match (from_hashes.get(&component), to_hashes.get(&component)) {
            (None, Some(next)) => component_changes.push(ModuleComponentChange {
                component,
                kind: ModuleComponentChangeKind::Added,
                previous_sha256: None,
                next_sha256: Some(next.clone()),
            }),
            (Some(previous), None) => component_changes.push(ModuleComponentChange {
                component,
                kind: ModuleComponentChangeKind::Removed,
                previous_sha256: Some(previous.clone()),
                next_sha256: None,
            }),
            (Some(previous), Some(next)) if previous != next => {
                component_changes.push(ModuleComponentChange {
                    component,
                    kind: ModuleComponentChangeKind::Modified,
                    previous_sha256: Some(previous.clone()),
                    next_sha256: Some(next.clone()),
                });
            }
            _ => {}
        }
    }

    let from_capabilities = from
        .module
        .required_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let to_capabilities = to
        .module
        .required_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let capability_changes = ModuleCapabilityDiff {
        added: to_capabilities
            .difference(&from_capabilities)
            .copied()
            .collect(),
        removed: from_capabilities
            .difference(&to_capabilities)
            .copied()
            .collect(),
    };
    let mut metadata_changed_fields = Vec::new();
    if from.module.name != to.module.name {
        metadata_changed_fields.push("name".to_owned());
    }
    if from.module.version != to.module.version {
        metadata_changed_fields.push("version".to_owned());
    }
    if from.module.schema_version != to.module.schema_version {
        metadata_changed_fields.push("schema_version".to_owned());
    }
    if from.module.metadata != to.module.metadata {
        metadata_changed_fields.push("metadata".to_owned());
    }

    let diff_sha256 = module_revision_diff_sha256(
        &from.module.id,
        &from.revision.id,
        &to.revision.id,
        &from.revision.source_hash,
        &to.revision.source_hash,
        &component_changes,
        &capability_changes,
        &metadata_changed_fields,
    )?;
    Ok(ModuleRevisionDiff {
        diff_sha256,
        module_id: from.module.id.clone(),
        from_revision_id: from.revision.id.clone(),
        to_revision_id: to.revision.id.clone(),
        from_source_sha256: from.revision.source_hash.clone(),
        to_source_sha256: to.revision.source_hash.clone(),
        component_changes,
        capability_changes,
        metadata_changed_fields,
    })
}

#[derive(Serialize)]
struct ModuleRevisionDiffDigest<'a> {
    module_id: &'a ContentModuleId,
    from_revision_id: &'a ModuleRevisionId,
    to_revision_id: &'a ModuleRevisionId,
    from_source_sha256: &'a Sha256Digest,
    to_source_sha256: &'a Sha256Digest,
    component_changes: &'a [ModuleComponentChange],
    capability_changes: &'a ModuleCapabilityDiff,
    metadata_changed_fields: &'a [String],
}

#[allow(clippy::too_many_arguments)] // Fields mirror the immutable digest payload one-to-one.
fn module_revision_diff_sha256(
    module_id: &ContentModuleId,
    from_revision_id: &ModuleRevisionId,
    to_revision_id: &ModuleRevisionId,
    from_source_sha256: &Sha256Digest,
    to_source_sha256: &Sha256Digest,
    component_changes: &[ModuleComponentChange],
    capability_changes: &ModuleCapabilityDiff,
    metadata_changed_fields: &[String],
) -> Result<Sha256Digest, ModuleMergeError> {
    canonical_sha256(&ModuleRevisionDiffDigest {
        module_id,
        from_revision_id,
        to_revision_id,
        from_source_sha256,
        to_source_sha256,
        component_changes,
        capability_changes,
        metadata_changed_fields,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleRollbackPolicy {
    pub state_revision: u64,
    pub maximum_module_schema_version: u32,
    pub scope_target_exists: bool,
    pub available_asset_ids: Vec<AssetId>,
    pub supported_capabilities: Vec<ContentCapability>,
    pub quarantined_revision_ids: Vec<ModuleRevisionId>,
    pub unresolved_components: Vec<ModuleComponentRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModuleRollbackBlocker {
    BindingDisabled,
    BindingAwaitingApproval,
    StaleBinding,
    DifferentModule,
    TargetAlreadyActive,
    TargetNotAncestor,
    CorruptRevisionLineage,
    CorruptSnapshot,
    UnsupportedSchemaVersion { schema_version: u32 },
    ScopeTargetMissing,
    MissingAsset { asset_id: AssetId },
    UnsupportedCapability { capability: ContentCapability },
    QuarantinedTarget,
    UnresolvedConflict { component: ModuleComponentRef },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleRollbackReview {
    pub review_sha256: Sha256Digest,
    pub expected_state_revision: u64,
    pub binding_id: ModuleBindingId,
    pub current_revision_id: ModuleRevisionId,
    pub current_source_sha256: Sha256Digest,
    pub target_revision_id: ModuleRevisionId,
    pub target_source_sha256: Sha256Digest,
    pub diff: Option<ModuleRevisionDiff>,
    pub blockers: Vec<ModuleRollbackBlocker>,
    pub eligible: bool,
}

impl ModuleRollbackReview {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        let expected = module_rollback_review_sha256(
            self.expected_state_revision,
            &self.binding_id,
            &self.current_revision_id,
            &self.current_source_sha256,
            &self.target_revision_id,
            &self.target_source_sha256,
            self.diff.as_ref(),
            &self.blockers,
            self.eligible,
        )?;
        if expected != self.review_sha256 {
            return Err(ModuleMergeError::RollbackReviewHashMismatch);
        }
        Ok(())
    }
}

pub fn review_module_rollback(
    binding: &ModuleBinding,
    current: &ModuleRevisionSnapshot,
    target: &ModuleRevisionSnapshot,
    known_revisions: &[ContentModuleRevision],
    policy: &ModuleRollbackPolicy,
) -> Result<ModuleRollbackReview, ModuleMergeError> {
    let mut blockers = Vec::new();
    if !binding.enabled {
        blockers.push(ModuleRollbackBlocker::BindingDisabled);
    }
    if !binding.approved {
        blockers.push(ModuleRollbackBlocker::BindingAwaitingApproval);
    }
    if binding.revision_id != current.revision.id {
        blockers.push(ModuleRollbackBlocker::StaleBinding);
    }
    if binding.module_id != current.module.id
        || current.module.id != target.module.id
        || current.revision.module_id != target.revision.module_id
    {
        blockers.push(ModuleRollbackBlocker::DifferentModule);
    }
    if current.revision.id == target.revision.id {
        blockers.push(ModuleRollbackBlocker::TargetAlreadyActive);
    }
    if !policy.scope_target_exists {
        blockers.push(ModuleRollbackBlocker::ScopeTargetMissing);
    }
    if target.module.schema_version > policy.maximum_module_schema_version {
        blockers.push(ModuleRollbackBlocker::UnsupportedSchemaVersion {
            schema_version: target.module.schema_version,
        });
    }
    for capability in &target.module.required_capabilities {
        if !policy.supported_capabilities.contains(capability) {
            blockers.push(ModuleRollbackBlocker::UnsupportedCapability {
                capability: *capability,
            });
        }
    }
    for asset_id in &target.module.asset_ids {
        if !policy.available_asset_ids.contains(asset_id) {
            blockers.push(ModuleRollbackBlocker::MissingAsset {
                asset_id: asset_id.clone(),
            });
        }
    }
    if policy
        .quarantined_revision_ids
        .contains(&target.revision.id)
    {
        blockers.push(ModuleRollbackBlocker::QuarantinedTarget);
    }
    blockers.extend(
        policy
            .unresolved_components
            .iter()
            .cloned()
            .map(|component| ModuleRollbackBlocker::UnresolvedConflict { component }),
    );

    let current_valid = validate_revision_snapshot(current).is_ok();
    let target_valid = validate_revision_snapshot(target).is_ok();
    if !current_valid || !target_valid {
        blockers.push(ModuleRollbackBlocker::CorruptSnapshot);
    }
    match target_is_ancestor(current, target, known_revisions) {
        Ok(true) => {}
        Ok(false) => blockers.push(ModuleRollbackBlocker::TargetNotAncestor),
        Err(()) => blockers.push(ModuleRollbackBlocker::CorruptRevisionLineage),
    }
    let diff = if current_valid && target_valid && current.module.id == target.module.id {
        diff_module_revisions(current, target).ok()
    } else {
        None
    };
    if diff.is_none() {
        blockers.push(ModuleRollbackBlocker::CorruptSnapshot);
    }
    blockers.sort();
    blockers.dedup();
    let eligible = blockers.is_empty() && diff.is_some();
    let review_sha256 = module_rollback_review_sha256(
        policy.state_revision,
        &binding.id,
        &current.revision.id,
        &current.revision.source_hash,
        &target.revision.id,
        &target.revision.source_hash,
        diff.as_ref(),
        &blockers,
        eligible,
    )?;
    Ok(ModuleRollbackReview {
        review_sha256,
        expected_state_revision: policy.state_revision,
        binding_id: binding.id.clone(),
        current_revision_id: current.revision.id.clone(),
        current_source_sha256: current.revision.source_hash.clone(),
        target_revision_id: target.revision.id.clone(),
        target_source_sha256: target.revision.source_hash.clone(),
        diff,
        blockers,
        eligible,
    })
}

fn target_is_ancestor(
    current: &ModuleRevisionSnapshot,
    target: &ModuleRevisionSnapshot,
    known_revisions: &[ContentModuleRevision],
) -> Result<bool, ()> {
    let mut revisions = BTreeMap::new();
    for revision in known_revisions
        .iter()
        .chain([&current.revision, &target.revision])
    {
        if let Some(existing) = revisions.insert(revision.id.clone(), revision)
            && existing != revision
        {
            return Err(());
        }
    }
    let mut visited = BTreeSet::new();
    let mut cursor = current.revision.previous_revision_id.as_ref();
    while let Some(id) = cursor {
        if !visited.insert(id.clone()) {
            return Err(());
        }
        if id == &target.revision.id {
            return Ok(true);
        }
        let revision = revisions.get(id).ok_or(())?;
        if revision.module_id != current.revision.module_id {
            return Err(());
        }
        cursor = revision.previous_revision_id.as_ref();
    }
    Ok(false)
}

#[derive(Serialize)]
struct ModuleRollbackReviewDigest<'a> {
    expected_state_revision: u64,
    binding_id: &'a ModuleBindingId,
    current_revision_id: &'a ModuleRevisionId,
    current_source_sha256: &'a Sha256Digest,
    target_revision_id: &'a ModuleRevisionId,
    target_source_sha256: &'a Sha256Digest,
    diff: Option<&'a ModuleRevisionDiff>,
    blockers: &'a [ModuleRollbackBlocker],
    eligible: bool,
}

#[allow(clippy::too_many_arguments)]
fn module_rollback_review_sha256(
    expected_state_revision: u64,
    binding_id: &ModuleBindingId,
    current_revision_id: &ModuleRevisionId,
    current_source_sha256: &Sha256Digest,
    target_revision_id: &ModuleRevisionId,
    target_source_sha256: &Sha256Digest,
    diff: Option<&ModuleRevisionDiff>,
    blockers: &[ModuleRollbackBlocker],
    eligible: bool,
) -> Result<Sha256Digest, ModuleMergeError> {
    canonical_sha256(&ModuleRollbackReviewDigest {
        expected_state_revision,
        binding_id,
        current_revision_id,
        current_source_sha256,
        target_revision_id,
        target_source_sha256,
        diff,
        blockers,
        eligible,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleRollbackPlan {
    pub plan_sha256: Sha256Digest,
    pub review_sha256: Sha256Digest,
    pub expected_state_revision: u64,
    pub binding_id: ModuleBindingId,
    pub expected_current_revision_id: ModuleRevisionId,
    pub expected_current_source_sha256: Sha256Digest,
    pub target_revision_id: ModuleRevisionId,
    pub target_source_sha256: Sha256Digest,
    pub diff_sha256: Sha256Digest,
}

impl ModuleRollbackPlan {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        let expected = module_rollback_plan_sha256(
            &self.review_sha256,
            self.expected_state_revision,
            &self.binding_id,
            &self.expected_current_revision_id,
            &self.expected_current_source_sha256,
            &self.target_revision_id,
            &self.target_source_sha256,
            &self.diff_sha256,
        )?;
        if expected != self.plan_sha256 {
            return Err(ModuleMergeError::RollbackPlanHashMismatch);
        }
        Ok(())
    }
}

pub fn prepare_module_rollback(
    review: &ModuleRollbackReview,
    expected_review_sha256: &Sha256Digest,
) -> Result<ModuleRollbackPlan, ModuleMergeError> {
    review.verify()?;
    if expected_review_sha256 != &review.review_sha256 {
        return Err(ModuleMergeError::StaleRollbackReview);
    }
    if !review.eligible {
        return Err(ModuleMergeError::RollbackBlocked);
    }
    let diff = review
        .diff
        .as_ref()
        .ok_or(ModuleMergeError::RollbackBlocked)?;
    diff.verify()?;
    let plan_sha256 = module_rollback_plan_sha256(
        &review.review_sha256,
        review.expected_state_revision,
        &review.binding_id,
        &review.current_revision_id,
        &review.current_source_sha256,
        &review.target_revision_id,
        &review.target_source_sha256,
        &diff.diff_sha256,
    )?;
    Ok(ModuleRollbackPlan {
        plan_sha256,
        review_sha256: review.review_sha256.clone(),
        expected_state_revision: review.expected_state_revision,
        binding_id: review.binding_id.clone(),
        expected_current_revision_id: review.current_revision_id.clone(),
        expected_current_source_sha256: review.current_source_sha256.clone(),
        target_revision_id: review.target_revision_id.clone(),
        target_source_sha256: review.target_source_sha256.clone(),
        diff_sha256: diff.diff_sha256.clone(),
    })
}

/// One rollback approval that also carries the complete, freshly reviewed
/// target-revision runtime composition.
///
/// Storage applies this value atomically. Persisting only [`ModuleRollbackPlan`]
/// is insufficient because it does not authorize component-level runtime
/// overlays for the target revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedModuleRollbackPlan {
    pub approval_sha256: Sha256Digest,
    pub rollback: ModuleRollbackPlan,
    pub activation_review: ModuleActivationReview,
    pub activation: ApprovedModuleActivationPlan,
}

impl ApprovedModuleRollbackPlan {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        self.rollback.verify()?;
        self.activation_review.verify()?;
        self.activation.verify()?;
        validate_rollback_activation(
            &self.rollback,
            &self.activation_review,
            &self.activation.plan,
        )?;
        let expected = approved_module_rollback_sha256(
            &self.rollback,
            &self.activation_review,
            &self.activation,
        )?;
        if expected != self.approval_sha256 {
            return Err(ModuleMergeError::RollbackApprovalHashMismatch);
        }
        Ok(())
    }
}

/// Builds one hash-bound rollback and target-overlay approval.
///
/// The activation review must contain the same binding as a disabled,
/// unapproved, pinned target candidate. Conflict resolutions are therefore
/// reviewed against the complete target composition before commit.
pub fn approve_module_rollback_plan(
    rollback_review: &ModuleRollbackReview,
    expected_rollback_review_sha256: &Sha256Digest,
    activation_review: &ModuleActivationReview,
    resolution_set: &ModuleMergeResolutionSet,
    activation_approval: &ModuleActivationApproval,
) -> Result<ApprovedModuleRollbackPlan, ModuleMergeError> {
    let rollback = prepare_module_rollback(rollback_review, expected_rollback_review_sha256)?;
    activation_review.verify()?;
    let activation_plan = resolve_module_merge(activation_review, resolution_set)?;
    validate_rollback_activation(&rollback, activation_review, &activation_plan)?;
    let activation = approve_module_activation_plan(&activation_plan, activation_approval)?;
    let approval_sha256 =
        approved_module_rollback_sha256(&rollback, activation_review, &activation)?;
    Ok(ApprovedModuleRollbackPlan {
        approval_sha256,
        rollback,
        activation_review: activation_review.clone(),
        activation,
    })
}

fn validate_rollback_activation(
    rollback: &ModuleRollbackPlan,
    activation_review: &ModuleActivationReview,
    activation_plan: &ModuleActivationPlan,
) -> Result<(), ModuleMergeError> {
    let expected_binding_ids = [rollback.binding_id.clone()];
    if activation_review.state_revision != rollback.expected_state_revision
        || activation_review.activation_binding_ids != expected_binding_ids
        || activation_plan.expected_state_revision != rollback.expected_state_revision
        || activation_plan.activation_binding_ids != expected_binding_ids
        || activation_plan.review_sha256 != activation_review.review_sha256
    {
        return Err(ModuleMergeError::RollbackActivationMismatch);
    }
    let proposed = activation_review
        .ordered_bindings
        .iter()
        .find(|binding| binding.id == rollback.binding_id)
        .ok_or(ModuleMergeError::RollbackActivationMismatch)?;
    if proposed.resolution_mode != ModuleRevisionResolutionMode::Pinned
        || proposed.pinned_revision_id.as_ref() != Some(&rollback.target_revision_id)
        || proposed.revision_id != rollback.target_revision_id
        || proposed.enabled
        || proposed.approved
        || proposed.activation_approval_id.is_some()
        || proposed.activation_review_sha256.is_some()
        || proposed.activation_plan_sha256.is_some()
    {
        return Err(ModuleMergeError::RollbackActivationMismatch);
    }
    Ok(())
}

#[derive(Serialize)]
struct ApprovedModuleRollbackDigest<'a> {
    rollback: &'a ModuleRollbackPlan,
    activation_review: &'a ModuleActivationReview,
    activation: &'a ApprovedModuleActivationPlan,
}

fn approved_module_rollback_sha256(
    rollback: &ModuleRollbackPlan,
    activation_review: &ModuleActivationReview,
    activation: &ApprovedModuleActivationPlan,
) -> Result<Sha256Digest, ModuleMergeError> {
    canonical_sha256(&ApprovedModuleRollbackDigest {
        rollback,
        activation_review,
        activation,
    })
}

#[derive(Serialize)]
struct ModuleRollbackPlanDigest<'a> {
    review_sha256: &'a Sha256Digest,
    expected_state_revision: u64,
    binding_id: &'a ModuleBindingId,
    expected_current_revision_id: &'a ModuleRevisionId,
    expected_current_source_sha256: &'a Sha256Digest,
    target_revision_id: &'a ModuleRevisionId,
    target_source_sha256: &'a Sha256Digest,
    diff_sha256: &'a Sha256Digest,
}

#[allow(clippy::too_many_arguments)]
fn module_rollback_plan_sha256(
    review_sha256: &Sha256Digest,
    expected_state_revision: u64,
    binding_id: &ModuleBindingId,
    expected_current_revision_id: &ModuleRevisionId,
    expected_current_source_sha256: &Sha256Digest,
    target_revision_id: &ModuleRevisionId,
    target_source_sha256: &Sha256Digest,
    diff_sha256: &Sha256Digest,
) -> Result<Sha256Digest, ModuleMergeError> {
    canonical_sha256(&ModuleRollbackPlanDigest {
        review_sha256,
        expected_state_revision,
        binding_id,
        expected_current_revision_id,
        expected_current_source_sha256,
        target_revision_id,
        target_source_sha256,
        diff_sha256,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use lorepia_domain::{
        AssetId, ContentModuleId, ConversationId, KnowledgeBookId, ModuleBindingId,
        ModuleRevisionId, ModuleRevisionResolutionMode, PackageMetadata, Provenance, SourceKind,
        TransformSetId, VariableId, VariableRef, VariableScope, VariableValue,
    };

    use super::*;

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

    #[test]
    fn no_module_runtime_identity_is_canonical_and_stable() {
        assert_eq!(
            no_applied_module_runtime_plan_sha256().as_str(),
            "c269c6af108864ad31ef2bd22a22ad95c943ce7cf4e90ede2fdd8f83eeee96dd"
        );
    }

    fn module(id: &str, version: &str, components: &[(&str, &str)]) -> ContentModule {
        let mut knowledge_book_ids = Vec::new();
        let mut transform_set_ids = Vec::new();
        let mut asset_ids = Vec::new();
        for (kind, id) in components {
            match *kind {
                "knowledge" => knowledge_book_ids.push(KnowledgeBookId((*id).to_owned())),
                "transform" => transform_set_ids.push(TransformSetId((*id).to_owned())),
                "asset" => asset_ids.push(AssetId((*id).to_owned())),
                _ => panic!("unsupported synthetic component kind"),
            }
        }
        ContentModule {
            id: ContentModuleId(id.to_owned()),
            name: format!("Module {id}"),
            version: version.to_owned(),
            schema_version: 1,
            prompt_fragments: Vec::new(),
            knowledge_book_ids,
            control_specs: Vec::new(),
            transform_set_ids,
            interaction_rule_set_ids: Vec::new(),
            asset_ids,
            imported_components_enabled: false,
            required_capabilities: vec![ContentCapability::Knowledge],
            metadata: PackageMetadata {
                author: Some("Synthetic Author".to_owned()),
                license: "LicenseRef-Private".to_owned(),
                redistribution_allowed: false,
                homepage: None,
                description: String::new(),
                tags: Vec::new(),
                provenance: provenance(),
            },
        }
    }

    fn component_ref(kind: &str, id: &str) -> ModuleComponentRef {
        match kind {
            "knowledge" => ModuleComponentRef::KnowledgeBook {
                id: KnowledgeBookId(id.to_owned()),
            },
            "transform" => ModuleComponentRef::TransformSet {
                id: TransformSetId(id.to_owned()),
            },
            "asset" => ModuleComponentRef::Asset {
                id: AssetId(id.to_owned()),
            },
            _ => panic!("unsupported synthetic component kind"),
        }
    }

    fn snapshot(
        module_id: &str,
        revision_id: &str,
        previous_revision_id: Option<&str>,
        version: &str,
        components: &[(&str, &str, &str)],
    ) -> ModuleRevisionSnapshot {
        let module_components = components
            .iter()
            .map(|(kind, id, _)| (*kind, *id))
            .collect::<Vec<_>>();
        ModuleRevisionSnapshot {
            module: module(module_id, version, &module_components),
            revision: ContentModuleRevision {
                id: ModuleRevisionId(revision_id.to_owned()),
                module_id: ContentModuleId(module_id.to_owned()),
                version: version.to_owned(),
                source_hash: digest(if version == "1.0.0" { "a1" } else { "a2" }),
                previous_revision_id: previous_revision_id
                    .map(|id| ModuleRevisionId(id.to_owned())),
                component_hashes: components
                    .iter()
                    .map(|(kind, id, hash)| ComponentHash {
                        component: component_ref(kind, id),
                        sha256: digest(hash),
                    })
                    .collect(),
                created_at: Utc
                    .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
                    .single()
                    .expect("valid timestamp"),
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
    ) -> ModuleBinding {
        ModuleBinding {
            id: ModuleBindingId(id.to_owned()),
            module_id: ContentModuleId(module_id.to_owned()),
            scope,
            target_id: target_id.map(str::to_owned),
            conversation_id: (scope == ModuleScope::Branch)
                .then(|| ConversationId("room-1".to_owned())),
            priority: 0,
            resolution_mode: ModuleRevisionResolutionMode::Active,
            pinned_revision_id: None,
            enabled: true,
            approved: true,
            package_import_approval_id: None,
            activation_approval_id: Some(format!("approval-{id}")),
            activation_review_sha256: Some(digest("e1")),
            activation_plan_sha256: Some(digest("e2")),
            variable_overrides: VariableMap::default(),
            revision_id: ModuleRevisionId(revision_id.to_owned()),
            created_at: Utc
                .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
        }
    }

    fn context() -> ModuleResolutionContext {
        ModuleResolutionContext {
            local_user_id: LocalUserId::from("local-user-1"),
            persona_id: Some(PersonaId::from("persona-1")),
            character_id: Some("character-1".to_owned()),
            conversation_id: Some("room-1".to_owned()),
            branch_id: Some("branch-1".to_owned()),
            supported_capabilities: vec![
                ContentCapability::Knowledge,
                ContentCapability::Transforms,
            ],
        }
    }

    #[test]
    fn scope_order_and_review_hash_do_not_depend_on_input_order() {
        let revisions = vec![
            snapshot("app", "rev-app", None, "1.0.0", &[("knowledge", "a", "11")]),
            snapshot(
                "user",
                "rev-user",
                None,
                "1.0.0",
                &[("knowledge", "b", "22")],
            ),
            snapshot(
                "persona",
                "rev-persona",
                None,
                "1.0.0",
                &[("knowledge", "c", "33")],
            ),
            snapshot(
                "character",
                "rev-character",
                None,
                "1.0.0",
                &[("knowledge", "d", "44")],
            ),
            snapshot(
                "room",
                "rev-room",
                None,
                "1.0.0",
                &[("knowledge", "e", "55")],
            ),
            snapshot(
                "branch",
                "rev-branch",
                None,
                "1.0.0",
                &[("knowledge", "f", "66")],
            ),
        ];
        let bindings = vec![
            binding(
                "b6",
                "branch",
                "rev-branch",
                ModuleScope::Branch,
                Some("branch-1"),
            ),
            binding("b2", "user", "rev-user", ModuleScope::User, None),
            binding(
                "b4",
                "character",
                "rev-character",
                ModuleScope::Character,
                Some("character-1"),
            ),
            binding("b1", "app", "rev-app", ModuleScope::App, None),
            binding(
                "b5",
                "room",
                "rev-room",
                ModuleScope::Conversation,
                Some("room-1"),
            ),
            binding(
                "b3",
                "persona",
                "rev-persona",
                ModuleScope::Persona,
                Some("persona-1"),
            ),
        ];
        let first = review_module_merge(9, &context(), &bindings, &revisions).expect("review");
        let mut reversed_bindings = bindings;
        reversed_bindings.reverse();
        let mut reversed_revisions = revisions;
        reversed_revisions.reverse();
        let second = review_module_merge(9, &context(), &reversed_bindings, &reversed_revisions)
            .expect("review");

        assert_eq!(first, second);
        assert_eq!(
            first
                .ordered_bindings
                .iter()
                .map(|binding| binding.scope)
                .collect::<Vec<_>>(),
            vec![
                ModuleScope::App,
                ModuleScope::User,
                ModuleScope::Persona,
                ModuleScope::Character,
                ModuleScope::Conversation,
                ModuleScope::Branch,
            ]
        );
        first.verify().expect("review hash");
    }

    #[test]
    fn local_user_identity_is_bound_into_each_user_scope_review() {
        let revisions = vec![snapshot(
            "user",
            "rev-user",
            None,
            "1.0.0",
            &[("knowledge", "user", "11")],
        )];
        let bindings = vec![binding(
            "user-binding",
            "user",
            "rev-user",
            ModuleScope::User,
            None,
        )];
        let first_context = context();
        let mut second_context = first_context.clone();
        second_context.local_user_id = LocalUserId::from("local-user-2");

        let first =
            review_module_merge(1, &first_context, &bindings, &revisions).expect("first review");
        let second =
            review_module_merge(1, &second_context, &bindings, &revisions).expect("second review");

        assert_eq!(first.ordered_bindings, second.ordered_bindings);
        assert_ne!(first.review_sha256, second.review_sha256);
        first.verify().expect("first review hash");
        second.verify().expect("second review hash");
    }

    #[test]
    fn declarative_runtime_overlay_is_selected_source_and_plan_hash_bound() {
        let mut revision = snapshot(
            "runtime",
            "rev-runtime",
            None,
            "1.0.0",
            &[("transform", "rewrite", "11")],
        );
        revision.module.imported_components_enabled = true;
        let bindings = vec![binding(
            "runtime-binding",
            "runtime",
            "rev-runtime",
            ModuleScope::User,
            None,
        )];
        let review =
            review_module_merge(1, &context(), &bindings, &[revision]).expect("runtime review");
        let plan = resolve_module_merge(
            &review,
            &ModuleMergeResolutionSet {
                expected_review_sha256: review.review_sha256.clone(),
                resolutions: Vec::new(),
            },
        )
        .expect("runtime plan");

        assert!(plan.components[0].selected_source.runtime_enabled_intent);
        assert!(plan.components[0].runtime_enabled);
        plan.verify().expect("runtime plan hash");

        let mut tampered = plan;
        tampered.components[0].runtime_enabled = false;
        assert_eq!(tampered.verify(), Err(ModuleMergeError::PlanHashMismatch));
    }

    #[test]
    fn variable_overrides_follow_scope_and_are_hash_bound() {
        let variable = VariableRef {
            scope: VariableScope::Conversation,
            namespace: None,
            id: VariableId::from("tone"),
        };
        let mut app = binding("app", "app", "rev-app", ModuleScope::App, None);
        app.variable_overrides
            .insert(variable.clone(), VariableValue::Text("app".to_owned()));
        let mut branch_low = binding(
            "branch-low",
            "branch-low",
            "rev-branch-low",
            ModuleScope::Branch,
            Some("branch-1"),
        );
        branch_low.priority = 1;
        branch_low
            .variable_overrides
            .insert(variable.clone(), VariableValue::Text("low".to_owned()));
        let mut branch = binding(
            "branch",
            "branch",
            "rev-branch",
            ModuleScope::Branch,
            Some("branch-1"),
        );
        branch.priority = 10;
        branch
            .variable_overrides
            .insert(variable.clone(), VariableValue::Text("branch".to_owned()));
        let revisions = vec![
            snapshot("app", "rev-app", None, "1.0.0", &[("knowledge", "a", "11")]),
            snapshot(
                "branch-low",
                "rev-branch-low",
                None,
                "1.0.0",
                &[("knowledge", "c", "33")],
            ),
            snapshot(
                "branch",
                "rev-branch",
                None,
                "1.0.0",
                &[("knowledge", "b", "22")],
            ),
        ];
        let review = review_module_merge(
            3,
            &context(),
            &[branch.clone(), app.clone(), branch_low.clone()],
            &revisions,
        )
        .expect("review");
        assert_eq!(
            review.effective_variable_overrides.get(&variable),
            Some(&VariableValue::Text("branch".to_owned()))
        );
        let reversed = review_module_merge(
            3,
            &context(),
            &[app.clone(), branch_low.clone(), branch.clone()],
            &revisions,
        )
        .expect("same review");
        assert_eq!(review.review_sha256, reversed.review_sha256);

        branch
            .variable_overrides
            .insert(variable, VariableValue::Text("changed".to_owned()));
        let changed = review_module_merge(3, &context(), &[branch, app, branch_low], &revisions)
            .expect("changed review");
        assert_ne!(review.review_sha256, changed.review_sha256);

        let plan = resolve_module_merge(
            &review,
            &ModuleMergeResolutionSet {
                expected_review_sha256: review.review_sha256.clone(),
                resolutions: Vec::new(),
            },
        )
        .expect("resolved plan");
        assert_eq!(
            plan.effective_variable_overrides,
            review.effective_variable_overrides
        );
        plan.verify().expect("hash-bound variable overrides");
    }

    #[test]
    fn pending_binding_requires_exact_activation_review_plan_and_approval() {
        let revisions = vec![snapshot(
            "room",
            "rev-room",
            None,
            "1.0.0",
            &[("knowledge", "a", "11")],
        )];
        let mut proposed = binding(
            "pending",
            "room",
            "rev-room",
            ModuleScope::Conversation,
            Some("room-1"),
        );
        proposed.enabled = false;
        proposed.approved = false;
        proposed.activation_approval_id = None;
        proposed.activation_review_sha256 = None;
        proposed.activation_plan_sha256 = None;

        let normal = review_module_merge(0, &context(), &[proposed.clone()], &revisions)
            .expect("normal review");
        assert!(normal.ordered_bindings.is_empty());
        let review = review_module_activation(None, &context(), &[], &proposed, &revisions)
            .expect("activation review");
        assert_eq!(review.activation_binding_ids, vec![proposed.id.clone()]);
        assert_eq!(review.ordered_bindings, vec![proposed]);

        let plan = resolve_module_merge(
            &review,
            &ModuleMergeResolutionSet {
                expected_review_sha256: review.review_sha256.clone(),
                resolutions: Vec::new(),
            },
        )
        .expect("activation plan");
        let approved = approve_module_activation_plan(
            &plan,
            &ModuleActivationApproval {
                approval_id: "activation-1".to_owned(),
                expected_review_sha256: review.review_sha256,
                expected_plan_sha256: plan.plan_sha256.clone(),
            },
        )
        .expect("activation approval");
        approved.verify().expect("hash-bound approval");
    }

    #[test]
    fn runtime_plan_derivation_rebinds_only_an_identical_child_branch_context() {
        let revisions = vec![snapshot(
            "room",
            "rev-room",
            None,
            "1.0.0",
            &[("knowledge", "a", "11")],
        )];
        let mut proposed = binding(
            "pending",
            "room",
            "rev-room",
            ModuleScope::Conversation,
            Some("room-1"),
        );
        proposed.enabled = false;
        proposed.approved = false;
        proposed.activation_approval_id = None;
        proposed.activation_review_sha256 = None;
        proposed.activation_plan_sha256 = None;
        let activation_review =
            review_module_activation(None, &context(), &[], &proposed, &revisions)
                .expect("activation review");
        let activation_plan = resolve_module_merge(
            &activation_review,
            &ModuleMergeResolutionSet {
                expected_review_sha256: activation_review.review_sha256.clone(),
                resolutions: Vec::new(),
            },
        )
        .expect("activation plan");
        let approval = approve_module_activation_plan(
            &activation_plan,
            &ModuleActivationApproval {
                approval_id: "activation-runtime-1".to_owned(),
                expected_review_sha256: activation_review.review_sha256.clone(),
                expected_plan_sha256: activation_plan.plan_sha256.clone(),
            },
        )
        .expect("activation approval");

        let mut activated = proposed;
        activated.enabled = true;
        activated.approved = true;
        activated.activation_approval_id = Some(approval.approval_id.clone());
        activated.activation_review_sha256 = Some(activation_review.review_sha256);
        activated.activation_plan_sha256 = Some(activation_plan.plan_sha256);
        let source_review = review_module_merge(1, &context(), &[activated.clone()], &revisions)
            .expect("source runtime review");
        let source = materialize_approved_module_runtime_plan(&approval, &source_review)
            .expect("source runtime materialization");
        source.verify().expect("source materialization hash");
        assert!(source.derived_from_plan_sha256.is_none());

        let mut child_context = context();
        child_context.branch_id = Some("branch-2".to_owned());
        let child_review = review_module_merge(1, &child_context, &[activated.clone()], &revisions)
            .expect("child runtime review");
        let child =
            derive_applied_module_runtime_plan(&source, &child_review).expect("derived child");
        child.verify().expect("derived child hash");
        assert_eq!(
            child.derived_from_plan_sha256.as_ref(),
            Some(&source.applied_plan_sha256)
        );
        assert_ne!(child.applied_plan_sha256, source.applied_plan_sha256);
        assert_eq!(child.plan.components, source.plan.components);

        let mut tampered = child.clone();
        tampered.derived_from_plan_sha256 = Some(digest("f0"));
        assert_eq!(
            tampered.verify(),
            Err(ModuleMergeError::RuntimePlanHashMismatch)
        );

        let mut other_room = child_context;
        other_room.conversation_id = Some("room-2".to_owned());
        let other_room_review =
            review_module_merge(1, &other_room, &[activated], &revisions).expect("other room");
        assert_eq!(
            derive_applied_module_runtime_plan(&source, &other_room_review),
            Err(ModuleMergeError::RuntimeDerivationChanged)
        );
    }

    #[test]
    fn app_runtime_plan_materializes_across_rooms_when_composition_is_identical() {
        let revisions = vec![snapshot(
            "app",
            "rev-app",
            None,
            "1.0.0",
            &[("knowledge", "a", "11")],
        )];
        let mut proposed = binding("app-binding", "app", "rev-app", ModuleScope::App, None);
        proposed.enabled = false;
        proposed.approved = false;
        proposed.activation_approval_id = None;
        proposed.activation_review_sha256 = None;
        proposed.activation_plan_sha256 = None;
        let activation_review =
            review_module_activation(None, &context(), &[], &proposed, &revisions)
                .expect("activation review");
        let activation_plan = resolve_module_merge(
            &activation_review,
            &ModuleMergeResolutionSet {
                expected_review_sha256: activation_review.review_sha256.clone(),
                resolutions: Vec::new(),
            },
        )
        .expect("activation plan");
        let approval = approve_module_activation_plan(
            &activation_plan,
            &ModuleActivationApproval {
                approval_id: "activation-app-runtime".to_owned(),
                expected_review_sha256: activation_review.review_sha256.clone(),
                expected_plan_sha256: activation_plan.plan_sha256.clone(),
            },
        )
        .expect("activation approval");
        let mut activated = proposed;
        activated.enabled = true;
        activated.approved = true;
        activated.activation_approval_id = Some(approval.approval_id.clone());
        activated.activation_review_sha256 = Some(activation_review.review_sha256);
        activated.activation_plan_sha256 = Some(activation_plan.plan_sha256);
        let source_review = review_module_merge(1, &context(), &[activated.clone()], &revisions)
            .expect("source review");
        let source = materialize_approved_module_runtime_plan(&approval, &source_review)
            .expect("source materialization");

        let mut second_room = context();
        second_room.persona_id = Some(PersonaId::from("persona-2"));
        second_room.character_id = Some("character-2".to_owned());
        second_room.conversation_id = Some("room-2".to_owned());
        second_room.branch_id = Some("branch-2".to_owned());
        let second_review = review_module_merge(1, &second_room, &[activated], &revisions)
            .expect("second room review");
        let second = derive_applied_module_runtime_plan(&source, &second_review)
            .expect("identical app composition derives");
        second.verify().expect("second room materialization");
        assert_eq!(second.plan.components, source.plan.components);
        assert_eq!(
            second.derived_from_plan_sha256.as_ref(),
            Some(&source.applied_plan_sha256)
        );
        assert_ne!(second.applied_plan_sha256, source.applied_plan_sha256);
    }

    #[test]
    fn runtime_plan_derivation_rejects_a_target_branch_override() {
        let revisions = vec![
            snapshot(
                "room",
                "rev-room",
                None,
                "1.0.0",
                &[("knowledge", "a", "11")],
            ),
            snapshot(
                "branch",
                "rev-branch",
                None,
                "1.0.0",
                &[("knowledge", "b", "22")],
            ),
        ];
        let approved_binding = binding(
            "room-binding",
            "room",
            "rev-room",
            ModuleScope::Conversation,
            Some("room-1"),
        );
        let mut proposed = approved_binding.clone();
        proposed.enabled = false;
        proposed.approved = false;
        proposed.activation_approval_id = None;
        proposed.activation_review_sha256 = None;
        proposed.activation_plan_sha256 = None;
        let activation_review = review_module_activation(
            Some(1),
            &context(),
            &[approved_binding],
            &proposed,
            &revisions,
        )
        .expect("activation review");
        let activation_plan = resolve_module_merge(
            &activation_review,
            &ModuleMergeResolutionSet {
                expected_review_sha256: activation_review.review_sha256.clone(),
                resolutions: Vec::new(),
            },
        )
        .expect("activation plan");
        let approval = approve_module_activation_plan(
            &activation_plan,
            &ModuleActivationApproval {
                approval_id: "activation-runtime-override".to_owned(),
                expected_review_sha256: activation_review.review_sha256.clone(),
                expected_plan_sha256: activation_plan.plan_sha256.clone(),
            },
        )
        .expect("activation approval");
        let mut activated = proposed;
        activated.enabled = true;
        activated.approved = true;
        activated.activation_approval_id = Some(approval.approval_id.clone());
        activated.activation_review_sha256 = Some(activation_review.review_sha256);
        activated.activation_plan_sha256 = Some(activation_plan.plan_sha256);
        let source_review = review_module_merge(2, &context(), &[activated.clone()], &revisions)
            .expect("source review");
        let source = materialize_approved_module_runtime_plan(&approval, &source_review)
            .expect("source materialization");

        let mut child_context = context();
        child_context.branch_id = Some("branch-2".to_owned());
        let branch_override = binding(
            "branch-binding",
            "branch",
            "rev-branch",
            ModuleScope::Branch,
            Some("branch-2"),
        );
        let child_review =
            review_module_merge(2, &child_context, &[activated, branch_override], &revisions)
                .expect("child review");
        assert_eq!(
            derive_applied_module_runtime_plan(&source, &child_review),
            Err(ModuleMergeError::RuntimeTargetBranchOverride)
        );
    }

    #[test]
    fn different_targets_and_unapproved_imports_never_apply() {
        let revisions = vec![snapshot(
            "room",
            "rev-room",
            None,
            "1.0.0",
            &[("knowledge", "a", "11")],
        )];
        let different = binding(
            "different",
            "room",
            "rev-room",
            ModuleScope::Conversation,
            Some("other-room"),
        );
        let mut awaiting = binding(
            "awaiting",
            "room",
            "rev-room",
            ModuleScope::Conversation,
            Some("room-1"),
        );
        awaiting.approved = false;
        awaiting.activation_approval_id = None;
        awaiting.activation_review_sha256 = None;
        awaiting.activation_plan_sha256 = None;

        let review =
            review_module_merge(1, &context(), &[different, awaiting], &revisions).expect("review");

        assert!(review.ordered_bindings.is_empty());
        assert_eq!(
            review
                .ignored_bindings
                .iter()
                .map(|binding| binding.reason)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                IgnoredModuleBindingReason::AwaitingApproval,
                IgnoredModuleBindingReason::DifferentTarget,
            ])
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one authority-chain test covers missing, exact, stale, disabled, and invented evidence"
    )]
    fn imported_package_activation_requires_exact_completed_component_authority() {
        let mut imported = snapshot(
            "imported",
            "rev-imported",
            None,
            "1.0.0",
            &[("knowledge", "book", "11")],
        );
        imported.module.metadata.provenance.source_kind = SourceKind::ImportedPackage;
        imported.module.metadata.provenance.source_id = Some("package.synthetic".to_owned());
        imported.module.metadata.provenance.source_hash = Some(digest("b1").as_str().to_owned());
        let mut proposed = binding(
            "imported-binding",
            "imported",
            "rev-imported",
            ModuleScope::App,
            None,
        );
        proposed.enabled = false;
        proposed.approved = false;
        proposed.package_import_approval_id = Some("package-approval-1".to_owned());
        proposed.activation_approval_id = None;
        proposed.activation_review_sha256 = None;
        proposed.activation_plan_sha256 = None;

        assert_eq!(
            review_module_activation(
                None,
                &context(),
                &[],
                &proposed,
                std::slice::from_ref(&imported),
            ),
            Err(ModuleMergeError::MissingImportApproval {
                binding_id: proposed.id.as_str().to_owned()
            })
        );

        imported.import_approval = Some(ModuleImportApprovalEvidence {
            approval_id: "package-approval-1".to_owned(),
            approval_sha256: digest("c1"),
            import_id: "import-1".to_owned(),
            import_revision: 4,
            package_id: PackageId::from("package.synthetic"),
            package_source_sha256: digest("b1"),
            selection_sha256: digest("c2"),
            capability_review_sha256: digest("c3"),
            module_id: imported.module.id.clone(),
            module_revision_id: imported.revision.id.clone(),
            module_revision_source_sha256: imported.revision.source_hash.clone(),
            module_package_component_id: "module-component".to_owned(),
            module_package_component_sha256: digest("c4"),
            module_commit_result_sha256: digest("c5"),
            selected_package_component_ids: vec![
                "knowledge-component".to_owned(),
                "module-component".to_owned(),
            ],
            authorized_capabilities: vec![ContentCapability::Knowledge],
            component_authorities: vec![ModuleImportComponentAuthority {
                component: component_ref("knowledge", "book"),
                component_sha256: digest("11"),
                package_component_id: "knowledge-component".to_owned(),
                package_component_sha256: digest("c6"),
                committed_target_object_id: "book".to_owned(),
                committed_target_revision_id: "knowledge-revision-1".to_owned(),
                committed_result_sha256: digest("c7"),
                committed_content_sha256: None,
            }],
        });
        let review = review_module_activation(
            None,
            &context(),
            &[],
            &proposed,
            std::slice::from_ref(&imported),
        )
        .expect("exact completed authority");
        review.verify().expect("authority is review-hash-bound");
        assert_eq!(review.import_approvals.len(), 1);
        let plan = resolve_module_merge(
            &review,
            &ModuleMergeResolutionSet {
                expected_review_sha256: review.review_sha256.clone(),
                resolutions: Vec::new(),
            },
        )
        .expect("authority plan");
        assert_eq!(plan.import_approvals, review.import_approvals);
        plan.verify().expect("authority is plan-hash-bound");

        let mut wrong_source = imported.clone();
        wrong_source
            .import_approval
            .as_mut()
            .expect("authority")
            .package_source_sha256 = digest("ff");
        assert!(matches!(
            review_module_activation(None, &context(), &[], &proposed, &[wrong_source]),
            Err(ModuleMergeError::InvalidImportApproval { .. })
        ));

        let mut disabled_component = imported.clone();
        disabled_component
            .import_approval
            .as_mut()
            .expect("authority")
            .component_authorities
            .clear();
        assert!(matches!(
            review_module_activation(None, &context(), &[], &proposed, &[disabled_component]),
            Err(ModuleMergeError::InvalidImportApproval { .. })
        ));

        let mut arbitrary = proposed;
        arbitrary.package_import_approval_id = Some("invented-approval".to_owned());
        assert!(matches!(
            review_module_activation(None, &context(), &[], &arbitrary, &[imported]),
            Err(ModuleMergeError::InvalidImportApproval { .. })
        ));
    }

    #[test]
    fn invalid_scope_target_is_rejected() {
        let invalid = binding(
            "invalid",
            "app",
            "revision",
            ModuleScope::App,
            Some("must-not-exist"),
        );

        assert!(matches!(
            review_module_merge(0, &context(), &[invalid], &[]),
            Err(ModuleMergeError::InvalidBindingTarget { .. })
        ));
    }

    #[test]
    fn equal_hashes_coalesce_but_distinct_hashes_form_one_three_way_conflict() {
        let revisions = vec![
            snapshot(
                "one",
                "rev-1",
                None,
                "1.0.0",
                &[("knowledge", "shared", "11")],
            ),
            snapshot(
                "two",
                "rev-2",
                None,
                "1.0.0",
                &[("knowledge", "shared", "11")],
            ),
            snapshot(
                "three",
                "rev-3",
                None,
                "1.0.0",
                &[("knowledge", "shared", "22")],
            ),
            snapshot(
                "four",
                "rev-4",
                None,
                "1.0.0",
                &[("knowledge", "shared", "33")],
            ),
        ];
        let bindings = vec![
            binding("b1", "one", "rev-1", ModuleScope::App, None),
            binding("b2", "two", "rev-2", ModuleScope::User, None),
            binding(
                "b3",
                "three",
                "rev-3",
                ModuleScope::Character,
                Some("character-1"),
            ),
            binding("b4", "four", "rev-4", ModuleScope::Branch, Some("branch-1")),
        ];

        let review = review_module_merge(2, &context(), &bindings, &revisions).expect("review");

        assert_eq!(review.components.len(), 1);
        assert_eq!(review.components[0].candidates.len(), 3);
        assert_eq!(
            review.components[0]
                .candidates
                .iter()
                .find(|candidate| candidate.candidate.component_hash == digest("11"))
                .expect("coalesced hash")
                .sources
                .len(),
            2
        );
        assert_eq!(review.conflicts.len(), 1);
        assert_eq!(review.conflicts[0].candidates.len(), 3);
    }

    #[test]
    fn conflicts_require_hash_bound_explicit_resolution() {
        let revisions = vec![
            snapshot(
                "low",
                "rev-low",
                None,
                "1.0.0",
                &[("knowledge", "shared", "11")],
            ),
            snapshot(
                "high",
                "rev-high",
                None,
                "1.0.0",
                &[("knowledge", "shared", "22")],
            ),
        ];
        let bindings = vec![
            binding("low", "low", "rev-low", ModuleScope::App, None),
            binding(
                "high",
                "high",
                "rev-high",
                ModuleScope::Branch,
                Some("branch-1"),
            ),
        ];
        let review = review_module_merge(4, &context(), &bindings, &revisions).expect("review");
        assert!(matches!(
            resolve_module_merge(
                &review,
                &ModuleMergeResolutionSet {
                    expected_review_sha256: review.review_sha256.clone(),
                    resolutions: Vec::new(),
                }
            ),
            Err(ModuleMergeError::UnresolvedConflict(_))
        ));

        let conflict = &review.conflicts[0];
        let selected = conflict
            .candidates
            .iter()
            .find(|candidate| candidate.module_id == ContentModuleId("low".to_owned()))
            .expect("lower-scope candidate")
            .clone();
        let plan = resolve_module_merge(
            &review,
            &ModuleMergeResolutionSet {
                expected_review_sha256: review.review_sha256.clone(),
                resolutions: vec![ModuleConflictResolution {
                    component: conflict.component.clone(),
                    expected_candidates: conflict.candidates.clone(),
                    selected: Some(selected),
                }],
            },
        )
        .expect("resolved plan");

        assert_eq!(plan.components[0].sha256, digest("11"));
        plan.verify().expect("plan hash");
    }

    #[test]
    fn stale_candidate_set_is_rejected() {
        let revisions = vec![
            snapshot(
                "one",
                "rev-1",
                None,
                "1.0.0",
                &[("knowledge", "shared", "11")],
            ),
            snapshot(
                "two",
                "rev-2",
                None,
                "1.0.0",
                &[("knowledge", "shared", "22")],
            ),
        ];
        let bindings = vec![
            binding("b1", "one", "rev-1", ModuleScope::App, None),
            binding("b2", "two", "rev-2", ModuleScope::User, None),
        ];
        let review = review_module_merge(1, &context(), &bindings, &revisions).expect("review");
        let conflict = &review.conflicts[0];
        let mut stale = conflict.candidates.clone();
        stale[0].component_hash = digest("ff");

        assert!(matches!(
            resolve_module_merge(
                &review,
                &ModuleMergeResolutionSet {
                    expected_review_sha256: review.review_sha256.clone(),
                    resolutions: vec![ModuleConflictResolution {
                        component: conflict.component.clone(),
                        expected_candidates: stale,
                        selected: None,
                    }],
                }
            ),
            Err(ModuleMergeError::StaleConflictCandidates(_))
        ));
    }

    #[test]
    fn immutable_revision_source_hash_is_bound_into_merge_review() {
        let binding = binding("b1", "one", "rev-1", ModuleScope::App, None);
        let original = snapshot(
            "one",
            "rev-1",
            None,
            "1.0.0",
            &[("knowledge", "shared", "11")],
        );
        let mut tampered = original.clone();
        tampered.revision.source_hash = digest("ff");

        let original_review =
            review_module_merge(1, &context(), std::slice::from_ref(&binding), &[original])
                .expect("original review");
        let tampered_review =
            review_module_merge(1, &context(), &[binding], &[tampered]).expect("tampered review");

        assert_ne!(original_review.review_sha256, tampered_review.review_sha256);
    }

    #[test]
    fn revision_diff_reports_added_modified_removed_and_capabilities() {
        let mut from = snapshot(
            "module",
            "rev-1",
            None,
            "1.0.0",
            &[
                ("knowledge", "removed", "11"),
                ("knowledge", "changed", "22"),
            ],
        );
        let mut to = snapshot(
            "module",
            "rev-2",
            Some("rev-1"),
            "2.0.0",
            &[("knowledge", "changed", "33"), ("transform", "added", "44")],
        );
        from.module.required_capabilities = vec![ContentCapability::Knowledge];
        to.module.required_capabilities =
            vec![ContentCapability::Knowledge, ContentCapability::Transforms];

        let diff = diff_module_revisions(&from, &to).expect("diff");

        assert_eq!(diff.component_changes.len(), 3);
        assert_eq!(
            diff.component_changes
                .iter()
                .map(|change| change.kind)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                ModuleComponentChangeKind::Added,
                ModuleComponentChangeKind::Modified,
                ModuleComponentChangeKind::Removed,
            ])
        );
        assert_eq!(
            diff.capability_changes.added,
            vec![ContentCapability::Transforms]
        );
        assert!(diff.metadata_changed_fields.contains(&"version".to_owned()));
        diff.verify().expect("diff hash");
    }

    #[test]
    fn same_revision_id_with_different_content_is_corruption() {
        let from = snapshot("module", "same", None, "1.0.0", &[("knowledge", "a", "11")]);
        let to = snapshot("module", "same", None, "1.0.0", &[("knowledge", "a", "22")]);

        assert_eq!(
            diff_module_revisions(&from, &to),
            Err(ModuleMergeError::CorruptRevisionIdentity)
        );
    }

    #[test]
    fn rollback_plan_binds_state_current_target_and_diff() {
        let target = snapshot(
            "module",
            "rev-1",
            None,
            "1.0.0",
            &[("knowledge", "a", "11")],
        );
        let current = snapshot(
            "module",
            "rev-2",
            Some("rev-1"),
            "2.0.0",
            &[("knowledge", "a", "22")],
        );
        let binding = binding("binding", "module", "rev-2", ModuleScope::User, None);
        let policy = ModuleRollbackPolicy {
            state_revision: 77,
            maximum_module_schema_version: 1,
            scope_target_exists: true,
            available_asset_ids: Vec::new(),
            supported_capabilities: vec![ContentCapability::Knowledge],
            quarantined_revision_ids: Vec::new(),
            unresolved_components: Vec::new(),
        };

        let review =
            review_module_rollback(&binding, &current, &target, &[], &policy).expect("review");
        assert!(review.eligible);
        assert!(review.blockers.is_empty());
        review.verify().expect("review hash");
        let plan = prepare_module_rollback(&review, &review.review_sha256).expect("rollback plan");
        assert_eq!(plan.expected_state_revision, 77);
        assert_eq!(plan.expected_current_revision_id, current.revision.id);
        assert_eq!(plan.target_revision_id, target.revision.id);
        plan.verify().expect("plan hash");
        let mut tampered = plan;
        tampered.expected_state_revision += 1;
        assert_eq!(
            tampered.verify(),
            Err(ModuleMergeError::RollbackPlanHashMismatch)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Rollback and target overlay hashes are one atomic contract.
    fn rollback_approval_carries_the_exact_target_runtime_plan() {
        let target = snapshot(
            "module",
            "rev-1",
            None,
            "1.0.0",
            &[("knowledge", "a", "11")],
        );
        let current = snapshot(
            "module",
            "rev-2",
            Some("rev-1"),
            "2.0.0",
            &[("knowledge", "a", "22")],
        );
        let base = snapshot(
            "base",
            "rev-base",
            None,
            "1.0.0",
            &[("knowledge", "a", "33")],
        );
        let base_binding = binding("base-binding", "base", "rev-base", ModuleScope::App, None);
        let binding = binding("binding", "module", "rev-2", ModuleScope::User, None);
        let rollback_review = review_module_rollback(
            &binding,
            &current,
            &target,
            &[],
            &ModuleRollbackPolicy {
                state_revision: 77,
                maximum_module_schema_version: 1,
                scope_target_exists: true,
                available_asset_ids: Vec::new(),
                supported_capabilities: vec![ContentCapability::Knowledge],
                quarantined_revision_ids: Vec::new(),
                unresolved_components: Vec::new(),
            },
        )
        .expect("rollback review");
        let mut proposed = binding.clone();
        proposed.resolution_mode = ModuleRevisionResolutionMode::Pinned;
        proposed.pinned_revision_id = Some(target.revision.id.clone());
        proposed.revision_id = target.revision.id.clone();
        proposed.enabled = false;
        proposed.approved = false;
        proposed.activation_approval_id = None;
        proposed.activation_review_sha256 = None;
        proposed.activation_plan_sha256 = None;
        let activation_review = review_module_activation(
            Some(77),
            &context(),
            &[binding.clone(), base_binding],
            &proposed,
            &[current, target.clone(), base],
        )
        .expect("target activation review");
        let conflict = activation_review
            .conflicts
            .first()
            .expect("rollback target conflicts with app component");
        let selected_target = conflict
            .candidates
            .iter()
            .find(|candidate| candidate.module_id == target.module.id)
            .expect("target candidate")
            .clone();
        let resolutions = ModuleMergeResolutionSet {
            expected_review_sha256: activation_review.review_sha256.clone(),
            resolutions: vec![ModuleConflictResolution {
                component: conflict.component.clone(),
                expected_candidates: conflict.candidates.clone(),
                selected: Some(selected_target),
            }],
        };
        let activation_plan =
            resolve_module_merge(&activation_review, &resolutions).expect("target plan");
        let approved = approve_module_rollback_plan(
            &rollback_review,
            &rollback_review.review_sha256,
            &activation_review,
            &resolutions,
            &ModuleActivationApproval {
                approval_id: "rollback-activation-1".to_owned(),
                expected_review_sha256: activation_review.review_sha256.clone(),
                expected_plan_sha256: activation_plan.plan_sha256,
            },
        )
        .expect("approved rollback");

        approved.verify().expect("combined approval hash");
        assert_eq!(
            approved.activation.plan.activation_binding_ids,
            vec![binding.id]
        );
        assert_eq!(
            approved
                .activation
                .plan
                .components
                .first()
                .expect("selected target component")
                .sha256,
            target.revision.component_hashes[0].sha256
        );

        let omitted_resolutions = ModuleMergeResolutionSet {
            expected_review_sha256: activation_review.review_sha256.clone(),
            resolutions: vec![ModuleConflictResolution {
                component: conflict.component.clone(),
                expected_candidates: conflict.candidates.clone(),
                selected: None,
            }],
        };
        let omitted_plan = resolve_module_merge(&activation_review, &omitted_resolutions)
            .expect("explicit omission plan");
        let omitted = approve_module_rollback_plan(
            &rollback_review,
            &rollback_review.review_sha256,
            &activation_review,
            &omitted_resolutions,
            &ModuleActivationApproval {
                approval_id: "rollback-activation-omit".to_owned(),
                expected_review_sha256: activation_review.review_sha256.clone(),
                expected_plan_sha256: omitted_plan.plan_sha256,
            },
        )
        .expect("approved omitted rollback component");
        omitted.verify().expect("omitted approval hash");
        assert!(omitted.activation.plan.components.is_empty());
        assert_eq!(
            omitted.activation.plan.omitted_components,
            vec![conflict.component.clone()]
        );
    }

    #[test]
    fn rollback_eligibility_explains_every_material_blocker() {
        let target = snapshot(
            "module",
            "rev-1",
            None,
            "1.0.0",
            &[("asset", "missing", "11")],
        );
        let current = snapshot(
            "module",
            "rev-2",
            None,
            "2.0.0",
            &[("knowledge", "a", "22")],
        );
        let mut binding = binding("binding", "module", "rev-stale", ModuleScope::User, None);
        binding.approved = false;
        let conflict = component_ref("knowledge", "a");
        let policy = ModuleRollbackPolicy {
            state_revision: 3,
            maximum_module_schema_version: 0,
            scope_target_exists: false,
            available_asset_ids: Vec::new(),
            supported_capabilities: Vec::new(),
            quarantined_revision_ids: vec![target.revision.id.clone()],
            unresolved_components: vec![conflict.clone()],
        };

        let review =
            review_module_rollback(&binding, &current, &target, &[], &policy).expect("review");

        assert!(!review.eligible);
        assert!(
            review
                .blockers
                .contains(&ModuleRollbackBlocker::StaleBinding)
        );
        assert!(
            review
                .blockers
                .contains(&ModuleRollbackBlocker::BindingAwaitingApproval)
        );
        assert!(
            review
                .blockers
                .contains(&ModuleRollbackBlocker::TargetNotAncestor)
        );
        assert!(
            review
                .blockers
                .contains(&ModuleRollbackBlocker::ScopeTargetMissing)
        );
        assert!(
            review
                .blockers
                .contains(&ModuleRollbackBlocker::MissingAsset {
                    asset_id: AssetId("missing".to_owned())
                })
        );
        assert!(
            review
                .blockers
                .contains(&ModuleRollbackBlocker::QuarantinedTarget)
        );
        assert!(
            review
                .blockers
                .contains(&ModuleRollbackBlocker::UnresolvedConflict {
                    component: conflict
                })
        );
        assert_eq!(
            prepare_module_rollback(&review, &review.review_sha256),
            Err(ModuleMergeError::RollbackBlocked)
        );
    }
}

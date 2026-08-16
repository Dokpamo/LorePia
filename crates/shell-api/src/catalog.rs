use chrono::{DateTime, Utc};
use lorepia_core::{
    CatalogDiffDto, ProviderCatalogActivationKind, ProviderCatalogActivationSummary,
    ProviderCatalogHistory, ProviderCatalogImportPlan, ProviderCatalogImportResult,
    ProviderCatalogImportReview, ProviderCatalogRevisionSummary, ProviderCatalogRollbackPlan,
    ProviderCatalogRollbackResult, ProviderCatalogStatus,
};
use serde::{Deserialize, Serialize};

use crate::{ShellApi, ShellError, ShellResult, SignedCatalogEnvelope};

/// Typed, bounded catalog diff produced by the verified Rust catalog engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderCatalogDiffDto(CatalogDiffDto);

impl From<CatalogDiffDto> for ProviderCatalogDiffDto {
    fn from(value: CatalogDiffDto) -> Self {
        Self(value)
    }
}

impl From<ProviderCatalogDiffDto> for CatalogDiffDto {
    fn from(value: ProviderCatalogDiffDto) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogStatusDto {
    pub status_schema_version: u32,
    pub state_version: u64,
    pub active_revision: u64,
    pub active_snapshot_sha256: String,
    pub bundled_baseline_sha256: String,
    pub snapshot_count: u32,
    pub signed_update_count: u32,
    pub highest_accepted_revision: u64,
    pub latest_issued_at: Option<DateTime<Utc>>,
    pub active_signed_revisions: Vec<u64>,
}

impl From<ProviderCatalogStatus> for ProviderCatalogStatusDto {
    fn from(value: ProviderCatalogStatus) -> Self {
        Self {
            status_schema_version: value.status_schema_version,
            state_version: value.state_version,
            active_revision: value.active_revision,
            active_snapshot_sha256: value.active_snapshot_sha256,
            bundled_baseline_sha256: value.bundled_baseline_sha256,
            snapshot_count: value.snapshot_count,
            signed_update_count: value.signed_update_count,
            highest_accepted_revision: value.highest_accepted_revision,
            latest_issued_at: value.latest_issued_at,
            active_signed_revisions: value.active_signed_revisions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogRevisionSummaryDto {
    pub revision: u64,
    pub captured_at: DateTime<Utc>,
    pub snapshot_sha256: String,
    pub signed_revisions: Vec<u64>,
    pub active: bool,
}

impl From<ProviderCatalogRevisionSummary> for ProviderCatalogRevisionSummaryDto {
    fn from(value: ProviderCatalogRevisionSummary) -> Self {
        Self {
            revision: value.revision,
            captured_at: value.captured_at,
            snapshot_sha256: value.snapshot_sha256,
            signed_revisions: value.signed_revisions,
            active: value.active,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogActivationSummaryDto {
    pub action_id: String,
    pub state_version: u64,
    pub kind: String,
    pub from_revision: Option<u64>,
    pub to_revision: u64,
    pub activated_at: DateTime<Utc>,
    pub diff: ProviderCatalogDiffDto,
}

impl From<ProviderCatalogActivationSummary> for ProviderCatalogActivationSummaryDto {
    fn from(value: ProviderCatalogActivationSummary) -> Self {
        Self {
            action_id: value.action_id,
            state_version: value.state_version,
            kind: match value.kind {
                ProviderCatalogActivationKind::Import => "import",
                ProviderCatalogActivationKind::Rollback => "rollback",
            }
            .to_owned(),
            from_revision: value.from_revision,
            to_revision: value.to_revision,
            activated_at: value.activated_at,
            diff: value.diff.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogHistoryDto {
    pub history_schema_version: u32,
    pub active_revision: u64,
    pub revisions: Vec<ProviderCatalogRevisionSummaryDto>,
    pub activations: Vec<ProviderCatalogActivationSummaryDto>,
    pub next_before_revision: Option<u64>,
    pub next_before_state_version: Option<u64>,
}

impl From<ProviderCatalogHistory> for ProviderCatalogHistoryDto {
    fn from(value: ProviderCatalogHistory) -> Self {
        Self {
            history_schema_version: value.history_schema_version,
            active_revision: value.active_revision,
            revisions: value.revisions.into_iter().map(Into::into).collect(),
            activations: value.activations.into_iter().map(Into::into).collect(),
            next_before_revision: value.next_before_revision,
            next_before_state_version: value.next_before_state_version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogImportReviewDto {
    pub plan_schema_version: u32,
    pub action_id: String,
    pub expected_state_version: u64,
    pub expected_active_revision: u64,
    pub expected_active_snapshot_sha256: String,
    pub expected_highest_accepted_revision: u64,
    pub envelope_byte_count: u64,
    pub envelope_sha256: String,
    pub signing_key_id: String,
    pub payload_sha256: String,
    pub signed_catalog_revision: u64,
    pub candidate_revision: u64,
    pub candidate_snapshot_sha256: String,
    pub prepared_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub diff: ProviderCatalogDiffDto,
}

impl From<ProviderCatalogImportReview> for ProviderCatalogImportReviewDto {
    fn from(value: ProviderCatalogImportReview) -> Self {
        Self {
            plan_schema_version: value.plan_schema_version,
            action_id: value.action_id,
            expected_state_version: value.expected_state_version,
            expected_active_revision: value.expected_active_revision,
            expected_active_snapshot_sha256: value.expected_active_snapshot_sha256,
            expected_highest_accepted_revision: value.expected_highest_accepted_revision,
            envelope_byte_count: value.envelope_byte_count,
            envelope_sha256: value.envelope_sha256,
            signing_key_id: value.signing_key_id,
            payload_sha256: value.payload_sha256,
            signed_catalog_revision: value.signed_catalog_revision,
            candidate_revision: value.candidate_revision,
            candidate_snapshot_sha256: value.candidate_snapshot_sha256,
            prepared_at: value.prepared_at,
            expires_at: value.expires_at,
            diff: value.diff.into(),
        }
    }
}

impl From<ProviderCatalogImportReviewDto> for ProviderCatalogImportReview {
    fn from(value: ProviderCatalogImportReviewDto) -> Self {
        Self {
            plan_schema_version: value.plan_schema_version,
            action_id: value.action_id,
            expected_state_version: value.expected_state_version,
            expected_active_revision: value.expected_active_revision,
            expected_active_snapshot_sha256: value.expected_active_snapshot_sha256,
            expected_highest_accepted_revision: value.expected_highest_accepted_revision,
            envelope_byte_count: value.envelope_byte_count,
            envelope_sha256: value.envelope_sha256,
            signing_key_id: value.signing_key_id,
            payload_sha256: value.payload_sha256,
            signed_catalog_revision: value.signed_catalog_revision,
            candidate_revision: value.candidate_revision,
            candidate_snapshot_sha256: value.candidate_snapshot_sha256,
            prepared_at: value.prepared_at,
            expires_at: value.expires_at,
            diff: value.diff.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogImportPlanDto {
    pub review: ProviderCatalogImportReviewDto,
    pub plan_sha256: String,
}

impl From<ProviderCatalogImportPlan> for ProviderCatalogImportPlanDto {
    fn from(value: ProviderCatalogImportPlan) -> Self {
        Self {
            review: value.review.into(),
            plan_sha256: value.plan_sha256,
        }
    }
}

impl From<ProviderCatalogImportPlanDto> for ProviderCatalogImportPlan {
    fn from(value: ProviderCatalogImportPlanDto) -> Self {
        Self {
            review: value.review.into(),
            plan_sha256: value.plan_sha256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogImportResultDto {
    pub signed_catalog_revision: u64,
    pub activated_revision: u64,
    pub diff: ProviderCatalogDiffDto,
    pub status: ProviderCatalogStatusDto,
}

impl From<ProviderCatalogImportResult> for ProviderCatalogImportResultDto {
    fn from(value: ProviderCatalogImportResult) -> Self {
        Self {
            signed_catalog_revision: value.signed_catalog_revision,
            activated_revision: value.activated_revision,
            diff: value.diff.into(),
            status: value.status.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderCatalogRollbackPlanDto(ProviderCatalogRollbackPlan);

impl From<ProviderCatalogRollbackPlan> for ProviderCatalogRollbackPlanDto {
    fn from(value: ProviderCatalogRollbackPlan) -> Self {
        Self(value)
    }
}

impl From<ProviderCatalogRollbackPlanDto> for ProviderCatalogRollbackPlan {
    fn from(value: ProviderCatalogRollbackPlanDto) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogRollbackResultDto {
    pub from_revision: u64,
    pub activated_revision: u64,
    pub status: ProviderCatalogStatusDto,
}

impl From<ProviderCatalogRollbackResult> for ProviderCatalogRollbackResultDto {
    fn from(value: ProviderCatalogRollbackResult) -> Self {
        Self {
            from_revision: value.from_revision,
            activated_revision: value.activated_revision,
            status: value.status.into(),
        }
    }
}

impl ShellApi {
    pub fn prepare_signed_provider_catalog_import(
        &self,
        envelope: &SignedCatalogEnvelope,
    ) -> ShellResult<ProviderCatalogImportPlanDto> {
        self.core
            .prepare_signed_provider_catalog_import(envelope.as_bytes())
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn activate_signed_provider_catalog_import(
        &self,
        plan: ProviderCatalogImportPlanDto,
        envelope: &SignedCatalogEnvelope,
    ) -> ShellResult<ProviderCatalogImportResultDto> {
        self.core
            .activate_signed_provider_catalog_import(&plan.into(), envelope.as_bytes())
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn provider_catalog_status(&self) -> ShellResult<ProviderCatalogStatusDto> {
        self.core
            .provider_catalog_status()
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn provider_catalog_history(
        &self,
        limit: u32,
        before_revision: Option<u64>,
        before_state_version: Option<u64>,
    ) -> ShellResult<ProviderCatalogHistoryDto> {
        self.core
            .provider_catalog_history(limit, before_revision, before_state_version)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn diff_provider_catalog_revisions(
        &self,
        from_revision: u64,
        to_revision: u64,
    ) -> ShellResult<ProviderCatalogDiffDto> {
        self.core
            .diff_provider_catalog_revisions(from_revision, to_revision)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn prepare_provider_catalog_rollback(
        &self,
        target_revision: u64,
    ) -> ShellResult<ProviderCatalogRollbackPlanDto> {
        self.core
            .prepare_provider_catalog_rollback(target_revision)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn activate_provider_catalog_rollback(
        &self,
        plan: ProviderCatalogRollbackPlanDto,
    ) -> ShellResult<ProviderCatalogRollbackResultDto> {
        self.core
            .activate_provider_catalog_rollback(&plan.into())
            .map(Into::into)
            .map_err(ShellError::from)
    }
}

#[cfg(test)]
mod tests {
    use lorepia_core::CoreConfig;
    use tempfile::tempdir;

    use crate::ShellApi;

    #[test]
    fn catalog_status_and_history_are_core_backed() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("shell");

        let status = shell.provider_catalog_status().expect("status");
        let history = shell
            .provider_catalog_history(10, None, None)
            .expect("history");

        assert_eq!(status.status_schema_version, 1);
        assert_eq!(history.active_revision, status.active_revision);
        assert!(!status.active_snapshot_sha256.is_empty());
    }
}

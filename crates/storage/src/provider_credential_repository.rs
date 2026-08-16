//! Secret-free durable ownership journal for native provider credential slots.
//!
//! Native keychains and credential managers cannot participate in a `SQLite`
//! transaction. Every ordinary provider credential mutation is therefore
//! bound to an immutable `SQLite` plan before the platform side effect starts.
//! This module never accepts credential bytes or a credential-derived digest.

use chrono::{DateTime, Utc};
use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, CredentialScope, ProviderConnection, ProviderConnectionId,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::database::{
    Storage, archive_provider_connection_row, ensure_provider_connection_has_no_unfinished_work,
    storage_db_error, validate_provider_catalog_foreign_keys,
};

const PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION: u32 = 1;
const PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION: u32 = 1;
const MAX_PROVIDER_CREDENTIAL_PLAN_BYTES: usize = 16 * 1024;
const NATIVE_CREDENTIAL_RECOVERY_OWNER: &str = "native_platform";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialOperationKind {
    Install,
    RemoveCredential,
    RemoveForArchive,
}

impl ProviderCredentialOperationKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::RemoveCredential => "remove_credential",
            Self::RemoveForArchive => "remove_for_archive",
        }
    }

    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "install" => Ok(Self::Install),
            "remove_credential" => Ok(Self::RemoveCredential),
            "remove_for_archive" => Ok(Self::RemoveForArchive),
            _ => Err(stored_credential_journal_corrupted(
                "stored credential operation kind is invalid",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialObservedStatus {
    Missing,
    Available,
    Unreadable,
}

impl ProviderCredentialObservedStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Available => "available",
            Self::Unreadable => "unreadable",
        }
    }

    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "missing" => Ok(Self::Missing),
            "available" => Ok(Self::Available),
            "unreadable" => Ok(Self::Unreadable),
            _ => Err(stored_credential_journal_corrupted(
                "stored credential preflight status is invalid",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialOperationStatus {
    Prepared,
    Started,
    Succeeded,
    NoEffect,
    CleanupRequired,
    OutcomeUnknown,
}

impl ProviderCredentialOperationStatus {
    pub const fn is_unresolved(self) -> bool {
        matches!(
            self,
            Self::Prepared | Self::Started | Self::CleanupRequired | Self::OutcomeUnknown
        )
    }

    pub const fn is_terminal(self) -> bool {
        !self.is_unresolved()
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Started => "started",
            Self::Succeeded => "succeeded",
            Self::NoEffect => "no_effect",
            Self::CleanupRequired => "cleanup_required",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "started" => Ok(Self::Started),
            "succeeded" => Ok(Self::Succeeded),
            "no_effect" => Ok(Self::NoEffect),
            "cleanup_required" => Ok(Self::CleanupRequired),
            "outcome_unknown" => Ok(Self::OutcomeUnknown),
            _ => Err(stored_credential_journal_corrupted(
                "stored credential operation status is invalid",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialOutcomeCode {
    NativeEffectConfirmed,
    NativeEffectAbsent,
    NativeStatusUnreadable,
    NativeDurabilityUnknown,
    NativePredecessorDurabilityUnknown,
    ConnectionChanged,
    ArchiveCommitFailed,
}

impl ProviderCredentialOutcomeCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::NativeEffectConfirmed => "native_effect_confirmed",
            Self::NativeEffectAbsent => "native_effect_absent",
            Self::NativeStatusUnreadable => "native_status_unreadable",
            Self::NativeDurabilityUnknown => "native_durability_unknown",
            Self::NativePredecessorDurabilityUnknown => "native_predecessor_durability_unknown",
            Self::ConnectionChanged => "connection_changed",
            Self::ArchiveCommitFailed => "archive_commit_failed",
        }
    }

    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "native_effect_confirmed" => Ok(Self::NativeEffectConfirmed),
            "native_effect_absent" => Ok(Self::NativeEffectAbsent),
            "native_status_unreadable" => Ok(Self::NativeStatusUnreadable),
            "native_durability_unknown" => Ok(Self::NativeDurabilityUnknown),
            "native_predecessor_durability_unknown" => Ok(Self::NativePredecessorDurabilityUnknown),
            "connection_changed" => Ok(Self::ConnectionChanged),
            "archive_commit_failed" => Ok(Self::ArchiveCommitFailed),
            _ => Err(stored_credential_journal_corrupted(
                "stored credential operation outcome code is invalid",
            )),
        }
    }
}

/// Exact non-secret native-vault operation authorized by `SQLite`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCredentialOperationPlan {
    pub schema_version: u32,
    pub redaction_version: u32,
    pub operation_id: String,
    pub operation_sequence: u64,
    pub operation_kind: ProviderCredentialOperationKind,
    pub connection_id: ProviderConnectionId,
    pub credential_ref: String,
    pub connection_binding_sha256: String,
    /// Exact authority-derived physical native slot affected by this plan.
    /// Install uses its own operation id; removal freezes the currently owned
    /// authority. `None` is reserved for explicit cleanup of an unowned raw
    /// logical slot and never grants credential read access.
    pub credential_authority_id: Option<String>,
    pub credential_authority_binding_sha256: Option<String>,
    pub predecessor_authority_id: Option<String>,
    pub predecessor_authority_binding_sha256: Option<String>,
    pub credential_scope: CredentialScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProviderCredentialOperation {
    pub plan: ProviderCredentialOperationPlan,
    pub plan_sha256: String,
    pub preflight_evidence_sha256: String,
    pub preflight_attested_at: DateTime<Utc>,
    pub preflight_status: ProviderCredentialObservedStatus,
    pub status: ProviderCredentialOperationStatus,
    pub outcome_code: Option<ProviderCredentialOutcomeCode>,
    pub outcome_attestation_sequence: Option<u64>,
    pub cleanup_archives_connection: bool,
    /// Authoritative append-only durability obligation for the operation's
    /// own physical slot. This is independent from the summary outcome code.
    pub operation_slot_recovery_required: bool,
    /// Authoritative append-only durability obligation for replacement A.
    pub predecessor_slot_recovery_required: bool,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Exact durable authority a native host must find inside the same secure
/// item before releasing a provider credential to Core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentialAccessAuthority {
    pub authority_id: String,
    pub connection_binding_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderCredentialDurabilityTarget {
    OperationSlot,
    PredecessorSlot,
}

impl ProviderCredentialDurabilityTarget {
    const fn required_stage(self) -> &'static str {
        match self {
            Self::OperationSlot => "operation_durability_required",
            Self::PredecessorSlot => "predecessor_durability_required",
        }
    }

    const fn repaired_stage(self) -> &'static str {
        match self {
            Self::OperationSlot => "operation_durability_repaired",
            Self::PredecessorSlot => "predecessor_durability_repaired",
        }
    }

    const fn is_active(self, operation: &StoredProviderCredentialOperation) -> bool {
        match self {
            Self::OperationSlot => operation.operation_slot_recovery_required,
            Self::PredecessorSlot => operation.predecessor_slot_recovery_required,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCredentialSlotGarbageStatus {
    Pending,
    Started,
    Completed,
}

impl ProviderCredentialSlotGarbageStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Started => "started",
            Self::Completed => "completed",
        }
    }

    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "started" => Ok(Self::Started),
            "completed" => Ok(Self::Completed),
            _ => Err(stored_credential_journal_corrupted(
                "stored provider credential slot gc status is invalid",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentialSlotGarbage {
    pub connection_id: ProviderConnectionId,
    pub authority_sequence: u64,
    pub authority: ProviderCredentialAccessAuthority,
    pub status: ProviderCredentialSlotGarbageStatus,
}

#[derive(Debug, Serialize)]
struct ProviderCredentialConnectionBinding<'a> {
    schema_version: u32,
    connection_id: &'a str,
    template_id: &'a str,
    template_version: u32,
    api_origin: &'a str,
    config: serde_json::Value,
    credential_ref: &'a str,
    credential_scope: &'a CredentialScope,
}

#[derive(Debug, Serialize)]
struct ProviderCredentialPreflightEvidence<'a> {
    schema_version: u32,
    redaction_version: u32,
    operation_id: &'a str,
    plan_sha256: &'a str,
    connection_id: &'a str,
    credential_ref: &'a str,
    connection_binding_sha256: &'a str,
    slot_status: ProviderCredentialObservedStatus,
    native_owner: &'static str,
    attested_at: &'a str,
}

#[derive(Debug, Serialize)]
struct ProviderCredentialOutcomeEvidence<'a> {
    schema_version: u32,
    redaction_version: u32,
    operation_id: &'a str,
    plan_sha256: &'a str,
    connection_id: &'a str,
    credential_ref: &'a str,
    connection_binding_sha256: &'a str,
    sequence: u64,
    stage: &'a str,
    slot_status: ProviderCredentialObservedStatus,
    native_owner: &'static str,
    attested_at: &'a str,
}

impl Storage {
    /// Persists a removal operation before native credential mutation.
    ///
    /// Installation cannot use this generic entry point because its physical
    /// slot does not exist until Core proposes an authority. Callers must
    /// observe that authority-derived slot and use the dedicated authority-
    /// bound preparation path instead.
    pub fn prepare_provider_credential_operation(
        &self,
        connection_id: &ProviderConnectionId,
        kind: ProviderCredentialOperationKind,
        preflight_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        if kind == ProviderCredentialOperationKind::Install {
            return Err(CoreError::invalid(
                "provider credential installation requires a proposed physical-slot authority",
            ));
        }
        self.prepare_provider_credential_operation_with_install_authority(
            connection_id,
            kind,
            preflight_status,
            None,
        )
    }

    /// Returns a fresh backend-only physical-slot authority for an ordinary
    /// installation. No journal row is written and no native effect is
    /// authorized until the exact authority is passed back to prepare after a
    /// Missing observation of its derived slot.
    pub fn propose_provider_credential_install_authority(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<ProviderCredentialAccessAuthority> {
        let connection = self.connection()?;
        let binding = load_active_credential_binding(&connection, connection_id)?;
        ensure_no_unresolved_provider_credential_operation(&connection, &binding.credential_ref)?;
        let credential_scope = serde_json::from_str::<CredentialScope>(
            &binding.credential_scope_json,
        )
        .map_err(|_| {
            stored_credential_journal_corrupted(
                "provider credential scope cannot be decoded for installation",
            )
        })?;
        Ok(ProviderCredentialAccessAuthority {
            authority_id: Uuid::new_v4().to_string(),
            connection_binding_sha256: binding.sha256(&credential_scope)?,
        })
    }

    /// Persists an installation using the exact authority whose derived
    /// physical slot was observed before this transaction.
    pub fn prepare_provider_credential_operation_with_install_authority(
        &self,
        connection_id: &ProviderConnectionId,
        kind: ProviderCredentialOperationKind,
        preflight_status: ProviderCredentialObservedStatus,
        proposed_install_authority: Option<&ProviderCredentialAccessAuthority>,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        if kind == ProviderCredentialOperationKind::Install && proposed_install_authority.is_none()
        {
            return Err(CoreError::invalid(
                "provider credential installation requires a proposed physical-slot authority",
            ));
        }
        if kind == ProviderCredentialOperationKind::Install
            && preflight_status != ProviderCredentialObservedStatus::Missing
        {
            return Err(CoreError::invalid(
                "a provider credential can be installed only into a missing native slot",
            ));
        }

        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let binding = load_active_credential_binding(&transaction, connection_id)?;
        if matches!(
            kind,
            ProviderCredentialOperationKind::RemoveCredential
                | ProviderCredentialOperationKind::RemoveForArchive
        ) {
            ensure_provider_connection_has_no_unfinished_work(
                &transaction,
                connection_id.as_str(),
            )?;
        }
        let credential_scope = serde_json::from_str::<CredentialScope>(
            &binding.credential_scope_json,
        )
        .map_err(|_| {
            stored_credential_journal_corrupted(
                "provider credential scope cannot be decoded for journaling",
            )
        })?;
        if binding.credential_ref != connection_id.as_str() {
            return Err(stored_credential_journal_corrupted(
                "provider credential reference is detached from its connection",
            ));
        }
        ensure_no_unresolved_provider_credential_operation(&transaction, &binding.credential_ref)?;
        let connection_binding_sha256 = binding.sha256(&credential_scope)?;
        let authorities = resolve_provider_credential_operation_authorities(
            &transaction,
            connection_id,
            kind,
            &connection_binding_sha256,
            proposed_install_authority,
        )?;
        let now = Utc::now();
        let operation_id = insert_prepared_provider_credential_operation(
            &transaction,
            &PreparedProviderCredentialOperationInput {
                connection_id,
                kind,
                preflight_status,
                binding: &binding,
                credential_scope,
                authorities: &authorities,
                now,
            },
        )?;
        if preflight_status == ProviderCredentialObservedStatus::Unreadable {
            let current = load_provider_credential_operation(&transaction, &operation_id)?;
            let attestation_sequence = insert_operation_attestation(
                &transaction,
                &current,
                "postflight",
                ProviderCredentialObservedStatus::Unreadable,
                now,
            )?;
            update_operation_status(
                &transaction,
                &current,
                ProviderCredentialOperationStatus::OutcomeUnknown,
                ProviderCredentialOutcomeCode::NativeStatusUnreadable,
                attestation_sequence,
                now,
            )?;
        }
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_provider_credential_operation(&operation_id)
    }

    /// Durably records the last cutpoint before the native vault mutation.
    pub fn start_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let current = load_provider_credential_operation(&transaction, operation_id)?;
        validate_exact_operation(&current, plan_sha256)?;
        validate_current_connection_binding(&transaction, &current)?;
        if current.status != ProviderCredentialOperationStatus::Prepared {
            return Err(CoreError::invalid(
                "provider credential operation is not prepared",
            ));
        }
        if matches!(
            current.plan.operation_kind,
            ProviderCredentialOperationKind::RemoveCredential
                | ProviderCredentialOperationKind::RemoveForArchive
        ) && current.preflight_status == ProviderCredentialObservedStatus::Missing
        {
            return Err(CoreError::invalid(
                "a missing provider credential removal has no native effect to start",
            ));
        }
        let now = Utc::now();
        let changed = transaction
            .execute(
                "UPDATE provider_credential_operations
                 SET status = 'started', started_at = ?3, updated_at = ?3
                 WHERE id = ?1 AND plan_sha256 = ?2 AND status = 'prepared'",
                params![operation_id, plan_sha256, now.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::invalid(
                "provider credential operation changed before native mutation",
            ));
        }
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_provider_credential_operation(operation_id)
    }

    pub fn attest_provider_credential_predecessor_delete_intent(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.attest_provider_credential_predecessor(
            operation_id,
            plan_sha256,
            "predecessor_delete_intent",
            observed_status,
        )
    }

    pub fn attest_provider_credential_predecessor_missing(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.attest_provider_credential_predecessor(
            operation_id,
            plan_sha256,
            "predecessor_missing",
            ProviderCredentialObservedStatus::Missing,
        )
    }

    fn attest_provider_credential_predecessor(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        stage: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let current = load_provider_credential_operation(&transaction, operation_id)?;
        validate_exact_operation(&current, plan_sha256)?;
        if current.plan.operation_kind != ProviderCredentialOperationKind::Install
            || current.plan.predecessor_authority_id.is_none()
            || !matches!(
                current.status,
                ProviderCredentialOperationStatus::Started
                    | ProviderCredentialOperationStatus::CleanupRequired
                    | ProviderCredentialOperationStatus::OutcomeUnknown
            )
        {
            return Err(CoreError::invalid(
                "provider credential predecessor attestation is not applicable",
            ));
        }
        let existing = transaction
            .query_row(
                "SELECT slot_status
                 FROM provider_credential_operation_attestations
                 WHERE operation_id = ?1 AND stage = ?2
                 ORDER BY sequence DESC LIMIT 1",
                params![operation_id, stage],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?;
        if let Some(existing) = existing {
            if stage == "predecessor_delete_intent"
                || ProviderCredentialObservedStatus::parse(&existing)? == observed_status
            {
                return Ok(current);
            }
            return Err(CoreError::invalid(
                "provider credential predecessor observation changed",
            ));
        }
        let now = Utc::now();
        insert_operation_attestation(&transaction, &current, stage, observed_status, now)?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_provider_credential_operation(operation_id)
    }

    pub fn get_provider_credential_operation(
        &self,
        operation_id: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        let connection = self.connection()?;
        load_provider_credential_operation(&connection, operation_id)
    }

    pub fn list_unresolved_provider_credential_operations(
        &self,
    ) -> CoreResult<Vec<StoredProviderCredentialOperation>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, connection_id, credential_ref, operation_sequence,
                        operation_kind, connection_binding_sha256, plan_json,
                        plan_sha256, preflight_status, preflight_evidence_sha256,
                        preflight_attested_at, status, outcome_code,
                        outcome_attestation_sequence, schema_version, redaction_version,
                        created_at, started_at, finished_at, updated_at
                 FROM provider_credential_operations
                 WHERE status IN (
                    'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                 )
                 ORDER BY created_at, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], provider_credential_operation_row)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        drop(statement);
        rows.into_iter()
            .map(|row| {
                let mut operation = decode_operation_row(row)?;
                operation.cleanup_archives_connection =
                    validate_provider_credential_cleanup_intents(&connection, &operation)?.1;
                apply_provider_credential_durability_ledger(&connection, &mut operation)?;
                validate_operation_evidence(&connection, &operation)?;
                Ok(operation)
            })
            .collect()
    }

    /// Returns exact superseded authority-derived slots which still require
    /// bounded native deletion. The current ownership event and the logical
    /// raw legacy slot are never returned.
    pub fn list_provider_credential_slot_garbage(
        &self,
    ) -> CoreResult<Vec<ProviderCredentialSlotGarbage>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT gc.connection_id, gc.authority_sequence, gc.authority_id,
                        gc.connection_binding_sha256, gc.status
                 FROM provider_credential_slot_gc AS gc
                 WHERE gc.status <> 'completed'
                 ORDER BY gc.created_at, gc.connection_id, gc.authority_sequence",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        drop(statement);
        rows.into_iter()
            .map(|row| decode_provider_credential_slot_garbage(&connection, row))
            .collect()
    }

    /// Appends one typed observation to a superseded-slot GC record. Pending
    /// Missing completes without a native effect; Available/Unreadable marks
    /// the durable delete cutpoint. A later Missing observation completes it.
    pub fn observe_provider_credential_slot_garbage(
        &self,
        connection_id: &ProviderConnectionId,
        authority_sequence: u64,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<ProviderCredentialSlotGarbage> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current =
            load_provider_credential_slot_garbage(&transaction, connection_id, authority_sequence)?;
        if current.status == ProviderCredentialSlotGarbageStatus::Completed {
            return Ok(current);
        }
        let now = Utc::now().to_rfc3339();
        let (next_status, preflight_status, delete_started_at, completed_at) =
            match (current.status, observed_status) {
                (
                    ProviderCredentialSlotGarbageStatus::Pending,
                    ProviderCredentialObservedStatus::Missing,
                ) => (
                    ProviderCredentialSlotGarbageStatus::Completed,
                    Some(ProviderCredentialObservedStatus::Missing.as_str()),
                    None,
                    Some(now.as_str()),
                ),
                (
                    ProviderCredentialSlotGarbageStatus::Started,
                    ProviderCredentialObservedStatus::Missing,
                ) => (
                    ProviderCredentialSlotGarbageStatus::Completed,
                    None,
                    None,
                    Some(now.as_str()),
                ),
                (ProviderCredentialSlotGarbageStatus::Pending, observed) => (
                    ProviderCredentialSlotGarbageStatus::Started,
                    Some(observed.as_str()),
                    Some(now.as_str()),
                    None,
                ),
                (ProviderCredentialSlotGarbageStatus::Started, _) => (
                    ProviderCredentialSlotGarbageStatus::Started,
                    None,
                    None,
                    None,
                ),
                (ProviderCredentialSlotGarbageStatus::Completed, _) => unreachable!(),
            };
        let changed = transaction
            .execute(
                "UPDATE provider_credential_slot_gc
                 SET status = ?3,
                     preflight_status = COALESCE(preflight_status, ?4),
                     last_observed_status = ?5,
                     delete_started_at = COALESCE(delete_started_at, ?6),
                     completed_at = ?7,
                     updated_at = ?8
                 WHERE connection_id = ?1 AND authority_sequence = ?2
                   AND status = ?9",
                params![
                    connection_id.as_str(),
                    authority_sequence,
                    next_status.as_str(),
                    preflight_status,
                    observed_status.as_str(),
                    delete_started_at,
                    completed_at,
                    now,
                    current.status.as_str(),
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::invalid(
                "provider credential slot gc changed concurrently",
            ));
        }
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        let connection = self.connection()?;
        load_provider_credential_slot_garbage(&connection, connection_id, authority_sequence)
    }

    /// Fails closed unless the active binding has exact durable ownership.
    ///
    /// Schema-37 migration leaves pre-journal bindings in `legacy_pending`;
    /// native slot availability alone never promotes or grants access to them.
    pub fn ensure_provider_credential_access_settled(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<ProviderCredentialAccessAuthority> {
        self.provider_credential_authority(connection_id, true)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::InvalidInput,
                    "provider credential slot has no durable ownership authority",
                    true,
                )
            })
    }

    /// Permits the legacy raw vault format only for an exact schema-36
    /// dual-written profile binding which has never entered the durable
    /// credential journal.
    pub fn ensure_legacy_profile_raw_credential_access(
        &self,
        provider_profile_id: &str,
    ) -> CoreResult<()> {
        let connection = self.connection()?;
        let ownership = connection
            .query_row(
                "SELECT connection.credential_ref,
                        connection.credential_scope_json,
                        ownership.ownership_state,
                        ownership.connection_binding_sha256,
                        ownership.authority_id
                 FROM provider_profiles AS profile
                 JOIN provider_connections AS connection
                   ON connection.id = profile.id
                  AND connection.archived_at IS NULL
                 LEFT JOIN provider_credential_ownership AS ownership
                   ON ownership.connection_id = connection.id
                  AND ownership.credential_ref = connection.credential_ref
                 WHERE profile.id = ?1",
                [provider_profile_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "legacy provider profile was not found",
                    false,
                )
            })?;
        let (credential_ref, credential_scope, state, binding_sha256, authority_id) = ownership;
        if credential_ref.as_deref() != Some(provider_profile_id)
            || credential_scope.is_none()
            || state.is_none()
        {
            return Err(stored_credential_journal_corrupted(
                "legacy provider profile credential ownership is detached",
            ));
        }
        let unresolved = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM provider_credential_operations
                    WHERE credential_ref = ?1
                      AND status IN (
                        'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                      )
                 )",
                [provider_profile_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if unresolved {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "legacy provider credential slot has an unresolved durable operation",
                true,
            ));
        }
        if state.as_deref() != Some("legacy_pending")
            || binding_sha256.is_some()
            || authority_id.as_deref() != Some("schema-36-cutover")
        {
            return Err(CoreError::invalid(
                "legacy provider credential slot is not eligible for raw access",
            ));
        }
        Ok(())
    }

    /// Reports whether an ordinary connection target aliases an active,
    /// migration-authorized legacy raw credential slot.
    ///
    /// Tauri uses this before exposing ordinary credential mutations for a
    /// dual-written legacy profile. The legacy surface remains the sole owner
    /// of that raw slot until an explicit re-entry flow replaces its authority.
    pub fn provider_connection_uses_legacy_raw_credential(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<bool> {
        let connection = self.connection()?;
        let projection = connection
            .query_row(
                "SELECT binding.credential_ref,
                        binding.credential_scope_json,
                        ownership.ownership_state,
                        ownership.connection_binding_sha256,
                        ownership.authority_id,
                        EXISTS(
                            SELECT 1 FROM provider_profiles AS profile
                            WHERE profile.id = binding.id
                        )
                 FROM provider_connections AS binding
                 LEFT JOIN provider_credential_ownership AS ownership
                   ON ownership.connection_id = binding.id
                  AND ownership.credential_ref = binding.credential_ref
                 WHERE binding.id = ?1 AND binding.archived_at IS NULL",
                [connection_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, bool>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "provider connection was not found",
                    false,
                )
            })?;
        let (credential_ref, credential_scope, state, binding_sha256, authority_id, has_profile) =
            projection;
        if !has_profile || state.as_deref() != Some("legacy_pending") {
            return Ok(false);
        }
        if credential_ref.as_deref() != Some(connection_id.as_str())
            || credential_scope.is_none()
            || binding_sha256.is_some()
            || authority_id.as_deref() != Some("schema-36-cutover")
        {
            return Err(stored_credential_journal_corrupted(
                "legacy provider profile credential ownership is detached",
            ));
        }
        Ok(true)
    }

    /// Rejects a legacy raw credential mutation while provider work for its
    /// dual-written connection is unfinished.
    ///
    /// Tauri calls this while holding the process-local legacy admission
    /// reservation. The `Immediate` transaction is the reverse half of the
    /// generation-attempt admission boundary: either the mutation observes no
    /// admitted work, or the attempt wins first and the mutation is rejected.
    pub fn ensure_legacy_profile_credential_mutation_settled(
        &self,
        provider_profile_id: &str,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let projection = transaction
            .query_row(
                "SELECT connection.credential_ref,
                        connection.credential_scope_json,
                        ownership.ownership_state,
                        ownership.connection_binding_sha256,
                        ownership.authority_id,
                        EXISTS(
                          SELECT 1 FROM provider_credential_operations AS operation
                          WHERE operation.connection_id = connection.id
                            AND operation.status IN (
                              'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                            )
                        )
                 FROM provider_profiles AS profile
                 JOIN provider_connections AS connection
                   ON connection.id = profile.id
                  AND connection.archived_at IS NULL
                 LEFT JOIN provider_credential_ownership AS ownership
                   ON ownership.connection_id = connection.id
                  AND ownership.credential_ref = connection.credential_ref
                 WHERE profile.id = ?1",
                [provider_profile_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, bool>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "legacy provider profile was not found",
                    false,
                )
            })?;
        let (credential_ref, scope, state, binding, authority_id, unresolved) = projection;
        if credential_ref.as_deref() != Some(provider_profile_id)
            || scope.is_none()
            || state.as_deref() != Some("legacy_pending")
            || binding.is_some()
            || authority_id.as_deref() != Some("schema-36-cutover")
            || unresolved
        {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "legacy provider credential is not settled for mutation",
                true,
            ));
        }
        ensure_provider_connection_has_no_unfinished_work(&transaction, provider_profile_id)?;
        transaction.commit().map_err(storage_db_error)
    }

    /// Returns the pre-existing ownership authority while a durable removal
    /// is unresolved so native recovery can compare, but never release, the
    /// secure item.
    pub fn provider_credential_recovery_authority(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<Option<ProviderCredentialAccessAuthority>> {
        self.provider_credential_authority(connection_id, false)
    }

    fn provider_credential_authority(
        &self,
        connection_id: &ProviderConnectionId,
        require_settled: bool,
    ) -> CoreResult<Option<ProviderCredentialAccessAuthority>> {
        let connection = self.connection()?;
        let unresolved = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM provider_credential_operations
                    WHERE connection_id = ?1
                      AND status IN (
                        'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                      )
                 )",
                [connection_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if require_settled && unresolved {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "provider credential state is unresolved; reopen the app to reconcile it",
                true,
            ));
        }
        let ownership = load_provider_credential_ownership(&connection, connection_id)?;
        if !matches!(
            ownership.state.as_str(),
            "ordinary_owned" | "discovery_owned"
        ) {
            return Ok(None);
        }
        if ownership.authority_sequence == 0 {
            return Err(stored_credential_journal_corrupted(
                "owned provider credential has no authority event sequence",
            ));
        }
        let authority_id = ownership.authority_id.ok_or_else(|| {
            stored_credential_journal_corrupted(
                "provider credential ownership authority is missing",
            )
        })?;
        let authority_is_valid = provider_credential_ownership_authority_is_valid(
            &connection,
            connection_id,
            &ownership.state,
            ownership.binding_sha256.as_deref(),
            &authority_id,
        )?;
        if !authority_is_valid {
            return Err(stored_credential_journal_corrupted(
                "provider credential ownership authority is not backed by durable history",
            ));
        }
        let binding = load_active_credential_binding(&connection, connection_id)?;
        let scope = serde_json::from_str::<CredentialScope>(&binding.credential_scope_json)
            .map_err(|_| {
                stored_credential_journal_corrupted(
                    "provider credential scope cannot be decoded for access authorization",
                )
            })?;
        let connection_binding_sha256 = binding.sha256(&scope)?;
        if ownership.binding_sha256.as_deref() != Some(connection_binding_sha256.as_str()) {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "provider credential ownership does not match the current connection binding",
                true,
            ));
        }
        Ok(Some(ProviderCredentialAccessAuthority {
            authority_id,
            connection_binding_sha256,
        }))
    }

    /// Records a typed, content-free native observation after a cutpoint.
    /// Archive completion uses a separate atomic storage method because the
    /// connection row and terminal journal status must commit together.
    pub fn finish_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.finish_provider_credential_operation_at_stage(
            operation_id,
            plan_sha256,
            observed_status,
            "postflight",
        )
    }

    /// Reconciles an interrupted ordinary operation without repeating the
    /// native store or delete effect.
    pub fn reconcile_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.finish_provider_credential_operation_at_stage(
            operation_id,
            plan_sha256,
            observed_status,
            "recovery",
        )
    }

    fn finish_provider_credential_operation_at_stage(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
        stage: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let current = load_provider_credential_operation(&transaction, operation_id)?;
        validate_exact_operation(&current, plan_sha256)?;
        if current.status.is_terminal() {
            if provider_credential_terminal_requires_archive(&current) {
                ensure_provider_connection_is_archived(&transaction, &current.plan.connection_id)?;
            }
            return Ok(current);
        }
        if provider_credential_has_explicit_durability_barrier(&current) {
            return Ok(current);
        }
        if current.status == ProviderCredentialOperationStatus::CleanupRequired
            && observed_status != ProviderCredentialObservedStatus::Missing
        {
            return Ok(current);
        }
        if observed_status == ProviderCredentialObservedStatus::Missing
            && (current.plan.operation_kind == ProviderCredentialOperationKind::RemoveForArchive
                || current.cleanup_archives_connection)
            && provider_credential_archive_native_no_effect(&current).is_ok()
        {
            return Err(CoreError::invalid(
                "provider connection archive completion must use the atomic archive boundary",
            ));
        }
        require_replacement_cleanup_predecessor_missing(&transaction, &current)?;
        let binding_matches = current_connection_binding_matches(&transaction, &current)?;
        let (status, outcome_code) = if binding_matches {
            terminal_observation_with_explicit_cleanup(&transaction, &current, observed_status)?
        } else {
            binding_drift_observation(observed_status)
        };
        if current.status == status && current.outcome_code == Some(outcome_code) {
            return Ok(current);
        }
        let now = Utc::now();
        let attestation_sequence =
            insert_operation_attestation(&transaction, &current, stage, observed_status, now)?;
        update_operation_status(
            &transaction,
            &current,
            status,
            outcome_code,
            attestation_sequence,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_provider_credential_operation(operation_id)
    }

    /// Atomically archives the connection and terminalizes its credential
    /// removal after the native owner proves the slot is missing.
    pub fn finish_provider_credential_archive(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.finish_provider_credential_archive_at_stage(
            operation_id,
            plan_sha256,
            observed_status,
            "postflight",
        )
    }

    pub fn reconcile_provider_credential_archive(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.finish_provider_credential_archive_at_stage(
            operation_id,
            plan_sha256,
            observed_status,
            "recovery",
        )
    }

    fn finish_provider_credential_archive_at_stage(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
        stage: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        require_missing_archive_observation(observed_status)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = load_provider_credential_operation(&transaction, operation_id)?;
        validate_exact_operation(&current, plan_sha256)?;
        if current.plan.operation_kind != ProviderCredentialOperationKind::RemoveForArchive
            && !current.cleanup_archives_connection
        {
            return Err(CoreError::invalid(
                "provider credential operation is not an archive removal",
            ));
        }
        if matches!(
            current.status,
            ProviderCredentialOperationStatus::Succeeded
                | ProviderCredentialOperationStatus::NoEffect
        ) {
            if provider_credential_terminal_requires_archive(&current) {
                ensure_provider_connection_is_archived(&transaction, &current.plan.connection_id)?;
                return Ok(current);
            }
            return Err(CoreError::invalid(
                "terminal credential operation did not authorize a connection archive",
            ));
        }
        if provider_credential_has_explicit_durability_barrier(&current) {
            return Ok(current);
        }
        require_replacement_cleanup_predecessor_missing(&transaction, &current)?;
        provider_credential_archive_native_no_effect(&current)?;
        let binding_matches = current_connection_binding_matches(&transaction, &current)?;
        if !binding_matches {
            drop(transaction);
            drop(connection);
            return self.finish_provider_credential_operation_at_stage(
                operation_id,
                plan_sha256,
                observed_status,
                stage,
            );
        }
        let (status, outcome_code) = terminal_observation_with_explicit_cleanup(
            &transaction,
            &current,
            ProviderCredentialObservedStatus::Missing,
        )?;
        if !matches!(
            status,
            ProviderCredentialOperationStatus::Succeeded
                | ProviderCredentialOperationStatus::NoEffect
        ) {
            return Err(CoreError::invalid(
                "provider connection archive cleanup is not terminally proven",
            ));
        }
        let now = Utc::now();
        let attestation_sequence = insert_operation_attestation(
            &transaction,
            &current,
            stage,
            ProviderCredentialObservedStatus::Missing,
            now,
        )?;
        if let Err(error) =
            archive_provider_connection_row(&transaction, current.plan.connection_id.as_str(), now)
        {
            drop(transaction);
            drop(connection);
            self.record_provider_credential_archive_commit_failed(
                operation_id,
                plan_sha256,
                stage,
            )?;
            return Err(error);
        }
        update_operation_status(
            &transaction,
            &current,
            status,
            outcome_code,
            attestation_sequence,
            now,
        )?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        if let Err(error) = transaction.commit().map_err(storage_db_error) {
            drop(connection);
            self.record_provider_credential_archive_commit_failed(
                operation_id,
                plan_sha256,
                stage,
            )?;
            return Err(error);
        }
        drop(connection);
        self.get_provider_credential_operation(operation_id)
    }

    /// Marks a post-vault failure which requires native cleanup before the
    /// credential slot may be reused.
    pub fn mark_provider_credential_cleanup_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
        archive_connection: bool,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        let cleanup_outcome = if observed_status == ProviderCredentialObservedStatus::Unreadable {
            ProviderCredentialOutcomeCode::NativeStatusUnreadable
        } else {
            ProviderCredentialOutcomeCode::ConnectionChanged
        };
        self.mark_provider_credential_cleanup_required_with_outcome(
            operation_id,
            plan_sha256,
            observed_status,
            archive_connection,
            cleanup_outcome,
            None,
        )
    }

    /// Persists a platform-reported durability failure independently from the
    /// slot's immediate visibility. Only an explicit exact cleanup attempt may
    /// later discharge this barrier.
    pub fn mark_provider_credential_durability_recovery_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        archive_connection: bool,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.mark_provider_credential_cleanup_required_with_outcome(
            operation_id,
            plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
            archive_connection,
            ProviderCredentialOutcomeCode::NativeDurabilityUnknown,
            Some(ProviderCredentialDurabilityTarget::OperationSlot),
        )
    }

    /// Converts the intent-only Started cutpoint into an explicit durability
    /// obligation before startup is allowed to inspect native visibility.
    pub fn fence_started_provider_credential_operation_for_recovery(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        let connection = self.connection()?;
        let current = load_provider_credential_operation(&connection, operation_id)?;
        validate_exact_operation(&current, plan_sha256)?;
        if current.status != ProviderCredentialOperationStatus::Started {
            return Err(CoreError::invalid(
                "credential startup fence requires an exact Started operation",
            ));
        }
        let target = recovery_durability_target(&connection, &current)?;
        drop(connection);
        self.mark_provider_credential_cleanup_required_with_outcome(
            operation_id,
            plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
            current.cleanup_archives_connection,
            match target {
                ProviderCredentialDurabilityTarget::OperationSlot => {
                    ProviderCredentialOutcomeCode::NativeDurabilityUnknown
                }
                ProviderCredentialDurabilityTarget::PredecessorSlot => {
                    ProviderCredentialOutcomeCode::NativePredecessorDurabilityUnknown
                }
            },
            Some(target),
        )
    }

    pub fn mark_provider_credential_predecessor_durability_recovery_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        archive_connection: bool,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.mark_provider_credential_cleanup_required_with_outcome(
            operation_id,
            plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
            archive_connection,
            ProviderCredentialOutcomeCode::NativePredecessorDurabilityUnknown,
            Some(ProviderCredentialDurabilityTarget::PredecessorSlot),
        )
    }

    /// Attests that an explicit idempotent native delete completed without a
    /// durability error and that the exact slot is now missing. This is the
    /// only transition which removes a platform durability barrier before the
    /// ordinary terminal reconciliation rules run.
    pub fn attest_provider_credential_durability_repaired(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.attest_provider_credential_durability_repaired_for_target(
            operation_id,
            plan_sha256,
            ProviderCredentialDurabilityTarget::OperationSlot,
        )
    }

    pub fn attest_provider_credential_predecessor_durability_repaired(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.attest_provider_credential_durability_repaired_for_target(
            operation_id,
            plan_sha256,
            ProviderCredentialDurabilityTarget::PredecessorSlot,
        )
    }

    fn attest_provider_credential_durability_repaired_for_target(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        target: ProviderCredentialDurabilityTarget,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let current = load_provider_credential_operation(&transaction, operation_id)?;
        validate_exact_operation(&current, plan_sha256)?;
        if current.status != ProviderCredentialOperationStatus::CleanupRequired
            || !target.is_active(&current)
        {
            return Err(CoreError::invalid(
                "credential durability repair requires an explicit native barrier",
            ));
        }
        let now = Utc::now();
        insert_operation_attestation(
            &transaction,
            &current,
            target.repaired_stage(),
            ProviderCredentialObservedStatus::Missing,
            now,
        )?;
        let other_barrier_active = match target {
            ProviderCredentialDurabilityTarget::OperationSlot => {
                current.predecessor_slot_recovery_required
            }
            ProviderCredentialDurabilityTarget::PredecessorSlot => {
                current.operation_slot_recovery_required
            }
        };
        if other_barrier_active {
            transaction.commit().map_err(storage_db_error)?;
            drop(connection);
            return self.get_provider_credential_operation(operation_id);
        }
        let attestation_sequence = insert_operation_attestation(
            &transaction,
            &current,
            "durability_repair",
            ProviderCredentialObservedStatus::Missing,
            now,
        )?;
        update_operation_status(
            &transaction,
            &current,
            ProviderCredentialOperationStatus::CleanupRequired,
            ProviderCredentialOutcomeCode::ConnectionChanged,
            attestation_sequence,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_provider_credential_operation(operation_id)
    }

    fn mark_provider_credential_cleanup_required_with_outcome(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
        archive_connection: bool,
        cleanup_outcome: ProviderCredentialOutcomeCode,
        durability_target: Option<ProviderCredentialDurabilityTarget>,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let current = load_provider_credential_operation(&transaction, operation_id)?;
        validate_exact_operation(&current, plan_sha256)?;
        if !matches!(
            current.status,
            ProviderCredentialOperationStatus::Started
                | ProviderCredentialOperationStatus::CleanupRequired
                | ProviderCredentialOperationStatus::OutcomeUnknown
        ) {
            return Err(CoreError::invalid(
                "credential cleanup requires an existing uncertain native item",
            ));
        }
        if cleanup_outcome == ProviderCredentialOutcomeCode::NativePredecessorDurabilityUnknown
            && (current.plan.operation_kind != ProviderCredentialOperationKind::Install
                || current.plan.predecessor_authority_id.is_none())
        {
            return Err(CoreError::invalid(
                "credential predecessor durability barrier requires replacement work",
            ));
        }
        let now = Utc::now();
        if let Some(target) = durability_target
            && !target.is_active(&current)
        {
            insert_operation_attestation(
                &transaction,
                &current,
                target.required_stage(),
                ProviderCredentialObservedStatus::Unreadable,
                now,
            )?;
        }
        let attestation_sequence = insert_operation_attestation(
            &transaction,
            &current,
            if archive_connection {
                "cleanup_archive_intent"
            } else {
                "cleanup_remove_intent"
            },
            observed_status,
            now,
        )?;
        let changed = transaction
            .execute(
                "UPDATE provider_credential_operations
                 SET status = 'cleanup_required', outcome_code = ?4,
                     outcome_attestation_sequence = ?3,
                     started_at = CASE
                         WHEN ?7 <> 'missing' THEN COALESCE(started_at, ?5)
                         ELSE started_at
                     END,
                     finished_at = COALESCE(finished_at, ?5), updated_at = ?5
                 WHERE id = ?1 AND plan_sha256 = ?2 AND status = ?6",
                params![
                    current.plan.operation_id,
                    current.plan_sha256,
                    attestation_sequence,
                    cleanup_outcome.as_str(),
                    now.to_rfc3339(),
                    current.status.as_str(),
                    observed_status.as_str(),
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::invalid(
                "provider credential cleanup intent changed concurrently",
            ));
        }
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_provider_credential_operation(operation_id)
    }

    fn record_provider_credential_archive_commit_failed(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        stage: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let current = load_provider_credential_operation(&transaction, operation_id)?;
        validate_exact_operation(&current, plan_sha256)?;
        if current.status == ProviderCredentialOperationStatus::Succeeded
            || (current.status == ProviderCredentialOperationStatus::CleanupRequired
                && current.outcome_code == Some(ProviderCredentialOutcomeCode::ArchiveCommitFailed))
        {
            return Ok(current);
        }
        let prepared_missing = current.status == ProviderCredentialOperationStatus::Prepared
            && current.started_at.is_none()
            && current.preflight_status == ProviderCredentialObservedStatus::Missing;
        let cleanup_missing = current.cleanup_archives_connection
            && current.status == ProviderCredentialOperationStatus::CleanupRequired
            && current.started_at.is_none()
            && current.outcome_code == Some(ProviderCredentialOutcomeCode::ConnectionChanged);
        let started_archive = current.started_at.is_some()
            && matches!(
                current.status,
                ProviderCredentialOperationStatus::Started
                    | ProviderCredentialOperationStatus::CleanupRequired
                    | ProviderCredentialOperationStatus::OutcomeUnknown
            );
        let has_archive_intent = current.plan.operation_kind
            == ProviderCredentialOperationKind::RemoveForArchive
            || current.cleanup_archives_connection;
        if !has_archive_intent || (!prepared_missing && !cleanup_missing && !started_archive) {
            return Err(CoreError::invalid(
                "provider credential archive failure is detached from a started removal",
            ));
        }
        let now = Utc::now();
        let sequence = insert_operation_attestation(
            &transaction,
            &current,
            stage,
            ProviderCredentialObservedStatus::Missing,
            now,
        )?;
        update_operation_status(
            &transaction,
            &current,
            ProviderCredentialOperationStatus::CleanupRequired,
            ProviderCredentialOutcomeCode::ArchiveCommitFailed,
            sequence,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_provider_credential_operation(operation_id)
    }
}

fn provider_credential_archive_native_no_effect(
    operation: &StoredProviderCredentialOperation,
) -> CoreResult<bool> {
    let native_no_effect = operation.started_at.is_none()
        && (operation.preflight_status == ProviderCredentialObservedStatus::Missing
            || (operation.cleanup_archives_connection
                && operation.status == ProviderCredentialOperationStatus::CleanupRequired
                && matches!(
                    operation.outcome_code,
                    Some(
                        ProviderCredentialOutcomeCode::ConnectionChanged
                            | ProviderCredentialOutcomeCode::ArchiveCommitFailed
                    )
                )))
        && matches!(
            operation.status,
            ProviderCredentialOperationStatus::Prepared
                | ProviderCredentialOperationStatus::CleanupRequired
                | ProviderCredentialOperationStatus::OutcomeUnknown
        );
    let native_effect_started = operation.started_at.is_some()
        && matches!(
            operation.status,
            ProviderCredentialOperationStatus::Started
                | ProviderCredentialOperationStatus::CleanupRequired
                | ProviderCredentialOperationStatus::OutcomeUnknown
        );
    if !native_no_effect && !native_effect_started {
        return Err(CoreError::invalid(
            "provider credential archive removal has not started",
        ));
    }
    Ok(native_no_effect)
}

fn ensure_provider_connection_is_archived(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<()> {
    let archived = connection
        .query_row(
            "SELECT archived_at IS NOT NULL FROM provider_connections WHERE id = ?1",
            [connection_id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .unwrap_or(false);
    if !archived {
        return Err(stored_credential_journal_corrupted(
            "provider credential archive terminal is detached from an archived connection",
        ));
    }
    Ok(())
}

fn ensure_no_unresolved_provider_credential_operation(
    transaction: &Connection,
    credential_ref: &str,
) -> CoreResult<()> {
    let unresolved_exists = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM provider_credential_operations
                WHERE credential_ref = ?1
                  AND status IN (
                    'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                  )
             )",
            [credential_ref],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if unresolved_exists {
        return Err(CoreError::invalid(
            "the provider credential slot already has an unresolved native operation",
        ));
    }
    Ok(())
}

struct ProviderCredentialOperationAuthorities {
    operation_id: String,
    credential: Option<ProviderCredentialAccessAuthority>,
    predecessor: Option<ProviderCredentialAccessAuthority>,
}

fn resolve_provider_credential_operation_authorities(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    kind: ProviderCredentialOperationKind,
    connection_binding_sha256: &str,
    proposed_install_authority: Option<&ProviderCredentialAccessAuthority>,
) -> CoreResult<ProviderCredentialOperationAuthorities> {
    let ownership = load_provider_credential_ownership(connection, connection_id)?;
    let predecessor = match ownership.state.as_str() {
        "ordinary_owned" | "discovery_owned" => {
            let authority_id = ownership.authority_id.ok_or_else(|| {
                stored_credential_journal_corrupted(
                    "owned provider credential has no predecessor authority",
                )
            })?;
            let binding_sha256 = ownership.binding_sha256.ok_or_else(|| {
                stored_credential_journal_corrupted(
                    "owned provider credential has no predecessor binding",
                )
            })?;
            if !provider_credential_ownership_authority_is_valid(
                connection,
                connection_id,
                &ownership.state,
                Some(binding_sha256.as_str()),
                &authority_id,
            )? {
                return Err(stored_credential_journal_corrupted(
                    "provider credential predecessor authority is invalid",
                ));
            }
            Some(ProviderCredentialAccessAuthority {
                authority_id,
                connection_binding_sha256: binding_sha256,
            })
        }
        "legacy_pending" | "unowned" | "removed" => None,
        _ => {
            return Err(stored_credential_journal_corrupted(
                "provider credential ownership state is invalid",
            ));
        }
    };
    match kind {
        ProviderCredentialOperationKind::Install => {
            let credential = proposed_install_authority.cloned().ok_or_else(|| {
                CoreError::invalid(
                    "provider credential installation requires a proposed physical-slot authority",
                )
            })?;
            if credential.connection_binding_sha256 != connection_binding_sha256 {
                return Err(CoreError::invalid(
                    "provider credential installation authority binding changed",
                ));
            }
            Ok(ProviderCredentialOperationAuthorities {
                operation_id: credential.authority_id.clone(),
                credential: Some(credential),
                predecessor,
            })
        }
        ProviderCredentialOperationKind::RemoveCredential
        | ProviderCredentialOperationKind::RemoveForArchive => {
            if proposed_install_authority.is_some() {
                return Err(CoreError::invalid(
                    "a removal cannot use an installation authority proposal",
                ));
            }
            Ok(ProviderCredentialOperationAuthorities {
                operation_id: Uuid::new_v4().to_string(),
                credential: predecessor.clone(),
                predecessor,
            })
        }
    }
}

struct PreparedProviderCredentialOperationInput<'a> {
    connection_id: &'a ProviderConnectionId,
    kind: ProviderCredentialOperationKind,
    preflight_status: ProviderCredentialObservedStatus,
    binding: &'a StoredProviderCredentialBinding,
    credential_scope: CredentialScope,
    authorities: &'a ProviderCredentialOperationAuthorities,
    now: DateTime<Utc>,
}

fn insert_prepared_provider_credential_operation(
    transaction: &Transaction<'_>,
    input: &PreparedProviderCredentialOperationInput<'_>,
) -> CoreResult<String> {
    let operation_id = input.authorities.operation_id.clone();
    let operation_sequence = transaction
        .query_row(
            "SELECT COALESCE(MAX(operation_sequence), 0) + 1
             FROM provider_credential_operations
             WHERE credential_ref = ?1",
            [input.binding.credential_ref.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .map_err(storage_db_error)?;
    let plan = ProviderCredentialOperationPlan {
        schema_version: PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION,
        redaction_version: PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION,
        operation_id: operation_id.clone(),
        operation_sequence,
        operation_kind: input.kind,
        connection_id: input.connection_id.clone(),
        credential_ref: input.binding.credential_ref.clone(),
        connection_binding_sha256: input.binding.sha256(&input.credential_scope)?,
        credential_authority_id: input
            .authorities
            .credential
            .as_ref()
            .map(|value| value.authority_id.clone()),
        credential_authority_binding_sha256: input
            .authorities
            .credential
            .as_ref()
            .map(|value| value.connection_binding_sha256.clone()),
        predecessor_authority_id: input
            .authorities
            .predecessor
            .as_ref()
            .map(|value| value.authority_id.clone()),
        predecessor_authority_binding_sha256: input
            .authorities
            .predecessor
            .as_ref()
            .map(|value| value.connection_binding_sha256.clone()),
        credential_scope: input.credential_scope.clone(),
    };
    let plan_json = encode_plan(&plan)?;
    let plan_sha256 = hex::encode(Sha256::digest(plan_json.as_bytes()));
    let preflight_attested_at = input.now.to_rfc3339();
    let preflight_evidence_sha256 = preflight_evidence_sha256(
        &plan,
        &plan_sha256,
        input.preflight_status,
        &preflight_attested_at,
    )?;
    transaction
        .execute(
            "INSERT INTO provider_credential_operations
             (id, connection_id, credential_ref, operation_sequence,
              operation_kind, connection_binding_sha256, credential_authority_id,
              credential_authority_binding_sha256, predecessor_authority_id,
              predecessor_authority_binding_sha256, plan_json,
              plan_sha256, preflight_status, preflight_evidence_sha256,
              preflight_attested_at, native_owner, status, outcome_code,
              outcome_attestation_sequence, schema_version, redaction_version,
              created_at, started_at, finished_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     ?14, ?15, ?16, 'prepared', NULL, NULL, ?17, ?18, ?19,
                     NULL, NULL, ?19)",
            params![
                operation_id,
                plan.connection_id.as_str(),
                plan.credential_ref,
                plan.operation_sequence,
                input.kind.as_str(),
                plan.connection_binding_sha256,
                plan.credential_authority_id,
                plan.credential_authority_binding_sha256,
                plan.predecessor_authority_id,
                plan.predecessor_authority_binding_sha256,
                plan_json,
                plan_sha256,
                input.preflight_status.as_str(),
                preflight_evidence_sha256,
                preflight_attested_at,
                NATIVE_CREDENTIAL_RECOVERY_OWNER,
                PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION,
                PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION,
                input.now.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(operation_id)
}

struct ProviderCredentialOwnershipProjection {
    state: String,
    binding_sha256: Option<String>,
    authority_id: Option<String>,
    authority_sequence: u64,
}

fn load_provider_credential_ownership(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<ProviderCredentialOwnershipProjection> {
    connection
        .query_row(
            "SELECT ownership_state, connection_binding_sha256, authority_id,
                    authority_sequence
             FROM provider_credential_ownership
             WHERE connection_id = ?1 AND credential_ref = ?1",
            [connection_id.as_str()],
            |row| {
                Ok(ProviderCredentialOwnershipProjection {
                    state: row.get(0)?,
                    binding_sha256: row.get(1)?,
                    authority_id: row.get(2)?,
                    authority_sequence: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            stored_credential_journal_corrupted(
                "provider credential ownership projection is missing",
            )
        })
}

pub(crate) fn provider_credential_ownership_authority_is_valid(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    ownership_state: &str,
    binding_sha256: Option<&str>,
    authority_id: &str,
) -> CoreResult<bool> {
    let event = connection
        .query_row(
            "SELECT event.ownership_state, event.connection_binding_sha256,
                    event.authority_id, event.source_kind, event.source_id,
                    event.authority_sequence
             FROM provider_credential_ownership AS ownership
             JOIN provider_credential_ownership_events AS event
               ON event.connection_id = ownership.connection_id
              AND event.authority_sequence = ownership.authority_sequence
             WHERE ownership.connection_id = ?1
               AND ownership.credential_ref = ?1
               AND ownership.authority_sequence = (
                 SELECT MAX(latest.authority_sequence)
                 FROM provider_credential_ownership_events AS latest
                 WHERE latest.connection_id = ownership.connection_id
               )",
            [connection_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    let Some((event_state, event_binding, event_authority, source_kind, source_id, _)) = event
    else {
        return Ok(false);
    };
    if event_state != ownership_state
        || event_binding.as_deref() != binding_sha256
        || event_authority != authority_id
        || (source_kind == "ordinary_operation" && source_id != authority_id)
    {
        return Ok(false);
    }
    validate_provider_credential_ownership_event_source(
        connection,
        ProviderCredentialOwnershipSource {
            connection_id,
            ownership_state,
            binding_sha256,
            authority_id,
            source_id: &source_id,
            source_kind: &source_kind,
            validation: ProviderCredentialOwnershipValidation::CurrentAccess,
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderCredentialOwnershipValidation {
    CurrentAccess,
    ActiveSlotGarbage,
    ArchivedSlotGarbage,
}

struct ProviderCredentialOwnershipSource<'a> {
    connection_id: &'a ProviderConnectionId,
    ownership_state: &'a str,
    binding_sha256: Option<&'a str>,
    authority_id: &'a str,
    source_id: &'a str,
    source_kind: &'a str,
    validation: ProviderCredentialOwnershipValidation,
}

fn validate_provider_credential_ownership_event_source(
    connection: &Connection,
    source: ProviderCredentialOwnershipSource<'_>,
) -> CoreResult<bool> {
    let ProviderCredentialOwnershipSource {
        connection_id,
        ownership_state,
        binding_sha256,
        authority_id,
        source_id,
        source_kind,
        validation,
    } = source;
    match (ownership_state, source_kind) {
        ("ordinary_owned", "ordinary_operation") => {
            if source_id != authority_id {
                return Ok(false);
            }
            let exists = connection
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM provider_credential_operations
                       WHERE id = ?1 AND connection_id = ?2 AND credential_ref = ?2
                     )",
                    params![authority_id, connection_id.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if !exists {
                return Ok(false);
            }
            let operation = load_provider_credential_operation(connection, authority_id)?;
            Ok(operation.plan.connection_id == *connection_id
                && operation.plan.credential_ref == connection_id.as_str()
                && operation.plan.operation_kind == ProviderCredentialOperationKind::Install
                && operation.status == ProviderCredentialOperationStatus::Succeeded
                && binding_sha256 == Some(operation.plan.connection_binding_sha256.as_str()))
        }
        ("discovery_owned", "discovery_commit") => {
            let Some(binding_sha256) = binding_sha256 else {
                return Ok(false);
            };
            if validation == ProviderCredentialOwnershipValidation::ArchivedSlotGarbage {
                crate::discovery_repository::validate_archived_discovery_credential_ownership_authority_for_slot_gc(
                    connection,
                    connection_id,
                    authority_id,
                    source_id,
                    binding_sha256,
                )?;
            } else {
                crate::discovery_repository::validate_discovery_credential_ownership_authority(
                    connection,
                    connection_id,
                    authority_id,
                    source_id,
                    binding_sha256,
                )?;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

type ProviderCredentialSlotGarbageRow = (String, u64, String, String, String);

#[derive(Debug, Clone)]
struct ProviderCredentialOwnershipEventHistory {
    authority_sequence: u64,
    ownership_state: String,
    binding_sha256: Option<String>,
    authority_id: String,
    source_kind: String,
    source_id: String,
    created_at: DateTime<Utc>,
}

fn load_provider_credential_ownership_event_history(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    first_authority_sequence: u64,
) -> CoreResult<(
    Vec<ProviderCredentialOwnershipEventHistory>,
    Option<DateTime<Utc>>,
)> {
    let projection = load_provider_credential_ownership(connection, connection_id)?;
    let mut statement = connection
        .prepare(
            "SELECT authority_sequence, ownership_state, connection_binding_sha256,
                    authority_id, source_kind, source_id, created_at
             FROM provider_credential_ownership_events
             WHERE connection_id = ?1 AND authority_sequence >= ?2
             ORDER BY authority_sequence",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![connection_id.as_str(), first_authority_sequence],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    drop(statement);
    let events = rows
        .into_iter()
        .map(|row| {
            Ok(ProviderCredentialOwnershipEventHistory {
                authority_sequence: row.0,
                ownership_state: row.1,
                binding_sha256: row.2,
                authority_id: row.3,
                source_kind: row.4,
                source_id: row.5,
                created_at: parse_timestamp("ownership event created_at", &row.6)?,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let Some(latest) = events.last() else {
        return Err(stored_credential_journal_corrupted(
            "provider credential slot gc ownership history is missing",
        ));
    };
    for (offset, event) in events.iter().enumerate() {
        if event.authority_sequence != first_authority_sequence.saturating_add(offset as u64) {
            return Err(stored_credential_journal_corrupted(
                "provider credential slot gc ownership history is not contiguous",
            ));
        }
    }
    if latest.authority_sequence != projection.authority_sequence
        || latest.ownership_state != projection.state
        || latest.binding_sha256 != projection.binding_sha256
        || Some(latest.authority_id.as_str()) != projection.authority_id.as_deref()
    {
        return Err(stored_credential_journal_corrupted(
            "provider credential slot gc ownership projection is not the latest event",
        ));
    }
    let archived_at = connection
        .query_row(
            "SELECT archived_at FROM provider_connections WHERE id = ?1",
            [connection_id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            stored_credential_journal_corrupted("provider credential slot gc connection is missing")
        })?
        .as_deref()
        .map(|value| parse_timestamp("archived connection archived_at", value))
        .transpose()?;
    Ok((events, archived_at))
}

fn load_provider_credential_slot_garbage(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    authority_sequence: u64,
) -> CoreResult<ProviderCredentialSlotGarbage> {
    let row = connection
        .query_row(
            "SELECT connection_id, authority_sequence, authority_id,
                    connection_binding_sha256, status
             FROM provider_credential_slot_gc
             WHERE connection_id = ?1 AND authority_sequence = ?2",
            params![connection_id.as_str(), authority_sequence],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider credential slot gc was not found",
                false,
            )
        })?;
    decode_provider_credential_slot_garbage(connection, row)
}

fn validate_owned_provider_credential_event_predecessor(
    connection: &Connection,
    event: &ProviderCredentialOwnershipEventHistory,
    predecessor: Option<(&str, &str)>,
) -> CoreResult<()> {
    match event.source_kind.as_str() {
        "ordinary_operation" => {
            let operation = load_provider_credential_operation(connection, &event.source_id)?;
            let expected_authority = predecessor.map(|value| value.0);
            let expected_binding = predecessor.map(|value| value.1);
            if operation.plan.operation_kind != ProviderCredentialOperationKind::Install
                || operation.status != ProviderCredentialOperationStatus::Succeeded
                || operation.plan.predecessor_authority_id.as_deref() != expected_authority
                || operation
                    .plan
                    .predecessor_authority_binding_sha256
                    .as_deref()
                    != expected_binding
                || operation.updated_at != event.created_at
            {
                return Err(stored_credential_journal_corrupted(
                    "provider credential slot gc owned successor is detached from its predecessor",
                ));
            }
        }
        "discovery_commit" if predecessor.is_some() => {
            return Err(stored_credential_journal_corrupted(
                "provider credential discovery authority cannot replace an existing owned slot",
            ));
        }
        "discovery_commit" => {}
        _ => {
            return Err(stored_credential_journal_corrupted(
                "provider credential slot gc owned event source is invalid",
            ));
        }
    }
    Ok(())
}

fn validate_removed_provider_credential_event(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    event: &ProviderCredentialOwnershipEventHistory,
    predecessor: Option<(&str, &str)>,
    archived_at: Option<DateTime<Utc>>,
) -> CoreResult<()> {
    if event.source_kind != "ordinary_operation" || event.source_id != event.authority_id {
        return Err(stored_credential_journal_corrupted(
            "provider credential removed event has no ordinary operation authority",
        ));
    }
    let operation = load_provider_credential_operation(connection, &event.source_id)?;
    let expected_authority = predecessor.map(|value| value.0);
    let expected_binding = predecessor.map(|value| value.1);
    if operation.plan.connection_id != *connection_id
        || operation.plan.credential_ref != connection_id.as_str()
        || operation.plan.predecessor_authority_id.as_deref() != expected_authority
        || operation
            .plan
            .predecessor_authority_binding_sha256
            .as_deref()
            != expected_binding
        || !matches!(
            operation.status,
            ProviderCredentialOperationStatus::Succeeded
                | ProviderCredentialOperationStatus::NoEffect
        )
        || operation.updated_at != event.created_at
    {
        return Err(stored_credential_journal_corrupted(
            "provider credential removed event is detached from its predecessor authority",
        ));
    }
    let outcome_sequence = operation.outcome_attestation_sequence.ok_or_else(|| {
        stored_credential_journal_corrupted(
            "provider credential removed event has no terminal observation",
        )
    })?;
    let (stage, slot_status, attested_at) = connection
        .query_row(
            "SELECT stage, slot_status, attested_at
             FROM provider_credential_operation_attestations
             WHERE operation_id = ?1 AND sequence = ?2",
            params![operation.plan.operation_id, outcome_sequence],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            stored_credential_journal_corrupted(
                "provider credential removed event terminal observation is missing",
            )
        })?;
    if !matches!(stage.as_str(), "postflight" | "recovery")
        || slot_status != ProviderCredentialObservedStatus::Missing.as_str()
        || parse_timestamp("removed credential attested_at", &attested_at)? != event.created_at
    {
        return Err(stored_credential_journal_corrupted(
            "provider credential removed event terminal observation is invalid",
        ));
    }
    if let Some(archived_at) = archived_at
        && (!provider_credential_terminal_requires_archive(&operation)
            || archived_at != event.created_at)
    {
        return Err(stored_credential_journal_corrupted(
            "provider credential archived slot gc is detached from its atomic archive",
        ));
    }
    Ok(())
}

fn validate_provider_credential_slot_garbage_history(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    authority_sequence: u64,
    authority_id: &str,
    binding_sha256: &str,
) -> CoreResult<ProviderCredentialOwnershipValidation> {
    let (events, archived_at) = load_provider_credential_ownership_event_history(
        connection,
        connection_id,
        authority_sequence,
    )?;
    let first = events.first().ok_or_else(|| {
        stored_credential_journal_corrupted(
            "provider credential slot gc authority event is missing",
        )
    })?;
    if !matches!(
        first.ownership_state.as_str(),
        "ordinary_owned" | "discovery_owned"
    ) || first.binding_sha256.as_deref() != Some(binding_sha256)
        || first.authority_id != authority_id
        || (first.source_kind == "ordinary_operation" && first.source_id != authority_id)
        || events.len() < 2
    {
        return Err(stored_credential_journal_corrupted(
            "provider credential slot gc target is not a superseded owned event",
        ));
    }
    let validation = if archived_at.is_some() {
        ProviderCredentialOwnershipValidation::ArchivedSlotGarbage
    } else {
        ProviderCredentialOwnershipValidation::ActiveSlotGarbage
    };
    let mut predecessor = Some((first.authority_id.as_str(), binding_sha256));
    for (index, event) in events.iter().enumerate() {
        if event.source_kind == "ordinary_operation" && event.source_id != event.authority_id {
            return Err(stored_credential_journal_corrupted(
                "provider credential slot gc ownership event source changed",
            ));
        }
        match event.ownership_state.as_str() {
            "ordinary_owned" | "discovery_owned" => {
                let source_is_valid = validate_provider_credential_ownership_event_source(
                    connection,
                    ProviderCredentialOwnershipSource {
                        connection_id,
                        ownership_state: &event.ownership_state,
                        binding_sha256: event.binding_sha256.as_deref(),
                        authority_id: &event.authority_id,
                        source_id: &event.source_id,
                        source_kind: &event.source_kind,
                        validation,
                    },
                )?;
                if !source_is_valid {
                    return Err(stored_credential_journal_corrupted(
                        "provider credential slot gc ownership source is invalid",
                    ));
                }
                if index > 0 {
                    validate_owned_provider_credential_event_predecessor(
                        connection,
                        event,
                        predecessor,
                    )?;
                }
                predecessor = Some((
                    event.authority_id.as_str(),
                    event.binding_sha256.as_deref().ok_or_else(|| {
                        stored_credential_journal_corrupted(
                            "owned provider credential event binding is missing",
                        )
                    })?,
                ));
            }
            "removed" => {
                let final_archive = (index + 1 == events.len()).then_some(archived_at).flatten();
                validate_removed_provider_credential_event(
                    connection,
                    connection_id,
                    event,
                    predecessor,
                    final_archive,
                )?;
                predecessor = None;
            }
            _ => {
                return Err(stored_credential_journal_corrupted(
                    "provider credential slot gc ownership event state is invalid",
                ));
            }
        }
    }
    if archived_at.is_some()
        && events
            .last()
            .is_none_or(|event| event.ownership_state != "removed")
    {
        return Err(stored_credential_journal_corrupted(
            "archived provider credential slot gc has no final removed event",
        ));
    }
    Ok(validation)
}

pub(crate) fn validate_superseded_provider_credential_ownership_event_history(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    authority_sequence: u64,
    authority_id: &str,
    binding_sha256: &str,
) -> CoreResult<()> {
    validate_provider_credential_slot_garbage_history(
        connection,
        connection_id,
        authority_sequence,
        authority_id,
        binding_sha256,
    )?;
    Ok(())
}

fn decode_provider_credential_slot_garbage(
    connection: &Connection,
    row: ProviderCredentialSlotGarbageRow,
) -> CoreResult<ProviderCredentialSlotGarbage> {
    let (connection_id, authority_sequence, authority_id, binding_sha256, status) = row;
    let connection_id = ProviderConnectionId::from(connection_id);
    validate_provider_credential_slot_garbage_history(
        connection,
        &connection_id,
        authority_sequence,
        &authority_id,
        &binding_sha256,
    )?;
    Ok(ProviderCredentialSlotGarbage {
        connection_id,
        authority_sequence,
        authority: ProviderCredentialAccessAuthority {
            authority_id,
            connection_binding_sha256: binding_sha256,
        },
        status: ProviderCredentialSlotGarbageStatus::parse(&status)?,
    })
}

type ProviderCredentialOperationRow = (
    String,
    String,
    String,
    u64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<u64>,
    u32,
    u32,
    String,
    Option<String>,
    Option<String>,
    String,
);

fn provider_credential_operation_row(
    row: &Row<'_>,
) -> rusqlite::Result<ProviderCredentialOperationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
        row.get(17)?,
        row.get(18)?,
        row.get(19)?,
    ))
}

fn load_provider_credential_operation(
    connection: &Connection,
    operation_id: &str,
) -> CoreResult<StoredProviderCredentialOperation> {
    let row = connection
        .query_row(
            "SELECT id, connection_id, credential_ref, operation_sequence,
                    operation_kind, connection_binding_sha256, plan_json,
                    plan_sha256, preflight_status, preflight_evidence_sha256,
                    preflight_attested_at, status, outcome_code,
                    outcome_attestation_sequence, schema_version, redaction_version,
                    created_at, started_at, finished_at, updated_at
             FROM provider_credential_operations
             WHERE id = ?1",
            [operation_id],
            provider_credential_operation_row,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider credential operation was not found",
                false,
            )
        })?;
    let mut operation = decode_operation_row(row)?;
    operation.cleanup_archives_connection =
        validate_provider_credential_cleanup_intents(connection, &operation)?.1;
    apply_provider_credential_durability_ledger(connection, &mut operation)?;
    validate_operation_evidence(connection, &operation)?;
    if provider_credential_terminal_requires_archive(&operation) {
        ensure_provider_connection_is_archived(connection, &operation.plan.connection_id)?;
    }
    Ok(operation)
}

fn validate_provider_credential_cleanup_intents(
    connection: &Connection,
    operation: &StoredProviderCredentialOperation,
) -> CoreResult<(bool, bool)> {
    let mut statement = connection
        .prepare(
            "SELECT sequence, stage, slot_status, evidence_sha256, native_owner,
                    schema_version, redaction_version, attested_at
             FROM provider_credential_operation_attestations
             WHERE operation_id = ?1
               AND stage IN ('cleanup_remove_intent', 'cleanup_archive_intent')
             ORDER BY sequence",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([operation.plan.operation_id.as_str()], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, u32>(5)?,
                row.get::<_, u32>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    drop(statement);

    let mut archives_connection = false;
    for (sequence, stage, slot_status, evidence, owner, schema, redaction, attested_at) in &rows {
        let slot_status = ProviderCredentialObservedStatus::parse(slot_status)?;
        let valid_authority = owner == NATIVE_CREDENTIAL_RECOVERY_OWNER
            && *schema == PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION
            && *redaction == PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION
            && matches!(
                stage.as_str(),
                "cleanup_remove_intent" | "cleanup_archive_intent"
            );
        let expected =
            outcome_evidence_sha256(operation, *sequence, stage, slot_status, attested_at)?;
        if !valid_authority || expected != *evidence {
            return Err(stored_credential_journal_corrupted(
                "stored provider credential cleanup intent evidence is invalid",
            ));
        }
        archives_connection |= stage == "cleanup_archive_intent";
    }
    Ok((!rows.is_empty(), archives_connection))
}

fn provider_credential_terminal_requires_archive(
    operation: &StoredProviderCredentialOperation,
) -> bool {
    operation.status.is_terminal()
        && (operation.cleanup_archives_connection
            || (operation.plan.operation_kind == ProviderCredentialOperationKind::RemoveForArchive
                && (operation.status == ProviderCredentialOperationStatus::Succeeded
                    || (operation.status == ProviderCredentialOperationStatus::NoEffect
                        && operation.outcome_code
                            == Some(ProviderCredentialOutcomeCode::NativeEffectAbsent)
                        && operation.preflight_status
                            == ProviderCredentialObservedStatus::Missing))))
}

fn decode_operation_row(
    row: ProviderCredentialOperationRow,
) -> CoreResult<StoredProviderCredentialOperation> {
    let (
        operation_id,
        connection_id,
        credential_ref,
        operation_sequence,
        operation_kind,
        connection_binding_sha256,
        plan_json,
        plan_sha256,
        preflight_status,
        preflight_evidence_sha256,
        preflight_attested_at,
        status,
        outcome_code,
        outcome_attestation_sequence,
        schema_version,
        redaction_version,
        created_at,
        started_at,
        finished_at,
        updated_at,
    ) = row;
    let plan =
        serde_json::from_str::<ProviderCredentialOperationPlan>(&plan_json).map_err(|_| {
            stored_credential_journal_corrupted("stored credential operation plan is invalid")
        })?;
    let decoded_kind = ProviderCredentialOperationKind::parse(&operation_kind)?;
    if schema_version != PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION
        || redaction_version != PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION
        || plan.schema_version != schema_version
        || plan.redaction_version != redaction_version
        || plan.operation_id != operation_id
        || plan.operation_sequence != operation_sequence
        || plan.operation_kind != decoded_kind
        || plan.connection_id.as_str() != connection_id
        || plan.credential_ref != credential_ref
        || plan.connection_binding_sha256 != connection_binding_sha256
        || hex::encode(Sha256::digest(plan_json.as_bytes())) != plan_sha256
    {
        return Err(stored_credential_journal_corrupted(
            "stored credential operation plan binding is invalid",
        ));
    }
    let status = ProviderCredentialOperationStatus::parse(&status)?;
    let outcome_code = outcome_code
        .as_deref()
        .map(ProviderCredentialOutcomeCode::parse)
        .transpose()?;
    let record = StoredProviderCredentialOperation {
        plan,
        plan_sha256,
        preflight_evidence_sha256,
        preflight_attested_at: parse_timestamp("preflight_attested_at", &preflight_attested_at)?,
        preflight_status: ProviderCredentialObservedStatus::parse(&preflight_status)?,
        status,
        outcome_code,
        outcome_attestation_sequence,
        cleanup_archives_connection: false,
        operation_slot_recovery_required: false,
        predecessor_slot_recovery_required: false,
        created_at: parse_timestamp("created_at", &created_at)?,
        started_at: started_at
            .as_deref()
            .map(|value| parse_timestamp("started_at", value))
            .transpose()?,
        finished_at: finished_at
            .as_deref()
            .map(|value| parse_timestamp("finished_at", value))
            .transpose()?,
        updated_at: parse_timestamp("updated_at", &updated_at)?,
    };
    validate_record_shape(&record)?;
    Ok(record)
}

fn encode_plan(plan: &ProviderCredentialOperationPlan) -> CoreResult<String> {
    let encoded = serde_json::to_string(plan)
        .map_err(|_| CoreError::internal("cannot encode provider credential operation plan"))?;
    if encoded.len() > MAX_PROVIDER_CREDENTIAL_PLAN_BYTES {
        return Err(CoreError::invalid(
            "provider credential operation plan exceeds its storage bound",
        ));
    }
    Ok(encoded)
}

struct StoredProviderCredentialBinding {
    template_id: String,
    template_version: u32,
    api_origin: String,
    config_json: String,
    credential_ref: String,
    credential_scope_json: String,
}

impl StoredProviderCredentialBinding {
    fn sha256(&self, credential_scope: &CredentialScope) -> CoreResult<String> {
        let config =
            serde_json::from_str::<serde_json::Value>(&self.config_json).map_err(|_| {
                stored_credential_journal_corrupted(
                    "provider connection config cannot be decoded for credential journaling",
                )
            })?;
        provider_credential_binding_sha256_from_parts(
            &self.credential_ref,
            &self.template_id,
            self.template_version,
            &self.api_origin,
            config,
            &self.credential_ref,
            credential_scope,
        )
    }
}

/// Canonical binding hash shared by pre-publication discovery installation
/// and the final durable ownership projection.
pub fn provider_credential_binding_sha256_for_connection(
    connection: &ProviderConnection,
) -> CoreResult<String> {
    let credential_ref = connection
        .credential_ref
        .as_ref()
        .ok_or_else(|| CoreError::invalid("provider connection has no credential reference"))?;
    let credential_scope = connection
        .credential_scope
        .as_ref()
        .ok_or_else(|| CoreError::invalid("provider connection has no credential scope"))?;
    if credential_ref.0 != connection.id.0 {
        return Err(CoreError::invalid(
            "provider credential reference is detached from its connection",
        ));
    }
    let config = serde_json::to_value(&connection.config)
        .map_err(|_| CoreError::internal("cannot encode provider credential connection config"))?;
    provider_credential_binding_sha256_from_parts(
        connection.id.as_str(),
        connection.template_id.as_str(),
        connection.template_version,
        connection.api_origin.as_str(),
        config,
        &credential_ref.0,
        credential_scope,
    )
}

#[allow(clippy::too_many_arguments)]
fn provider_credential_binding_sha256_from_parts(
    connection_id: &str,
    template_id: &str,
    template_version: u32,
    api_origin: &str,
    config: serde_json::Value,
    credential_ref: &str,
    credential_scope: &CredentialScope,
) -> CoreResult<String> {
    let binding = ProviderCredentialConnectionBinding {
        schema_version: PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION,
        connection_id,
        template_id,
        template_version,
        api_origin,
        config,
        credential_ref,
        credential_scope,
    };
    let encoded = serde_json::to_vec(&binding)
        .map_err(|_| CoreError::internal("cannot encode provider credential connection binding"))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn load_active_credential_binding(
    transaction: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<StoredProviderCredentialBinding> {
    transaction
        .query_row(
            "SELECT template_id, template_version, api_origin, config_json,
                    credential_ref, credential_scope_json
             FROM provider_connections
             WHERE id = ?1
               AND archived_at IS NULL
               AND credential_ref IS NOT NULL
               AND credential_scope_json IS NOT NULL",
            [connection_id.as_str()],
            |row| {
                Ok(StoredProviderCredentialBinding {
                    template_id: row.get(0)?,
                    template_version: row.get(1)?,
                    api_origin: row.get(2)?,
                    config_json: row.get(3)?,
                    credential_ref: row.get(4)?,
                    credential_scope_json: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| CoreError::invalid("provider connection has no active credential binding"))
}

fn load_archived_credential_binding(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<StoredProviderCredentialBinding> {
    connection
        .query_row(
            "SELECT template_id, template_version, api_origin, config_json,
                    credential_ref, credential_scope_json
             FROM provider_connections
             WHERE id = ?1
               AND archived_at IS NOT NULL
               AND credential_ref IS NOT NULL
               AND credential_scope_json IS NOT NULL",
            [connection_id.as_str()],
            |row| {
                Ok(StoredProviderCredentialBinding {
                    template_id: row.get(0)?,
                    template_version: row.get(1)?,
                    api_origin: row.get(2)?,
                    config_json: row.get(3)?,
                    credential_ref: row.get(4)?,
                    credential_scope_json: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            stored_credential_journal_corrupted(
                "archived provider connection has no historical credential binding",
            )
        })
}

fn current_connection_binding_matches(
    transaction: &Connection,
    operation: &StoredProviderCredentialOperation,
) -> CoreResult<bool> {
    let binding = match load_active_credential_binding(transaction, &operation.plan.connection_id) {
        Ok(binding) => binding,
        Err(error) if error.code == CoreErrorCode::InvalidInput => return Ok(false),
        Err(error) => return Err(error),
    };
    if binding.credential_ref != operation.plan.credential_ref {
        return Ok(false);
    }
    let credential_scope = serde_json::from_str::<CredentialScope>(&binding.credential_scope_json)
        .map_err(|_| {
            stored_credential_journal_corrupted(
                "provider credential scope cannot be decoded during operation validation",
            )
        })?;
    Ok(
        binding.sha256(&credential_scope)? == operation.plan.connection_binding_sha256
            && credential_scope == operation.plan.credential_scope,
    )
}

fn validate_current_connection_binding(
    transaction: &Connection,
    operation: &StoredProviderCredentialOperation,
) -> CoreResult<()> {
    if !current_connection_binding_matches(transaction, operation)? {
        return Err(CoreError::invalid(
            "provider connection binding changed during its native credential operation",
        ));
    }
    Ok(())
}

pub(crate) fn provider_credential_connection_binding_sha256(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<String> {
    let binding = load_active_credential_binding(connection, connection_id)?;
    let credential_scope = serde_json::from_str::<CredentialScope>(&binding.credential_scope_json)
        .map_err(|_| {
            stored_credential_journal_corrupted(
                "provider credential scope cannot be decoded for ownership projection",
            )
        })?;
    binding.sha256(&credential_scope)
}

pub(crate) fn provider_credential_archived_connection_binding_sha256(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<String> {
    let binding = load_archived_credential_binding(connection, connection_id)?;
    let credential_scope = serde_json::from_str::<CredentialScope>(&binding.credential_scope_json)
        .map_err(|_| {
            stored_credential_journal_corrupted(
                "archived provider credential scope cannot be decoded for historical authority",
            )
        })?;
    binding.sha256(&credential_scope)
}

fn terminal_observation(
    operation: &StoredProviderCredentialOperation,
    observed_status: ProviderCredentialObservedStatus,
) -> CoreResult<(
    ProviderCredentialOperationStatus,
    ProviderCredentialOutcomeCode,
)> {
    if !matches!(
        operation.status,
        ProviderCredentialOperationStatus::Prepared
            | ProviderCredentialOperationStatus::Started
            | ProviderCredentialOperationStatus::CleanupRequired
            | ProviderCredentialOperationStatus::OutcomeUnknown
    ) {
        return Err(CoreError::invalid(
            "provider credential operation is already terminal",
        ));
    }
    Ok(provider_credential_terminal_outcome(
        operation.plan.operation_kind,
        operation.started_at.is_some(),
        operation.plan.predecessor_authority_id.is_some(),
        operation.preflight_status,
        observed_status,
    ))
}

fn recovery_durability_target(
    connection: &Connection,
    operation: &StoredProviderCredentialOperation,
) -> CoreResult<ProviderCredentialDurabilityTarget> {
    let predecessor = validate_provider_credential_predecessor_evidence(connection, operation)?;
    if operation.plan.operation_kind == ProviderCredentialOperationKind::Install
        && operation.plan.predecessor_authority_id.is_some()
        && predecessor.delete_intent
        && !predecessor.missing
    {
        Ok(ProviderCredentialDurabilityTarget::PredecessorSlot)
    } else {
        Ok(ProviderCredentialDurabilityTarget::OperationSlot)
    }
}

fn terminal_observation_with_explicit_cleanup(
    connection: &Connection,
    operation: &StoredProviderCredentialOperation,
    observed_status: ProviderCredentialObservedStatus,
) -> CoreResult<(
    ProviderCredentialOperationStatus,
    ProviderCredentialOutcomeCode,
)> {
    require_replacement_cleanup_predecessor_missing(connection, operation)?;
    if operation.plan.operation_kind == ProviderCredentialOperationKind::Install
        && operation.plan.predecessor_authority_id.is_some()
        && observed_status == ProviderCredentialObservedStatus::Available
        && !validate_provider_credential_predecessor_evidence(connection, operation)?.missing
    {
        // A restored SQLite snapshot can predate the durable attestations that
        // predecessor A was deleted while native slot B survives the rollback.
        // Availability proves only that B exists; without the predecessor
        // evidence it must never project B as the owned credential.
        return Ok((
            ProviderCredentialOperationStatus::OutcomeUnknown,
            ProviderCredentialOutcomeCode::ConnectionChanged,
        ));
    }
    if operation.status == ProviderCredentialOperationStatus::CleanupRequired
        && operation.plan.operation_kind == ProviderCredentialOperationKind::Install
        && observed_status == ProviderCredentialObservedStatus::Missing
        && validate_provider_credential_cleanup_intents(connection, operation)?.0
    {
        let predecessor = validate_provider_credential_predecessor_evidence(connection, operation)?;
        if operation.plan.predecessor_authority_id.is_none() || predecessor.missing {
            return Ok((
                ProviderCredentialOperationStatus::NoEffect,
                ProviderCredentialOutcomeCode::ConnectionChanged,
            ));
        }
    }
    terminal_observation(operation, observed_status)
}

fn require_replacement_cleanup_predecessor_missing(
    connection: &Connection,
    operation: &StoredProviderCredentialOperation,
) -> CoreResult<()> {
    if operation.status != ProviderCredentialOperationStatus::CleanupRequired {
        return Ok(());
    }
    if operation.plan.operation_kind != ProviderCredentialOperationKind::Install
        || operation.plan.predecessor_authority_id.is_none()
    {
        return Ok(());
    }
    let (has_cleanup_intent, _) =
        validate_provider_credential_cleanup_intents(connection, operation)?;
    if !has_cleanup_intent {
        return Err(stored_credential_journal_corrupted(
            "provider credential cleanup state has no durable cleanup intent",
        ));
    }
    let predecessor = validate_provider_credential_predecessor_evidence(connection, operation)?;
    if !predecessor.delete_intent || !predecessor.missing {
        return Err(CoreError::invalid(
            "replacement credential cleanup requires exact predecessor-missing evidence",
        ));
    }
    Ok(())
}

fn provider_credential_terminal_outcome(
    kind: ProviderCredentialOperationKind,
    started: bool,
    had_owned_predecessor: bool,
    preflight: ProviderCredentialObservedStatus,
    observed: ProviderCredentialObservedStatus,
) -> (
    ProviderCredentialOperationStatus,
    ProviderCredentialOutcomeCode,
) {
    use ProviderCredentialObservedStatus::{Available, Missing, Unreadable};
    use ProviderCredentialOperationKind::{Install, RemoveCredential, RemoveForArchive};
    use ProviderCredentialOperationStatus::{NoEffect, OutcomeUnknown, Succeeded};
    use ProviderCredentialOutcomeCode::{
        ConnectionChanged, NativeEffectAbsent, NativeEffectConfirmed, NativeStatusUnreadable,
    };

    match (kind, started, observed) {
        (_, _, Unreadable) => (OutcomeUnknown, NativeStatusUnreadable),
        (Install, true, Missing) if had_owned_predecessor => (OutcomeUnknown, ConnectionChanged),
        (Install, _, Missing) => (NoEffect, NativeEffectAbsent),
        (Install, false, Available) => (OutcomeUnknown, NativeEffectConfirmed),
        (Install, true, Available) | (RemoveCredential | RemoveForArchive, true, Missing) => {
            (Succeeded, NativeEffectConfirmed)
        }
        (RemoveCredential | RemoveForArchive, true, Available) => {
            (OutcomeUnknown, ConnectionChanged)
        }
        (RemoveCredential | RemoveForArchive, false, Missing) => {
            if preflight == Missing {
                (NoEffect, NativeEffectAbsent)
            } else {
                (NoEffect, ConnectionChanged)
            }
        }
        (RemoveCredential | RemoveForArchive, false, Available) => {
            if preflight == Available {
                (NoEffect, NativeEffectAbsent)
            } else {
                (OutcomeUnknown, ConnectionChanged)
            }
        }
    }
}

fn binding_drift_observation(
    observed_status: ProviderCredentialObservedStatus,
) -> (
    ProviderCredentialOperationStatus,
    ProviderCredentialOutcomeCode,
) {
    match observed_status {
        ProviderCredentialObservedStatus::Missing => (
            ProviderCredentialOperationStatus::NoEffect,
            ProviderCredentialOutcomeCode::ConnectionChanged,
        ),
        ProviderCredentialObservedStatus::Available => (
            ProviderCredentialOperationStatus::OutcomeUnknown,
            ProviderCredentialOutcomeCode::ConnectionChanged,
        ),
        ProviderCredentialObservedStatus::Unreadable => (
            ProviderCredentialOperationStatus::OutcomeUnknown,
            ProviderCredentialOutcomeCode::NativeStatusUnreadable,
        ),
    }
}

fn update_operation_status(
    transaction: &Transaction<'_>,
    current: &StoredProviderCredentialOperation,
    status: ProviderCredentialOperationStatus,
    outcome_code: ProviderCredentialOutcomeCode,
    attestation_sequence: u64,
    now: DateTime<Utc>,
) -> CoreResult<()> {
    let preserve_finished_at = matches!(
        current.status,
        ProviderCredentialOperationStatus::CleanupRequired
            | ProviderCredentialOperationStatus::OutcomeUnknown
    );
    let finished_at = if preserve_finished_at {
        current.finished_at.unwrap_or(now)
    } else {
        now
    };
    let changed = transaction
        .execute(
            "UPDATE provider_credential_operations
             SET status = ?3, outcome_code = ?4, outcome_attestation_sequence = ?5,
                 finished_at = ?6, updated_at = ?7
             WHERE id = ?1 AND plan_sha256 = ?2 AND status = ?8",
            params![
                current.plan.operation_id,
                current.plan_sha256,
                status.as_str(),
                outcome_code.as_str(),
                attestation_sequence,
                finished_at.to_rfc3339(),
                now.to_rfc3339(),
                current.status.as_str(),
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "provider credential operation changed concurrently",
        ));
    }
    let projects_removed_no_effect = status == ProviderCredentialOperationStatus::NoEffect
        && matches!(
            current.plan.operation_kind,
            ProviderCredentialOperationKind::RemoveCredential
                | ProviderCredentialOperationKind::RemoveForArchive
        )
        && operation_attestation_is_missing(transaction, current, attestation_sequence)?;
    let projects_explicit_cleanup_removal =
        explicit_cleanup_projects_removal(transaction, current, status)?;
    if status == ProviderCredentialOperationStatus::Succeeded
        || projects_removed_no_effect
        || projects_explicit_cleanup_removal
    {
        let ownership_state = if projects_explicit_cleanup_removal {
            "removed"
        } else {
            match current.plan.operation_kind {
                ProviderCredentialOperationKind::Install => "ordinary_owned",
                ProviderCredentialOperationKind::RemoveCredential
                | ProviderCredentialOperationKind::RemoveForArchive => "removed",
            }
        };
        let ownership_binding_sha256 = (ownership_state == "ordinary_owned")
            .then_some(current.plan.connection_binding_sha256.as_str());
        let authority_sequence = insert_provider_credential_ownership_event(
            transaction,
            &current.plan.connection_id,
            ownership_state,
            ownership_binding_sha256,
            "ordinary_operation",
            &current.plan.operation_id,
            now,
        )?;
        let changed = transaction
            .execute(
                "UPDATE provider_credential_ownership
                 SET ownership_state = ?2, connection_binding_sha256 = ?3,
                     authority_id = ?4, authority_sequence = ?5, updated_at = ?6
                 WHERE connection_id = ?1 AND credential_ref = ?1",
                params![
                    current.plan.connection_id.as_str(),
                    ownership_state,
                    ownership_binding_sha256,
                    current.plan.operation_id,
                    authority_sequence,
                    now.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(stored_credential_journal_corrupted(
                "provider credential ownership projection is missing",
            ));
        }
    }
    Ok(())
}

fn explicit_cleanup_projects_removal(
    transaction: &Transaction<'_>,
    current: &StoredProviderCredentialOperation,
    status: ProviderCredentialOperationStatus,
) -> CoreResult<bool> {
    let projects_removal = current.status == ProviderCredentialOperationStatus::CleanupRequired
        && validate_provider_credential_cleanup_intents(transaction, current)?.0
        && matches!(
            status,
            ProviderCredentialOperationStatus::Succeeded
                | ProviderCredentialOperationStatus::NoEffect
        );
    if projects_removal {
        require_replacement_cleanup_predecessor_missing(transaction, current)?;
    }
    Ok(projects_removal)
}

fn operation_attestation_is_missing(
    connection: &Connection,
    operation: &StoredProviderCredentialOperation,
    sequence: u64,
) -> CoreResult<bool> {
    connection
        .query_row(
            "SELECT slot_status = 'missing'
             FROM provider_credential_operation_attestations
             WHERE operation_id = ?1 AND sequence = ?2",
            params![operation.plan.operation_id, sequence],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            stored_credential_journal_corrupted(
                "provider credential projection attestation is missing",
            )
        })
}

pub(crate) fn insert_provider_credential_ownership_event(
    transaction: &Transaction<'_>,
    connection_id: &ProviderConnectionId,
    ownership_state: &str,
    binding_sha256: Option<&str>,
    source_kind: &str,
    source_id: &str,
    created_at: DateTime<Utc>,
) -> CoreResult<u64> {
    let authority_sequence = transaction
        .query_row(
            "SELECT COALESCE(MAX(authority_sequence), 0) + 1
             FROM provider_credential_ownership_events
             WHERE connection_id = ?1",
            [connection_id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO provider_credential_ownership_events
             (connection_id, authority_sequence, ownership_state,
              connection_binding_sha256, authority_id, source_kind, source_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?5, ?7)",
            params![
                connection_id.as_str(),
                authority_sequence,
                ownership_state,
                binding_sha256,
                source_id,
                source_kind,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(authority_sequence)
}

fn preflight_evidence_sha256(
    plan: &ProviderCredentialOperationPlan,
    plan_sha256: &str,
    slot_status: ProviderCredentialObservedStatus,
    attested_at: &str,
) -> CoreResult<String> {
    let evidence = ProviderCredentialPreflightEvidence {
        schema_version: PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION,
        redaction_version: PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION,
        operation_id: &plan.operation_id,
        plan_sha256,
        connection_id: plan.connection_id.as_str(),
        credential_ref: &plan.credential_ref,
        connection_binding_sha256: &plan.connection_binding_sha256,
        slot_status,
        native_owner: NATIVE_CREDENTIAL_RECOVERY_OWNER,
        attested_at,
    };
    let encoded = serde_json::to_vec(&evidence)
        .map_err(|_| CoreError::internal("cannot encode provider credential preflight evidence"))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn outcome_evidence_sha256(
    operation: &StoredProviderCredentialOperation,
    sequence: u64,
    stage: &str,
    slot_status: ProviderCredentialObservedStatus,
    attested_at: &str,
) -> CoreResult<String> {
    let evidence = ProviderCredentialOutcomeEvidence {
        schema_version: PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION,
        redaction_version: PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION,
        operation_id: &operation.plan.operation_id,
        plan_sha256: &operation.plan_sha256,
        connection_id: operation.plan.connection_id.as_str(),
        credential_ref: &operation.plan.credential_ref,
        connection_binding_sha256: &operation.plan.connection_binding_sha256,
        sequence,
        stage,
        slot_status,
        native_owner: NATIVE_CREDENTIAL_RECOVERY_OWNER,
        attested_at,
    };
    let encoded = serde_json::to_vec(&evidence)
        .map_err(|_| CoreError::internal("cannot encode provider credential outcome evidence"))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn insert_operation_attestation(
    transaction: &Transaction<'_>,
    operation: &StoredProviderCredentialOperation,
    stage: &str,
    slot_status: ProviderCredentialObservedStatus,
    attested_at: DateTime<Utc>,
) -> CoreResult<u64> {
    if !matches!(
        stage,
        "postflight"
            | "recovery"
            | "cleanup_remove_intent"
            | "cleanup_archive_intent"
            | "durability_repair"
            | "operation_durability_required"
            | "operation_durability_repaired"
            | "predecessor_durability_required"
            | "predecessor_durability_repaired"
            | "predecessor_delete_intent"
            | "predecessor_missing"
    ) {
        return Err(CoreError::invalid(
            "provider credential attestation stage is invalid",
        ));
    }
    let sequence = transaction
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1
             FROM provider_credential_operation_attestations
             WHERE operation_id = ?1",
            [operation.plan.operation_id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .map_err(storage_db_error)?;
    let attested_at = attested_at.to_rfc3339();
    let evidence_sha256 =
        outcome_evidence_sha256(operation, sequence, stage, slot_status, &attested_at)?;
    transaction
        .execute(
            "INSERT INTO provider_credential_operation_attestations
             (operation_id, sequence, stage, slot_status, evidence_sha256,
              native_owner, schema_version, redaction_version, attested_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                operation.plan.operation_id,
                sequence,
                stage,
                slot_status.as_str(),
                evidence_sha256,
                NATIVE_CREDENTIAL_RECOVERY_OWNER,
                PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION,
                PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION,
                attested_at,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(sequence)
}

#[derive(Debug, Default)]
struct ProviderCredentialPredecessorEvidence {
    delete_intent: bool,
    missing: bool,
}

fn validate_provider_credential_predecessor_evidence(
    connection: &Connection,
    operation: &StoredProviderCredentialOperation,
) -> CoreResult<ProviderCredentialPredecessorEvidence> {
    let mut statement = connection
        .prepare(
            "SELECT sequence, stage, slot_status, evidence_sha256, native_owner,
                    schema_version, redaction_version, attested_at
             FROM provider_credential_operation_attestations
             WHERE operation_id = ?1
               AND stage IN ('predecessor_delete_intent', 'predecessor_missing')
             ORDER BY sequence",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([operation.plan.operation_id.as_str()], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, u32>(5)?,
                row.get::<_, u32>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    drop(statement);

    if !rows.is_empty()
        && (operation.plan.operation_kind != ProviderCredentialOperationKind::Install
            || operation.plan.predecessor_authority_id.is_none())
    {
        return Err(stored_credential_journal_corrupted(
            "stored credential predecessor evidence is detached from replacement work",
        ));
    }

    let mut result = ProviderCredentialPredecessorEvidence::default();
    for (sequence, stage, slot_status, evidence, owner, schema, redaction, attested_at) in rows {
        let slot_status = ProviderCredentialObservedStatus::parse(&slot_status)?;
        let valid_authority = owner == NATIVE_CREDENTIAL_RECOVERY_OWNER
            && schema == PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION
            && redaction == PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION;
        let expected =
            outcome_evidence_sha256(operation, sequence, &stage, slot_status, &attested_at)?;
        if !valid_authority || expected != evidence {
            return Err(stored_credential_journal_corrupted(
                "stored credential predecessor evidence is invalid",
            ));
        }
        match stage.as_str() {
            "predecessor_delete_intent" if !result.delete_intent => {
                result.delete_intent = true;
            }
            "predecessor_missing"
                if result.delete_intent
                    && !result.missing
                    && slot_status == ProviderCredentialObservedStatus::Missing =>
            {
                result.missing = true;
            }
            _ => {
                return Err(stored_credential_journal_corrupted(
                    "stored credential predecessor evidence ordering is invalid",
                ));
            }
        }
    }
    if operation.plan.operation_kind == ProviderCredentialOperationKind::Install
        && operation.plan.predecessor_authority_id.is_some()
        && operation.status == ProviderCredentialOperationStatus::Succeeded
        && !result.missing
    {
        return Err(stored_credential_journal_corrupted(
            "successful credential replacement did not delete its predecessor slot",
        ));
    }
    Ok(result)
}

fn apply_provider_credential_durability_ledger(
    connection: &Connection,
    operation: &mut StoredProviderCredentialOperation,
) -> CoreResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT sequence, stage, slot_status, evidence_sha256, native_owner,
                    schema_version, redaction_version, attested_at
             FROM provider_credential_operation_attestations
             WHERE operation_id = ?1
               AND stage IN (
                 'operation_durability_required', 'operation_durability_repaired',
                 'predecessor_durability_required', 'predecessor_durability_repaired'
               )
             ORDER BY sequence",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([operation.plan.operation_id.as_str()], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, u32>(5)?,
                row.get::<_, u32>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    drop(statement);

    for (sequence, stage, slot_status, evidence, owner, schema, redaction, attested_at) in rows {
        let slot_status = ProviderCredentialObservedStatus::parse(&slot_status)?;
        let target = if stage.starts_with("operation_") {
            ProviderCredentialDurabilityTarget::OperationSlot
        } else {
            ProviderCredentialDurabilityTarget::PredecessorSlot
        };
        if target == ProviderCredentialDurabilityTarget::PredecessorSlot
            && (operation.plan.operation_kind != ProviderCredentialOperationKind::Install
                || operation.plan.predecessor_authority_id.is_none())
        {
            return Err(stored_credential_journal_corrupted(
                "stored predecessor durability evidence is detached from replacement work",
            ));
        }
        let required = stage == target.required_stage();
        let valid_status = (required
            && slot_status == ProviderCredentialObservedStatus::Unreadable)
            || (!required && slot_status == ProviderCredentialObservedStatus::Missing);
        let expected =
            outcome_evidence_sha256(operation, sequence, &stage, slot_status, &attested_at)?;
        if owner != NATIVE_CREDENTIAL_RECOVERY_OWNER
            || schema != PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION
            || redaction != PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION
            || !valid_status
            || expected != evidence
            || required == target.is_active(operation)
        {
            return Err(stored_credential_journal_corrupted(
                "stored credential durability evidence is invalid or out of order",
            ));
        }
        match target {
            ProviderCredentialDurabilityTarget::OperationSlot => {
                operation.operation_slot_recovery_required = required;
            }
            ProviderCredentialDurabilityTarget::PredecessorSlot => {
                operation.predecessor_slot_recovery_required = required;
            }
        }
    }
    if operation.status.is_terminal()
        && (operation.operation_slot_recovery_required
            || operation.predecessor_slot_recovery_required)
    {
        return Err(stored_credential_journal_corrupted(
            "terminal credential operation retains a durability obligation",
        ));
    }
    Ok(())
}

fn validate_operation_evidence(
    connection: &Connection,
    operation: &StoredProviderCredentialOperation,
) -> CoreResult<()> {
    let predecessor = validate_provider_credential_predecessor_evidence(connection, operation)?;
    let (has_cleanup_intent, _) =
        validate_provider_credential_cleanup_intents(connection, operation)?;
    if operation.status.is_terminal()
        && has_cleanup_intent
        && operation.plan.operation_kind == ProviderCredentialOperationKind::Install
        && operation.plan.predecessor_authority_id.is_some()
        && (!predecessor.delete_intent || !predecessor.missing)
    {
        return Err(stored_credential_journal_corrupted(
            "terminal replacement cleanup lacks exact predecessor-missing evidence",
        ));
    }
    let expected_preflight = preflight_evidence_sha256(
        &operation.plan,
        &operation.plan_sha256,
        operation.preflight_status,
        &operation.preflight_attested_at.to_rfc3339(),
    )?;
    if expected_preflight != operation.preflight_evidence_sha256 {
        return Err(stored_credential_journal_corrupted(
            "stored credential preflight evidence is invalid",
        ));
    }
    let Some(sequence) = operation.outcome_attestation_sequence else {
        if matches!(
            operation.status,
            ProviderCredentialOperationStatus::Prepared
                | ProviderCredentialOperationStatus::Started
        ) {
            return Ok(());
        }
        return Err(stored_credential_journal_corrupted(
            "stored credential outcome has no native attestation",
        ));
    };
    let row = connection
        .query_row(
            "SELECT stage, slot_status, evidence_sha256, native_owner,
                    schema_version, redaction_version, attested_at
             FROM provider_credential_operation_attestations
             WHERE operation_id = ?1 AND sequence = ?2",
            params![operation.plan.operation_id, sequence],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            stored_credential_journal_corrupted("stored credential outcome attestation is missing")
        })?;
    let (stage, slot_status, evidence_sha256, owner, schema, redaction, attested_at) = row;
    if owner != NATIVE_CREDENTIAL_RECOVERY_OWNER
        || schema != PROVIDER_CREDENTIAL_OPERATION_SCHEMA_VERSION
        || redaction != PROVIDER_CREDENTIAL_OPERATION_REDACTION_VERSION
        || !matches!(
            stage.as_str(),
            "postflight"
                | "recovery"
                | "cleanup_remove_intent"
                | "cleanup_archive_intent"
                | "durability_repair"
        )
    {
        return Err(stored_credential_journal_corrupted(
            "stored credential outcome attestation authority is invalid",
        ));
    }
    let slot_status = ProviderCredentialObservedStatus::parse(&slot_status)?;
    let expected = outcome_evidence_sha256(operation, sequence, &stage, slot_status, &attested_at)?;
    if expected != evidence_sha256 {
        return Err(stored_credential_journal_corrupted(
            "stored credential outcome evidence is invalid",
        ));
    }
    if !provider_credential_outcome_matches_attestation(operation, slot_status) {
        return Err(stored_credential_journal_corrupted(
            "stored credential outcome does not match its native attestation",
        ));
    }
    Ok(())
}

fn require_missing_archive_observation(
    observed_status: ProviderCredentialObservedStatus,
) -> CoreResult<()> {
    if observed_status == ProviderCredentialObservedStatus::Missing {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "provider connection archive requires a missing native credential slot",
        ))
    }
}

fn provider_credential_outcome_matches_attestation(
    operation: &StoredProviderCredentialOperation,
    slot_status: ProviderCredentialObservedStatus,
) -> bool {
    use ProviderCredentialObservedStatus::{Available, Missing, Unreadable};
    use ProviderCredentialOperationKind::{Install, RemoveCredential, RemoveForArchive};
    use ProviderCredentialOperationStatus::{CleanupRequired, NoEffect, OutcomeUnknown, Succeeded};
    use ProviderCredentialOutcomeCode::{
        ArchiveCommitFailed, ConnectionChanged, NativeDurabilityUnknown, NativeEffectAbsent,
        NativeEffectConfirmed, NativePredecessorDurabilityUnknown, NativeStatusUnreadable,
    };

    match (operation.status, operation.outcome_code) {
        (Succeeded, Some(NativeEffectConfirmed)) => match operation.plan.operation_kind {
            Install => {
                operation.started_at.is_some()
                    && operation.preflight_status == Missing
                    && slot_status == Available
            }
            RemoveCredential | RemoveForArchive => {
                operation.started_at.is_some() && slot_status == Missing
            }
        },
        (NoEffect, Some(NativeEffectAbsent)) => match operation.plan.operation_kind {
            Install => {
                slot_status == Missing
                    && !(operation.started_at.is_some()
                        && operation.plan.predecessor_authority_id.is_some())
            }
            RemoveCredential | RemoveForArchive => {
                operation.started_at.is_none()
                    && matches!(operation.preflight_status, Missing | Available)
                    && slot_status == operation.preflight_status
            }
        },
        (NoEffect, Some(ConnectionChanged)) => slot_status == Missing,
        (CleanupRequired, Some(ConnectionChanged)) => {
            matches!(slot_status, Missing | Available)
        }
        (CleanupRequired | OutcomeUnknown, Some(NativeStatusUnreadable))
        | (CleanupRequired, Some(NativeDurabilityUnknown | NativePredecessorDurabilityUnknown)) => {
            slot_status == Unreadable
        }
        (CleanupRequired, Some(ArchiveCommitFailed)) => {
            slot_status == Missing
                && (operation.plan.operation_kind == RemoveForArchive
                    || operation.cleanup_archives_connection)
        }
        (OutcomeUnknown, Some(NativeEffectConfirmed)) => slot_status == Available,
        (OutcomeUnknown, Some(ConnectionChanged)) => {
            slot_status == Available
                || (operation.plan.operation_kind == Install
                    && operation.started_at.is_some()
                    && operation.plan.predecessor_authority_id.is_some()
                    && slot_status == Missing)
        }
        _ => false,
    }
}

fn provider_credential_has_explicit_durability_barrier(
    operation: &StoredProviderCredentialOperation,
) -> bool {
    operation.operation_slot_recovery_required || operation.predecessor_slot_recovery_required
}

fn validate_exact_operation(
    operation: &StoredProviderCredentialOperation,
    plan_sha256: &str,
) -> CoreResult<()> {
    if operation.plan_sha256 != plan_sha256 {
        return Err(CoreError::invalid(
            "provider credential operation plan changed",
        ));
    }
    Ok(())
}

fn validate_record_shape(operation: &StoredProviderCredentialOperation) -> CoreResult<()> {
    let credential_authority_is_paired = operation.plan.credential_authority_id.is_some()
        == operation.plan.credential_authority_binding_sha256.is_some();
    let predecessor_authority_is_paired = operation.plan.predecessor_authority_id.is_some()
        == operation
            .plan
            .predecessor_authority_binding_sha256
            .is_some();
    if !credential_authority_is_paired || !predecessor_authority_is_paired {
        return Err(stored_credential_journal_corrupted(
            "stored credential operation authority binding is incomplete",
        ));
    }
    if operation.plan.operation_kind == ProviderCredentialOperationKind::Install
        && (operation.plan.credential_authority_id.as_deref()
            != Some(operation.plan.operation_id.as_str())
            || operation
                .plan
                .credential_authority_binding_sha256
                .as_deref()
                != Some(operation.plan.connection_binding_sha256.as_str()))
    {
        return Err(stored_credential_journal_corrupted(
            "stored credential installation physical authority is invalid",
        ));
    }
    if operation.updated_at < operation.created_at
        || operation
            .started_at
            .is_some_and(|value| value < operation.created_at)
        || operation
            .finished_at
            .is_some_and(|value| value < operation.created_at)
    {
        return Err(stored_credential_journal_corrupted(
            "stored credential operation timestamps are invalid",
        ));
    }
    match operation.status {
        ProviderCredentialOperationStatus::Prepared
            if operation.started_at.is_none()
                && operation.finished_at.is_none()
                && operation.outcome_code.is_none()
                && operation.outcome_attestation_sequence.is_none() => {}
        ProviderCredentialOperationStatus::Started
            if operation.started_at.is_some()
                && operation.finished_at.is_none()
                && operation.outcome_code.is_none()
                && operation.outcome_attestation_sequence.is_none() => {}
        ProviderCredentialOperationStatus::Succeeded
        | ProviderCredentialOperationStatus::NoEffect
        | ProviderCredentialOperationStatus::CleanupRequired
        | ProviderCredentialOperationStatus::OutcomeUnknown
            if operation.finished_at.is_some()
                && operation.outcome_code.is_some()
                && operation.outcome_attestation_sequence.is_some() => {}
        _ => {
            return Err(stored_credential_journal_corrupted(
                "stored credential operation status shape is invalid",
            ));
        }
    }
    if operation.plan.operation_kind == ProviderCredentialOperationKind::Install
        && operation.preflight_status != ProviderCredentialObservedStatus::Missing
    {
        return Err(stored_credential_journal_corrupted(
            "stored credential installation did not begin from a missing slot",
        ));
    }
    Ok(())
}

fn parse_timestamp(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| {
            stored_credential_journal_corrupted(format!(
                "stored credential operation {label} is invalid"
            ))
        })
}

fn stored_credential_journal_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_terminal_outcome_matrix_is_total_for_every_native_cutpoint() {
        use ProviderCredentialObservedStatus::{Available, Missing, Unreadable};
        use ProviderCredentialOperationKind::{Install, RemoveCredential, RemoveForArchive};
        use ProviderCredentialOperationStatus::{NoEffect, OutcomeUnknown, Succeeded};
        use ProviderCredentialOutcomeCode::{
            ConnectionChanged, NativeEffectAbsent, NativeEffectConfirmed, NativeStatusUnreadable,
        };

        let install_cases = [
            (false, Missing, (NoEffect, NativeEffectAbsent)),
            (false, Available, (OutcomeUnknown, NativeEffectConfirmed)),
            (false, Unreadable, (OutcomeUnknown, NativeStatusUnreadable)),
            (true, Missing, (NoEffect, NativeEffectAbsent)),
            (true, Available, (Succeeded, NativeEffectConfirmed)),
            (true, Unreadable, (OutcomeUnknown, NativeStatusUnreadable)),
        ];
        for (started, observed, expected) in install_cases {
            assert_eq!(
                provider_credential_terminal_outcome(Install, started, false, Missing, observed,),
                expected
            );
        }

        let unstarted_removal_cases = [
            (Missing, Missing, (NoEffect, NativeEffectAbsent)),
            (Missing, Available, (OutcomeUnknown, ConnectionChanged)),
            (
                Missing,
                Unreadable,
                (OutcomeUnknown, NativeStatusUnreadable),
            ),
            (Available, Missing, (NoEffect, ConnectionChanged)),
            (Available, Available, (NoEffect, NativeEffectAbsent)),
            (
                Available,
                Unreadable,
                (OutcomeUnknown, NativeStatusUnreadable),
            ),
            (Unreadable, Missing, (NoEffect, ConnectionChanged)),
            (Unreadable, Available, (OutcomeUnknown, ConnectionChanged)),
            (
                Unreadable,
                Unreadable,
                (OutcomeUnknown, NativeStatusUnreadable),
            ),
        ];
        let started_removal_cases = [
            (Missing, (Succeeded, NativeEffectConfirmed)),
            (Available, (OutcomeUnknown, ConnectionChanged)),
            (Unreadable, (OutcomeUnknown, NativeStatusUnreadable)),
        ];
        for kind in [RemoveCredential, RemoveForArchive] {
            for (preflight, observed, expected) in unstarted_removal_cases {
                assert_eq!(
                    provider_credential_terminal_outcome(kind, false, false, preflight, observed),
                    expected
                );
            }
            for preflight in [Missing, Available, Unreadable] {
                for (observed, expected) in started_removal_cases {
                    assert_eq!(
                        provider_credential_terminal_outcome(
                            kind, true, false, preflight, observed,
                        ),
                        expected
                    );
                }
            }
        }
    }
}

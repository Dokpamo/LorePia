//! Bounded, project-owned synthetic fixtures for downstream adapter tests.
//!
//! This module is compiled only for this crate's tests or when the explicit
//! `test-support` feature is enabled. It deliberately exposes neither Core nor
//! Storage handles and does not accept arbitrary domain documents.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use lorepia_core::{
    ContentModuleRuntimeTarget, ConversationBranchId, ConversationId, Core, CoreConfig,
    CoreErrorCode, DiscoveryRecoveryOwner, MemoryKind, MemoryRecord, MemoryRecordId, MessageId,
    Provenance, ProviderProfile, SourceKind, VersionedJson,
};
use lorepia_storage::Storage;
use rusqlite::Connection;
use tempfile::NamedTempFile;

use crate::{
    ConversationGreetingSelectionInput, ConversationModeDto, CreateConversationInput,
    ListContentModuleLifecycleCandidatesInput, ProviderDiscoveryCredentialInstallContextDto,
    ProviderDiscoveryCredentialLeaseContextDto, ShellApi, ShellError, ShellResult,
    StagedImportFile,
};

const OWNERSHIP_RETRY_WINDOW: Duration = Duration::from_secs(2);
const OWNERSHIP_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const OWNER_LOCK_MESSAGE: &str = "data root is already owned by another LorePia process";
const SYNTHETIC_MEMORY_RECORD_ID: &str = "synthetic.shell-test.memory-record";
const SYNTHETIC_LEGACY_PROVIDER_PROFILE_ID: &str = "synthetic-shell-test-legacy-provider";

/// Stable identifiers for the fixed synthetic memory record fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticMemoryRecordFixture {
    pub conversation_id: String,
    pub branch_id: String,
    pub memory_record_id: String,
}

/// Shell-only handle and non-secret contexts for a Started discovery install.
pub struct SyntheticStartedDiscoveryCredentialInstallFixture {
    pub shell: ShellApi,
    pub install: ProviderDiscoveryCredentialInstallContextDto,
    pub lease: ProviderDiscoveryCredentialLeaseContextDto,
}

/// Shell-only handle and stable session ID for a schema-37 migration-sealed
/// pre-37 Started discovery credential install.
pub struct SyntheticMigratedPre37StartedDiscoveryFixture {
    pub shell: ShellApi,
    pub session_id: String,
}

/// Seeds a fixed network-free discovery commit through the native Started WAL
/// boundary. Downstream Tauri tests receive no Core or Storage handle.
pub fn seed_synthetic_started_discovery_credential_install(
    data_root: impl AsRef<Path>,
) -> ShellResult<SyntheticStartedDiscoveryCredentialInstallFixture> {
    let core = Core::open_with_discovery_recovery_owner(
        CoreConfig::new(data_root.as_ref()),
        DiscoveryRecoveryOwner::NativePlatform,
    )
    .map_err(ShellError::from)?;
    let fixture =
        lorepia_core::provider_discovery_test_support::seed_synthetic_started_credential_install(
            &core,
            "synthetic-shell-direct-capture-connection",
        )
        .map_err(ShellError::from)?;
    Ok(SyntheticStartedDiscoveryCredentialInstallFixture {
        shell: ShellApi::from_core(core),
        install: fixture.install.into(),
        lease: fixture.lease.into(),
    })
}

/// Seeds the exact recovery-only shape produced when migration 37 encounters
/// a pre-37 semantic Started credential WAL with no physical execution ID.
///
/// This bounded helper exists so Tauri adapter tests need neither Core nor
/// Storage development dependencies and cannot obtain either internal handle.
#[allow(clippy::too_many_lines)] // Keeps the exact sealed fixture transaction reviewable in one place.
pub fn seed_synthetic_migrated_pre37_started_discovery(
    data_root: impl AsRef<Path>,
) -> ShellResult<SyntheticMigratedPre37StartedDiscoveryFixture> {
    let data_root = data_root.as_ref();
    let fixture = seed_synthetic_started_discovery_credential_install(data_root)?;
    let operation_id = fixture.install.operation_id.clone();
    let session_id = fixture.install.session_id.clone();
    drop(fixture);

    let database_path = active_database_path(data_root)?;
    let connection = Connection::open(&database_path)
        .map_err(|error| test_fixture_error(format!("open synthetic database: {error}")))?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             BEGIN IMMEDIATE;
             DROP TRIGGER provider_discovery_native_credential_legacy_started_cutoff_no_insert;
             DROP TRIGGER provider_discovery_native_credential_store_attempt_no_delete;
             DROP TRIGGER provider_discovery_native_credential_execution_no_delete;",
        )
        .map_err(|error| test_fixture_error(format!("open legacy cutoff transaction: {error}")))?;
    let migrated = connection
        .execute(
            "INSERT INTO provider_discovery_native_credential_legacy_started_cutoff_snapshots (
                 operation_id, session_id, commit_attempt_id, commit_plan_sha256,
                 connection_id, session_cancellation_pending,
                 session_revision_at_cutoff, session_next_event_sequence_at_cutoff,
                 start_action_id, start_action_kind, request_sha256,
                 operation_expected_revision, start_transition_audit_sequence,
                 commit_prepared_audit_sequence, operation_created_at,
                 operation_started_at, cutoff_before_schema_version,
                 snapshot_schema_version, redaction_version
             )
             SELECT operation.id, operation.session_id, attempt.id,
                    attempt.plan_sha256,
                    json_extract(attempt.plan_json, '$.connection_id'),
                    session.cancellation_pending, session.revision,
                    session.next_event_sequence, receipt.action_id,
                    receipt.action_kind, operation.request_sha256,
                    operation.expected_revision, transition_audit.audit_sequence,
                    commit_audit.audit_sequence, operation.created_at,
                    operation.started_at, 37, 1, 1
             FROM provider_discovery_operations AS operation
             JOIN provider_discovery_sessions AS session
               ON session.id = operation.session_id
             JOIN provider_discovery_action_receipts AS receipt
               ON receipt.session_id = operation.session_id
              AND receipt.action_id = operation.action_id
             JOIN provider_discovery_commit_attempts AS attempt
               ON attempt.id = session.commit_attempt_id
              AND attempt.session_id = session.id
              AND attempt.plan_sha256 = session.commit_plan_sha256
             JOIN provider_discovery_audit_log AS transition_audit
               ON transition_audit.session_id = receipt.session_id
              AND transition_audit.audit_kind = 'transition_applied'
              AND transition_audit.action_id = receipt.action_id
              AND transition_audit.session_revision = receipt.resulting_revision
             JOIN provider_discovery_audit_log AS commit_audit
               ON commit_audit.session_id = receipt.session_id
              AND commit_audit.audit_kind = 'commit_prepared'
              AND commit_audit.action_id = receipt.action_id
              AND commit_audit.subject_id = attempt.id
              AND commit_audit.session_revision = receipt.resulting_revision
             WHERE operation.id = ?1
               AND operation.operation_kind = 'atomic_commit'
               AND operation.side_effect_class = 'persistent'
               AND operation.status = 'started'
               AND operation.finished_at IS NULL
               AND session.state = 'committing'
               AND session.active_operation_id = operation.id",
            [operation_id.as_str()],
        )
        .map_err(|error| test_fixture_error(format!("seal synthetic legacy cutoff: {error}")))?;
    if migrated != 1 {
        return Err(test_fixture_error(
            "synthetic legacy cutoff did not bind exactly one Started operation",
        ));
    }
    connection
        .execute(
            "DELETE FROM provider_discovery_native_credential_store_attempts
             WHERE operation_id = ?1",
            [operation_id.as_str()],
        )
        .and_then(|deleted| {
            (deleted == 1)
                .then_some(())
                .ok_or(rusqlite::Error::ExecuteReturnedResults)
        })
        .map_err(|error| test_fixture_error(format!("remove synthetic store attempt: {error}")))?;
    connection
        .execute(
            "DELETE FROM provider_discovery_native_credential_executions
             WHERE operation_id = ?1",
            [operation_id.as_str()],
        )
        .and_then(|deleted| {
            (deleted == 1)
                .then_some(())
                .ok_or(rusqlite::Error::ExecuteReturnedResults)
        })
        .map_err(|error| test_fixture_error(format!("remove synthetic execution: {error}")))?;
    connection
        .execute_batch(
            "CREATE TRIGGER provider_discovery_native_credential_legacy_started_cutoff_no_insert
             BEFORE INSERT ON provider_discovery_native_credential_legacy_started_cutoff_snapshots
             BEGIN
                 SELECT RAISE(ABORT, 'legacy native Started cutoff is sealed');
             END;
             CREATE TRIGGER provider_discovery_native_credential_store_attempt_no_delete
             BEFORE DELETE ON provider_discovery_native_credential_store_attempts
             BEGIN
                 SELECT RAISE(ABORT, 'native credential store attempts are immutable');
             END;
             CREATE TRIGGER provider_discovery_native_credential_execution_no_delete
             BEFORE DELETE ON provider_discovery_native_credential_executions
             BEGIN
                 SELECT RAISE(ABORT, 'native credential executions are immutable');
             END;
             COMMIT;",
        )
        .map_err(|error| {
            test_fixture_error(format!("seal synthetic migrated database: {error}"))
        })?;
    drop(connection);

    let shell = ShellApi::open_data_root_for_native_discovery_recovery(data_root)?;
    Ok(SyntheticMigratedPre37StartedDiscoveryFixture { shell, session_id })
}

fn active_database_path(data_root: &Path) -> ShellResult<PathBuf> {
    let cutover_root = data_root.join("db/schema-cutover");
    fs::read_dir(&cutover_root)
        .map_err(test_fixture_io_error)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .filter_map(|entry| {
            let manifest = fs::read(entry.path().join("generation-manifest.json")).ok()?;
            let manifest = serde_json::from_slice::<serde_json::Value>(&manifest).ok()?;
            Some((
                manifest.get("activation_sequence")?.as_u64()?,
                manifest
                    .get("active_database_relative_path")?
                    .as_str()?
                    .to_owned(),
            ))
        })
        .max_by_key(|(sequence, _)| *sequence)
        .map(|(_, relative)| data_root.join(relative))
        .ok_or_else(|| test_fixture_error("synthetic fixture has no active database generation"))
}

/// Opens a data root after a prior test owner has been dropped.
///
/// The retry is bounded and recognizes only the exact owner-lock diagnostic.
/// The Core diagnostic is consumed inside this test-only module and does not
/// cross the downstream adapter boundary.
pub fn open_data_root_after_drop(data_root: impl AsRef<Path>) -> ShellResult<ShellApi> {
    let data_root = data_root.as_ref();
    let deadline = Instant::now() + OWNERSHIP_RETRY_WINDOW;
    loop {
        match Core::open(CoreConfig::new(data_root)) {
            Ok(core) => return Ok(ShellApi::from_core(core)),
            Err(error)
                if error.code == CoreErrorCode::StorageUnavailable
                    && error.message == OWNER_LOCK_MESSAGE
                    && Instant::now() < deadline =>
            {
                thread::sleep(OWNERSHIP_RETRY_INTERVAL);
            }
            Err(error) => return Err(ShellError::from(error)),
        }
    }
}

/// Creates one fixed, project-owned synthetic memory record behind Shell API.
///
/// The helper returns only stable identifiers. Downstream adapter tests cannot
/// obtain a Core or Storage handle or customize the stored domain document.
pub fn seed_synthetic_memory_record_fixture(
    data_root: impl AsRef<Path>,
) -> ShellResult<SyntheticMemoryRecordFixture> {
    let data_root = data_root.as_ref();
    let shell = open_data_root_after_drop(data_root)?;
    let mut source = NamedTempFile::new().map_err(test_fixture_io_error)?;
    source
        .write_all(
            br#"{"spec":"chara_card_v3","data":{"name":"Shell Test Memory","description":"Project-owned synthetic Shell adapter fixture","first_mes":"Synthetic Shell adapter greeting"}}"#,
        )
        .map_err(test_fixture_io_error)?;
    let inspection = shell.inspect_import(&StagedImportFile::new(source.path()))?;
    let character = shell.commit_import(&inspection.inspection_id)?;
    let catalog = shell.get_character_greeting_catalog(&character.id)?;
    let greeting_id = catalog
        .greetings
        .first()
        .ok_or_else(|| test_fixture_error("synthetic character has no greeting"))?
        .id
        .clone();
    let conversation = shell.create_conversation(CreateConversationInput {
        character_id: character.id,
        title: "Synthetic Shell adapter memory fixture".to_owned(),
        mode: ConversationModeDto::Chat,
        greeting: Some(ConversationGreetingSelectionInput {
            character_content_revision_id: catalog.character_content_revision_id,
            greeting_id: Some(greeting_id),
        }),
    })?;
    let state = shell.get_conversation_state(&conversation.id)?;
    let source_message_id = shell
        .list_branch_messages(&state.active_branch_id)?
        .first()
        .ok_or_else(|| test_fixture_error("synthetic greeting was not committed"))?
        .id
        .clone();
    drop(shell);

    let record_id = SYNTHETIC_MEMORY_RECORD_ID.to_owned();
    let storage = open_storage_after_drop(data_root)?;
    storage
        .save_memory_record(
            &MemoryRecord {
                id: MemoryRecordId::from(record_id.clone()),
                conversation_id: ConversationId(conversation.id.clone()),
                branch_id: ConversationBranchId(state.active_branch_id.clone()),
                source_start_message_id: MessageId(source_message_id.clone()),
                source_end_message_id: MessageId(source_message_id),
                kind: MemoryKind::CreatorPinned,
                title: "Synthetic initial memory".to_owned(),
                summary: "Synthetic initial memory summary".to_owned(),
                structured_data: VersionedJson {
                    schema_version: 1,
                    value: serde_json::json!({"fixture": "shell-adapter-memory"}),
                },
                importance: 40,
                keywords: vec!["synthetic".to_owned()],
                embedding_ref: None,
                pinned: false,
                excluded_from_conversation: false,
                excluded_from_character: false,
                created_at: conversation.created_at,
                updated_at: conversation.created_at,
                invalidated_at: None,
                provenance: Provenance {
                    source_kind: SourceKind::UserCreated,
                    source_id: Some(record_id.clone()),
                    source_hash: None,
                    author: None,
                    license: None,
                    imported_at: None,
                },
            },
            None,
        )
        .map_err(ShellError::from)?;
    drop(storage);

    Ok(SyntheticMemoryRecordFixture {
        conversation_id: conversation.id,
        branch_id: state.active_branch_id,
        memory_record_id: record_id,
    })
}

/// Builds the one bounded module-candidate request used by adapter tests.
pub fn synthetic_content_module_lifecycle_candidates_input(
    conversation_id: &str,
    branch_id: &str,
) -> ListContentModuleLifecycleCandidatesInput {
    ListContentModuleLifecycleCandidatesInput {
        runtime_target: ContentModuleRuntimeTarget {
            conversation_id: ConversationId(conversation_id.to_owned()),
            branch_id: ConversationBranchId(branch_id.to_owned()),
        },
        limit: 10,
    }
}

/// Seeds the fixed legacy provider profile used by downstream adapter tests.
pub fn seed_synthetic_legacy_provider_profile(data_root: impl AsRef<Path>) -> ShellResult<String> {
    let storage = Storage::open(data_root.as_ref()).map_err(ShellError::from)?;
    storage
        .save_provider_profile(&ProviderProfile {
            id: SYNTHETIC_LEGACY_PROVIDER_PROFILE_ID.to_owned(),
            display_name: "Synthetic Shell test legacy provider".to_owned(),
            base_url: "http://127.0.0.1:9/v1".to_owned(),
            model: "synthetic-shell-test-model".to_owned(),
            timeout_seconds: 30,
        })
        .map_err(ShellError::from)?;
    drop(storage);
    Ok(SYNTHETIC_LEGACY_PROVIDER_PROFILE_ID.to_owned())
}

fn open_storage_after_drop(data_root: &Path) -> ShellResult<Storage> {
    let deadline = Instant::now() + OWNERSHIP_RETRY_WINDOW;
    loop {
        match Storage::open(data_root) {
            Ok(storage) => return Ok(storage),
            Err(error)
                if error.code == CoreErrorCode::StorageUnavailable
                    && error.message == OWNER_LOCK_MESSAGE
                    && Instant::now() < deadline =>
            {
                thread::sleep(OWNERSHIP_RETRY_INTERVAL);
            }
            Err(error) => return Err(ShellError::from(error)),
        }
    }
}

fn test_fixture_io_error(error: std::io::Error) -> ShellError {
    test_fixture_error(format!("synthetic fixture I/O failed: {error}"))
}

fn test_fixture_error(message: impl Into<String>) -> ShellError {
    lorepia_core::CoreError::new(CoreErrorCode::Internal, message, false).into()
}

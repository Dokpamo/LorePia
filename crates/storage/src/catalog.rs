//! Durable, provider-independent storage for signed catalog history.
//!
//! Callers verify signatures and construct catalog snapshots. This module
//! preserves the exact signed bytes, enforces monotonic acceptance with a
//! compare-and-swap, and keeps local rollback separate from the signed
//! anti-rollback guard.

use std::{error::Error, fmt};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Storage;

pub(crate) const SIGNED_CATALOG_HISTORY_MIGRATION: &str =
    include_str!("../migrations/0007_signed_catalog_history.sql");

const MAX_PAGE_SIZE: u32 = 200;
const MAX_ENVELOPE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;
const MAX_PLAN_BYTES: usize = 1024 * 1024;
const ROLLBACK_PLAN_VERSION_WITH_INLINE_DIFF: u64 = 1;
const ROLLBACK_AUDIT_PLAN_VERSION_WITH_DIFF_HASH: u64 = 2;
// Keep these schema-v1 limits aligned with lorepia-providers without adding a
// reverse storage -> providers dependency.
const CATALOG_REVISION_V1_MAX: u64 = (1_u64 << 48) - 1;
const CATALOG_REVISION_MAX_ADVANCE: u64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCatalogRevisionGuard {
    pub highest_accepted_revision: u64,
    pub latest_issued_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCatalogActiveSnapshot {
    pub local_revision: u64,
    pub snapshot_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCatalogState {
    pub state_version: u64,
    pub active: Option<StoredCatalogActiveSnapshot>,
    pub guard: StoredCatalogRevisionGuard,
    pub updated_at: DateTime<Utc>,
    pub snapshot_count: u64,
    pub update_count: u64,
    pub activation_count: u64,
}

impl StoredCatalogState {
    pub fn expectation(&self) -> CatalogStateExpectation {
        CatalogStateExpectation {
            state_version: self.state_version,
            active: self.active.clone(),
            guard: self.guard.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogStateExpectation {
    pub state_version: u64,
    pub active: Option<StoredCatalogActiveSnapshot>,
    pub guard: StoredCatalogRevisionGuard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSignedCatalogUpdate {
    pub id: String,
    pub catalog_id: String,
    pub catalog_schema_version: u32,
    pub catalog_revision: u64,
    pub envelope_version: u32,
    pub signing_key_id: String,
    pub envelope: Vec<u8>,
    pub envelope_sha256: String,
    pub payload_json: String,
    pub payload_sha256: String,
    pub issued_at: DateTime<Utc>,
    pub effective_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: DateTime<Utc>,
    pub accepted_guard: StoredCatalogRevisionGuard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogSnapshotSource {
    BundledBaseline,
    SignedImport {
        envelope_id: String,
        catalog_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCatalogSnapshot {
    pub local_revision: u64,
    pub snapshot_schema_version: u32,
    pub snapshot_json: String,
    pub snapshot_sha256: String,
    pub bundled_revision: u64,
    pub bundled_sha256: String,
    pub signed_revision_chain: Vec<u64>,
    pub source: CatalogSnapshotSource,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogActivationKind {
    Import,
    Rollback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogActivationRecord {
    pub action_id: String,
    pub state_version: u64,
    pub kind: CatalogActivationKind,
    pub from: Option<StoredCatalogActiveSnapshot>,
    pub to: StoredCatalogActiveSnapshot,
    pub source_envelope_id: Option<String>,
    pub signed_catalog_revision: Option<u64>,
    pub signing_key_id: Option<String>,
    pub diff_json: String,
    pub diff_sha256: String,
    pub rollback_plan_json: Option<String>,
    pub plan_sha256: Option<String>,
    pub activated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSignedCatalogUpdate<'a> {
    pub id: &'a str,
    pub catalog_id: &'a str,
    pub catalog_schema_version: u32,
    pub catalog_revision: u64,
    pub envelope_version: u32,
    pub signing_key_id: &'a str,
    pub envelope: &'a [u8],
    pub envelope_sha256: &'a str,
    pub payload_json: &'a str,
    pub payload_sha256: &'a str,
    pub issued_at: DateTime<Utc>,
    pub effective_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCatalogSnapshot<'a> {
    pub local_revision: u64,
    pub snapshot_schema_version: u32,
    pub snapshot_json: &'a str,
    pub snapshot_sha256: &'a str,
    pub bundled_revision: u64,
    pub bundled_sha256: &'a str,
    pub signed_revision_chain: &'a [u64],
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogImportCommit<'a> {
    pub expected: CatalogStateExpectation,
    pub action_id: &'a str,
    pub update: NewSignedCatalogUpdate<'a>,
    pub snapshot: NewCatalogSnapshot<'a>,
    pub initial_baseline: Option<NewCatalogSnapshot<'a>>,
    pub next_guard: StoredCatalogRevisionGuard,
    pub diff_json: &'a str,
    pub activated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRollbackCommit<'a> {
    pub expected: CatalogStateExpectation,
    pub action_id: &'a str,
    pub target_local_revision: u64,
    pub target_snapshot_sha256: &'a str,
    pub diff_json: &'a str,
    pub rollback_plan_json: &'a str,
    pub plan_sha256: &'a str,
    pub activated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub enum CatalogStorageError {
    Database(rusqlite::Error),
    InvalidInput(&'static str),
    Corrupted(&'static str),
    StateConflict,
    NotFound(&'static str),
    StorageUnavailable,
}

impl fmt::Display for CatalogStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "catalog database error: {error}"),
            Self::InvalidInput(reason) => {
                write!(formatter, "invalid catalog storage input: {reason}")
            }
            Self::Corrupted(reason) => {
                write!(formatter, "catalog storage is inconsistent: {reason}")
            }
            Self::StateConflict => formatter.write_str("catalog state changed"),
            Self::NotFound(item) => write!(formatter, "catalog {item} was not found"),
            Self::StorageUnavailable => formatter.write_str("catalog storage lock is unavailable"),
        }
    }
}

impl Error for CatalogStorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for CatalogStorageError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value)
    }
}

impl Storage {
    pub fn catalog_state(&self) -> Result<StoredCatalogState, CatalogStorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| CatalogStorageError::StorageUnavailable)?;
        load_catalog_state(&connection)
    }

    pub fn catalog_updates(
        &self,
        limit: u32,
        before_revision: Option<u64>,
    ) -> Result<Vec<StoredSignedCatalogUpdate>, CatalogStorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| CatalogStorageError::StorageUnavailable)?;
        load_catalog_updates(&connection, limit, before_revision)
    }

    pub fn catalog_update(
        &self,
        envelope_id: &str,
    ) -> Result<Option<StoredSignedCatalogUpdate>, CatalogStorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| CatalogStorageError::StorageUnavailable)?;
        load_catalog_update(&connection, envelope_id)
    }

    pub fn catalog_update_by_revision(
        &self,
        catalog_revision: u64,
    ) -> Result<Option<StoredSignedCatalogUpdate>, CatalogStorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| CatalogStorageError::StorageUnavailable)?;
        load_catalog_update_by_revision(&connection, catalog_revision)
    }

    pub fn catalog_snapshots(
        &self,
        limit: u32,
        before_local_revision: Option<u64>,
    ) -> Result<Vec<StoredCatalogSnapshot>, CatalogStorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| CatalogStorageError::StorageUnavailable)?;
        load_catalog_snapshots(&connection, limit, before_local_revision)
    }

    pub fn catalog_snapshot(
        &self,
        local_revision: u64,
    ) -> Result<Option<StoredCatalogSnapshot>, CatalogStorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| CatalogStorageError::StorageUnavailable)?;
        load_catalog_snapshot(&connection, local_revision)
    }

    pub fn catalog_activations(
        &self,
        limit: u32,
        before_state_version: Option<u64>,
    ) -> Result<Vec<CatalogActivationRecord>, CatalogStorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| CatalogStorageError::StorageUnavailable)?;
        load_catalog_activations(&connection, limit, before_state_version)
    }

    pub fn commit_catalog_import(
        &self,
        commit: &CatalogImportCommit<'_>,
    ) -> Result<StoredCatalogState, CatalogStorageError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| CatalogStorageError::StorageUnavailable)?;
        commit_catalog_import(&mut connection, commit)
    }

    pub fn activate_catalog_rollback(
        &self,
        commit: &CatalogRollbackCommit<'_>,
    ) -> Result<StoredCatalogState, CatalogStorageError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| CatalogStorageError::StorageUnavailable)?;
        activate_catalog_rollback(&mut connection, commit)
    }
}

pub(crate) fn load_catalog_state(
    connection: &Connection,
) -> Result<StoredCatalogState, CatalogStorageError> {
    let raw = connection.query_row(
        "SELECT state_version, active_local_revision, active_snapshot_sha256,
                highest_accepted_revision, latest_issued_at, updated_at
         FROM provider_catalog_state
         WHERE singleton = 1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        },
    )?;
    let active = match (raw.1, raw.2) {
        (None, None) => None,
        (Some(revision), Some(hash)) => Some(StoredCatalogActiveSnapshot {
            local_revision: from_i64(revision, "negative active local revision")?,
            snapshot_sha256: checked_stored_sha(hash)?,
        }),
        _ => {
            return Err(CatalogStorageError::Corrupted(
                "partial active snapshot pointer",
            ));
        }
    };
    let highest = from_i64(raw.3, "negative accepted revision")?;
    let latest = raw.4.as_deref().map(parse_stored_time).transpose()?;
    if (highest == 0) != latest.is_none() {
        return Err(CatalogStorageError::Corrupted("partial revision guard"));
    }
    Ok(StoredCatalogState {
        state_version: from_i64(raw.0, "negative state version")?,
        active,
        guard: StoredCatalogRevisionGuard {
            highest_accepted_revision: highest,
            latest_issued_at: latest,
        },
        updated_at: parse_stored_time(&raw.5)?,
        snapshot_count: count_rows(connection, "provider_catalog_snapshots")?,
        update_count: count_rows(connection, "provider_catalog_signed_envelopes")?,
        activation_count: count_rows(connection, "provider_catalog_activation_audit")?,
    })
}

pub(crate) fn load_catalog_updates(
    connection: &Connection,
    limit: u32,
    before_revision: Option<u64>,
) -> Result<Vec<StoredSignedCatalogUpdate>, CatalogStorageError> {
    let (limit, before) = page_parameters(limit, before_revision)?;
    let mut statement = connection.prepare(
        "SELECT id, catalog_id, catalog_schema_version, catalog_revision,
                envelope_version, signing_key_id, envelope_bytes,
                envelope_sha256, payload_json, payload_sha256, issued_at,
                effective_at, expires_at, accepted_at
         FROM provider_catalog_signed_envelopes
         WHERE (?1 IS NULL OR catalog_revision < ?1)
         ORDER BY catalog_revision DESC
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![before, limit], RawSignedUpdate::from_row)?;
    rows.map(|row| {
        row.map_err(Into::into)
            .and_then(StoredSignedCatalogUpdate::try_from)
    })
    .collect()
}

pub(crate) fn load_catalog_update(
    connection: &Connection,
    envelope_id: &str,
) -> Result<Option<StoredSignedCatalogUpdate>, CatalogStorageError> {
    load_update_where(connection, "id = ?1", envelope_id)
}

pub(crate) fn load_catalog_update_by_revision(
    connection: &Connection,
    catalog_revision: u64,
) -> Result<Option<StoredSignedCatalogUpdate>, CatalogStorageError> {
    load_update_where(
        connection,
        "catalog_revision = ?1",
        to_i64(catalog_revision, "catalog revision is too large")?,
    )
}

pub(crate) fn load_catalog_snapshots(
    connection: &Connection,
    limit: u32,
    before_local_revision: Option<u64>,
) -> Result<Vec<StoredCatalogSnapshot>, CatalogStorageError> {
    let (limit, before) = page_parameters(limit, before_local_revision)?;
    let mut statement = connection.prepare(
        "SELECT local_revision, snapshot_schema_version, snapshot_json,
                snapshot_sha256, bundled_revision, bundled_sha256,
                signed_revision_chain_json, source_kind,
                source_envelope_id, catalog_revision, captured_at
         FROM provider_catalog_snapshots
         WHERE (?1 IS NULL OR local_revision < ?1)
         ORDER BY local_revision DESC
         LIMIT ?2",
    )?;
    let raw = statement
        .query_map(params![before, limit], RawSnapshot::from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(|snapshot| stored_snapshot(connection, snapshot))
        .collect()
}

pub(crate) fn load_catalog_snapshot(
    connection: &Connection,
    local_revision: u64,
) -> Result<Option<StoredCatalogSnapshot>, CatalogStorageError> {
    let raw = connection
        .query_row(
            "SELECT local_revision, snapshot_schema_version, snapshot_json,
                    snapshot_sha256, bundled_revision, bundled_sha256,
                    signed_revision_chain_json, source_kind,
                    source_envelope_id, catalog_revision, captured_at
             FROM provider_catalog_snapshots
             WHERE local_revision = ?1",
            [to_i64(local_revision, "local revision is too large")?],
            RawSnapshot::from_row,
        )
        .optional()?;
    raw.map(|snapshot| stored_snapshot(connection, snapshot))
        .transpose()
}

pub(crate) fn load_catalog_activations(
    connection: &Connection,
    limit: u32,
    before_state_version: Option<u64>,
) -> Result<Vec<CatalogActivationRecord>, CatalogStorageError> {
    let (limit, before) = page_parameters(limit, before_state_version)?;
    let mut statement = connection.prepare(
        "SELECT action_id, state_version, activation_kind,
                from_local_revision, from_snapshot_sha256,
                to_local_revision, to_snapshot_sha256, source_envelope_id,
                signed_catalog_revision, signing_key_id, diff_json,
                diff_sha256, rollback_plan_json, plan_sha256, activated_at
         FROM provider_catalog_activation_audit
         WHERE (?1 IS NULL OR state_version < ?1)
         ORDER BY state_version DESC
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![before, limit], RawActivation::from_row)?;
    rows.map(|row| {
        row.map_err(Into::into)
            .and_then(CatalogActivationRecord::try_from)
    })
    .collect()
}

#[derive(Debug)]
struct RawSignedUpdate {
    id: String,
    catalog_id: String,
    catalog_schema_version: i64,
    catalog_revision: i64,
    envelope_version: i64,
    signing_key_id: String,
    envelope: Vec<u8>,
    envelope_sha256: String,
    payload_json: String,
    payload_sha256: String,
    issued_at: String,
    effective_at: String,
    expires_at: String,
    accepted_at: String,
}

impl RawSignedUpdate {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            catalog_id: row.get(1)?,
            catalog_schema_version: row.get(2)?,
            catalog_revision: row.get(3)?,
            envelope_version: row.get(4)?,
            signing_key_id: row.get(5)?,
            envelope: row.get(6)?,
            envelope_sha256: row.get(7)?,
            payload_json: row.get(8)?,
            payload_sha256: row.get(9)?,
            issued_at: row.get(10)?,
            effective_at: row.get(11)?,
            expires_at: row.get(12)?,
            accepted_at: row.get(13)?,
        })
    }
}

impl TryFrom<RawSignedUpdate> for StoredSignedCatalogUpdate {
    type Error = CatalogStorageError;

    fn try_from(raw: RawSignedUpdate) -> Result<Self, Self::Error> {
        let issued_at = parse_stored_time(&raw.issued_at)?;
        let catalog_revision = from_i64(raw.catalog_revision, "negative signed catalog revision")?;
        if sha256_hex(&raw.envelope) != raw.envelope_sha256
            || sha256_hex(raw.payload_json.as_bytes()) != raw.payload_sha256
        {
            return Err(CatalogStorageError::Corrupted(
                "stored signed catalog hash mismatch",
            ));
        }
        Ok(Self {
            id: raw.id,
            catalog_id: raw.catalog_id,
            catalog_schema_version: from_i64_u32(
                raw.catalog_schema_version,
                "invalid catalog schema version",
            )?,
            catalog_revision,
            envelope_version: from_i64_u32(raw.envelope_version, "invalid envelope version")?,
            signing_key_id: raw.signing_key_id,
            envelope: raw.envelope,
            envelope_sha256: checked_stored_sha(raw.envelope_sha256)?,
            payload_json: raw.payload_json,
            payload_sha256: checked_stored_sha(raw.payload_sha256)?,
            issued_at,
            effective_at: parse_stored_time(&raw.effective_at)?,
            expires_at: parse_stored_time(&raw.expires_at)?,
            accepted_at: parse_stored_time(&raw.accepted_at)?,
            accepted_guard: StoredCatalogRevisionGuard {
                highest_accepted_revision: catalog_revision,
                latest_issued_at: Some(issued_at),
            },
        })
    }
}

#[derive(Debug)]
struct RawSnapshot {
    local_revision: i64,
    snapshot_schema_version: i64,
    snapshot_json: String,
    snapshot_sha256: String,
    bundled_revision: i64,
    bundled_sha256: String,
    signed_revision_chain_json: String,
    source_kind: String,
    source_envelope_id: Option<String>,
    catalog_revision: Option<i64>,
    captured_at: String,
}

impl RawSnapshot {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            local_revision: row.get(0)?,
            snapshot_schema_version: row.get(1)?,
            snapshot_json: row.get(2)?,
            snapshot_sha256: row.get(3)?,
            bundled_revision: row.get(4)?,
            bundled_sha256: row.get(5)?,
            signed_revision_chain_json: row.get(6)?,
            source_kind: row.get(7)?,
            source_envelope_id: row.get(8)?,
            catalog_revision: row.get(9)?,
            captured_at: row.get(10)?,
        })
    }
}

#[derive(Debug)]
struct RawChainLink {
    ordinal: i64,
    envelope_id: String,
    catalog_revision: i64,
    payload_sha256: String,
}

fn stored_snapshot(
    connection: &Connection,
    raw: RawSnapshot,
) -> Result<StoredCatalogSnapshot, CatalogStorageError> {
    let local_revision = from_i64(raw.local_revision, "negative local revision")?;
    let json_chain: Vec<u64> = serde_json::from_str(&raw.signed_revision_chain_json)
        .map_err(|_| CatalogStorageError::Corrupted("invalid signed revision chain JSON"))?;
    validate_revision_chain(&json_chain)
        .map_err(|_| CatalogStorageError::Corrupted("invalid signed revision chain"))?;
    let links = load_chain_links(connection, local_revision)?;
    let relational_chain = links
        .iter()
        .map(|link| from_i64(link.catalog_revision, "negative chain revision"))
        .collect::<Result<Vec<_>, _>>()?;
    if json_chain != relational_chain {
        return Err(CatalogStorageError::Corrupted(
            "snapshot chain representations differ",
        ));
    }
    for (expected, link) in links.iter().enumerate() {
        if usize::try_from(link.ordinal).ok() != Some(expected) || !is_sha256(&link.payload_sha256)
        {
            return Err(CatalogStorageError::Corrupted(
                "invalid snapshot chain link",
            ));
        }
    }
    let source = match (
        raw.source_kind.as_str(),
        raw.source_envelope_id,
        raw.catalog_revision,
    ) {
        ("bundled_baseline", None, None) if links.is_empty() => {
            CatalogSnapshotSource::BundledBaseline
        }
        ("signed_import", Some(envelope_id), Some(revision)) => {
            let revision = from_i64(revision, "negative source catalog revision")?;
            let Some(last) = links.last() else {
                return Err(CatalogStorageError::Corrupted(
                    "signed snapshot has an empty chain",
                ));
            };
            if last.envelope_id != envelope_id
                || from_i64(last.catalog_revision, "negative chain revision")? != revision
            {
                return Err(CatalogStorageError::Corrupted(
                    "snapshot source is not its final chain link",
                ));
            }
            CatalogSnapshotSource::SignedImport {
                envelope_id,
                catalog_revision: revision,
            }
        }
        _ => return Err(CatalogStorageError::Corrupted("invalid snapshot source")),
    };
    if sha256_hex(raw.snapshot_json.as_bytes()) != raw.snapshot_sha256 {
        return Err(CatalogStorageError::Corrupted(
            "stored snapshot hash mismatch",
        ));
    }
    Ok(StoredCatalogSnapshot {
        local_revision,
        snapshot_schema_version: from_i64_u32(
            raw.snapshot_schema_version,
            "invalid snapshot schema version",
        )?,
        snapshot_json: raw.snapshot_json,
        snapshot_sha256: checked_stored_sha(raw.snapshot_sha256)?,
        bundled_revision: from_i64(raw.bundled_revision, "invalid bundled revision")?,
        bundled_sha256: checked_stored_sha(raw.bundled_sha256)?,
        signed_revision_chain: json_chain,
        source,
        captured_at: parse_stored_time(&raw.captured_at)?,
    })
}

#[derive(Debug)]
struct RawActivation {
    action_id: String,
    state_version: i64,
    kind: String,
    from_local_revision: Option<i64>,
    from_snapshot_sha256: Option<String>,
    to_local_revision: i64,
    to_snapshot_sha256: String,
    source_envelope_id: Option<String>,
    signed_catalog_revision: Option<i64>,
    signing_key_id: Option<String>,
    diff_json: String,
    diff_sha256: String,
    rollback_plan_json: Option<String>,
    plan_sha256: Option<String>,
    activated_at: String,
}

impl RawActivation {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            action_id: row.get(0)?,
            state_version: row.get(1)?,
            kind: row.get(2)?,
            from_local_revision: row.get(3)?,
            from_snapshot_sha256: row.get(4)?,
            to_local_revision: row.get(5)?,
            to_snapshot_sha256: row.get(6)?,
            source_envelope_id: row.get(7)?,
            signed_catalog_revision: row.get(8)?,
            signing_key_id: row.get(9)?,
            diff_json: row.get(10)?,
            diff_sha256: row.get(11)?,
            rollback_plan_json: row.get(12)?,
            plan_sha256: row.get(13)?,
            activated_at: row.get(14)?,
        })
    }
}

impl TryFrom<RawActivation> for CatalogActivationRecord {
    type Error = CatalogStorageError;

    fn try_from(raw: RawActivation) -> Result<Self, Self::Error> {
        let from = match (raw.from_local_revision, raw.from_snapshot_sha256) {
            (None, None) => None,
            (Some(revision), Some(hash)) => Some(StoredCatalogActiveSnapshot {
                local_revision: from_i64(revision, "negative audit source revision")?,
                snapshot_sha256: checked_stored_sha(hash)?,
            }),
            _ => return Err(CatalogStorageError::Corrupted("partial audit source")),
        };
        let kind = match raw.kind.as_str() {
            "import" => CatalogActivationKind::Import,
            "rollback" => CatalogActivationKind::Rollback,
            _ => return Err(CatalogStorageError::Corrupted("invalid activation kind")),
        };
        if sha256_hex(raw.diff_json.as_bytes()) != raw.diff_sha256 {
            return Err(CatalogStorageError::Corrupted(
                "activation diff hash mismatch",
            ));
        }
        if let (Some(plan), Some(hash)) = (&raw.rollback_plan_json, &raw.plan_sha256)
            && sha256_hex(plan.as_bytes()) != *hash
        {
            return Err(CatalogStorageError::Corrupted(
                "rollback plan hash mismatch",
            ));
        }
        Ok(Self {
            action_id: raw.action_id,
            state_version: from_i64(raw.state_version, "negative audit state version")?,
            kind,
            from,
            to: StoredCatalogActiveSnapshot {
                local_revision: from_i64(raw.to_local_revision, "negative audit target revision")?,
                snapshot_sha256: checked_stored_sha(raw.to_snapshot_sha256)?,
            },
            source_envelope_id: raw.source_envelope_id,
            signed_catalog_revision: raw
                .signed_catalog_revision
                .map(|value| from_i64(value, "negative audit catalog revision"))
                .transpose()?,
            signing_key_id: raw.signing_key_id,
            diff_json: raw.diff_json,
            diff_sha256: checked_stored_sha(raw.diff_sha256)?,
            rollback_plan_json: raw.rollback_plan_json,
            plan_sha256: raw.plan_sha256.map(checked_stored_sha).transpose()?,
            activated_at: parse_stored_time(&raw.activated_at)?,
        })
    }
}

fn load_update_where<T: rusqlite::ToSql>(
    connection: &Connection,
    predicate: &str,
    parameter: T,
) -> Result<Option<StoredSignedCatalogUpdate>, CatalogStorageError> {
    let sql = format!(
        "SELECT id, catalog_id, catalog_schema_version, catalog_revision,
                envelope_version, signing_key_id, envelope_bytes,
                envelope_sha256, payload_json, payload_sha256, issued_at,
                effective_at, expires_at, accepted_at
         FROM provider_catalog_signed_envelopes
         WHERE {predicate}"
    );
    connection
        .query_row(&sql, [&parameter], RawSignedUpdate::from_row)
        .optional()?
        .map(StoredSignedCatalogUpdate::try_from)
        .transpose()
}

fn load_chain_links(
    connection: &Connection,
    local_revision: u64,
) -> Result<Vec<RawChainLink>, CatalogStorageError> {
    let mut statement = connection.prepare(
        "SELECT ordinal, envelope_id, catalog_revision, payload_sha256
         FROM provider_catalog_snapshot_envelopes
         WHERE local_revision = ?1
         ORDER BY ordinal",
    )?;
    statement
        .query_map(
            [to_i64(local_revision, "local revision is too large")?],
            |row| {
                Ok(RawChainLink {
                    ordinal: row.get(0)?,
                    envelope_id: row.get(1)?,
                    catalog_revision: row.get(2)?,
                    payload_sha256: row.get(3)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn page_parameters(
    limit: u32,
    before: Option<u64>,
) -> Result<(i64, Option<i64>), CatalogStorageError> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(CatalogStorageError::InvalidInput(
            "page size must be between 1 and 200",
        ));
    }
    Ok((
        i64::from(limit),
        before
            .map(|value| to_i64(value, "page cursor is too large"))
            .transpose()?,
    ))
}

fn count_rows(connection: &Connection, table: &str) -> Result<u64, CatalogStorageError> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let count = connection.query_row(&sql, [], |row| row.get::<_, i64>(0))?;
    from_i64(count, "negative catalog row count")
}

pub(crate) fn commit_catalog_import(
    connection: &mut Connection,
    commit: &CatalogImportCommit<'_>,
) -> Result<StoredCatalogState, CatalogStorageError> {
    validate_action_id(commit.action_id)?;
    validate_signed_update(&commit.update)?;
    validate_snapshot(&commit.snapshot, false)?;
    validate_import_guard(commit)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if commit.update.expires_at <= Utc::now() {
        return Err(CatalogStorageError::InvalidInput(
            "catalog payload is expired",
        ));
    }
    let current = load_catalog_state(&transaction)?;
    require_expected_state(&current, &commit.expected)?;
    let maximum_local_revision = maximum_local_revision(&transaction)?;
    let base = prepare_import_base(
        &transaction,
        &current,
        commit.initial_baseline.as_ref(),
        maximum_local_revision,
    )?;
    validate_import_snapshot(commit, base.as_ref(), maximum_local_revision)?;
    let from_revision = base.as_ref().map_or(0, |value| value.active.local_revision);
    validate_diff_json(
        commit.diff_json,
        from_revision,
        commit.snapshot.local_revision,
    )?;

    insert_signed_update(&transaction, &commit.update)?;
    if let Some(baseline) = &commit.initial_baseline {
        insert_snapshot(
            &transaction,
            baseline,
            &CatalogSnapshotSource::BundledBaseline,
        )?;
    }
    let source = CatalogSnapshotSource::SignedImport {
        envelope_id: commit.update.id.to_owned(),
        catalog_revision: commit.update.catalog_revision,
    };
    insert_snapshot(&transaction, &commit.snapshot, &source)?;
    insert_snapshot_chain(
        &transaction,
        commit.snapshot.local_revision,
        commit.snapshot.signed_revision_chain,
    )?;

    let next_version = commit
        .expected
        .state_version
        .checked_add(1)
        .ok_or(CatalogStorageError::InvalidInput("state version overflow"))?;
    update_state_for_import(&transaction, commit, next_version)?;
    insert_import_audit(
        &transaction,
        commit,
        next_version,
        base.as_ref().map(|value| &value.active),
    )?;
    let state = load_catalog_state(&transaction)?;
    transaction.commit()?;
    Ok(state)
}

pub(crate) fn activate_catalog_rollback(
    connection: &mut Connection,
    commit: &CatalogRollbackCommit<'_>,
) -> Result<StoredCatalogState, CatalogStorageError> {
    validate_action_id(commit.action_id)?;
    validate_sha(commit.target_snapshot_sha256)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_catalog_state(&transaction)?;
    require_expected_state(&current, &commit.expected)?;
    let from = current
        .active
        .as_ref()
        .ok_or(CatalogStorageError::StateConflict)?;
    if from.local_revision == commit.target_local_revision {
        return Err(CatalogStorageError::InvalidInput(
            "rollback target is already active",
        ));
    }
    let target = load_catalog_snapshot(&transaction, commit.target_local_revision)?
        .ok_or(CatalogStorageError::NotFound("rollback snapshot"))?;
    if target.snapshot_sha256 != commit.target_snapshot_sha256 {
        return Err(CatalogStorageError::StateConflict);
    }
    validate_diff_json(commit.diff_json, from.local_revision, target.local_revision)?;
    validate_rollback_plan(commit, from, &target)?;
    let next_version = commit
        .expected
        .state_version
        .checked_add(1)
        .ok_or(CatalogStorageError::InvalidInput("state version overflow"))?;
    update_state_for_rollback(&transaction, commit, next_version)?;
    insert_rollback_audit(&transaction, commit, next_version, from, &target)?;
    let state = load_catalog_state(&transaction)?;
    if state.guard != current.guard {
        return Err(CatalogStorageError::Corrupted(
            "rollback changed the signed revision guard",
        ));
    }
    transaction.commit()?;
    Ok(state)
}

#[derive(Debug)]
struct ImportBase {
    active: StoredCatalogActiveSnapshot,
    signed_revision_chain: Vec<u64>,
    bundled_revision: u64,
    bundled_sha256: String,
}

fn prepare_import_base(
    transaction: &Transaction<'_>,
    current: &StoredCatalogState,
    baseline: Option<&NewCatalogSnapshot<'_>>,
    maximum_local_revision: u64,
) -> Result<Option<ImportBase>, CatalogStorageError> {
    match (&current.active, baseline) {
        (Some(_), Some(_)) => Err(CatalogStorageError::InvalidInput(
            "initial baseline is only valid for empty history",
        )),
        (Some(active), None) => {
            let snapshot = load_catalog_snapshot(transaction, active.local_revision)?
                .ok_or(CatalogStorageError::Corrupted("active snapshot is missing"))?;
            if snapshot.snapshot_sha256 != active.snapshot_sha256 {
                return Err(CatalogStorageError::Corrupted(
                    "active snapshot hash does not match",
                ));
            }
            Ok(Some(ImportBase {
                active: active.clone(),
                signed_revision_chain: snapshot.signed_revision_chain,
                bundled_revision: snapshot.bundled_revision,
                bundled_sha256: snapshot.bundled_sha256,
            }))
        }
        (None, Some(baseline)) => {
            if current.snapshot_count != 0 || maximum_local_revision != 0 {
                return Err(CatalogStorageError::Corrupted(
                    "inactive catalog has existing snapshots",
                ));
            }
            validate_snapshot(baseline, true)?;
            Ok(Some(ImportBase {
                active: StoredCatalogActiveSnapshot {
                    local_revision: baseline.local_revision,
                    snapshot_sha256: baseline.snapshot_sha256.to_owned(),
                },
                signed_revision_chain: Vec::new(),
                bundled_revision: baseline.bundled_revision,
                bundled_sha256: baseline.bundled_sha256.to_owned(),
            }))
        }
        (None, None) if current.snapshot_count == 0 && maximum_local_revision == 0 => Ok(None),
        (None, None) => Err(CatalogStorageError::Corrupted(
            "inactive catalog has existing snapshots",
        )),
    }
}

fn validate_import_guard(commit: &CatalogImportCommit<'_>) -> Result<(), CatalogStorageError> {
    if commit.update.catalog_revision <= commit.expected.guard.highest_accepted_revision
        || commit.next_guard.highest_accepted_revision != commit.update.catalog_revision
        || commit.next_guard.latest_issued_at != Some(commit.update.issued_at)
        || commit
            .expected
            .guard
            .latest_issued_at
            .is_some_and(|previous| commit.update.issued_at < previous)
    {
        return Err(CatalogStorageError::InvalidInput(
            "signed revision guard does not advance exactly once",
        ));
    }
    if commit.update.catalog_revision > CATALOG_REVISION_V1_MAX
        || commit.expected.guard.highest_accepted_revision > CATALOG_REVISION_V1_MAX
        || commit.update.catalog_revision - commit.expected.guard.highest_accepted_revision
            > CATALOG_REVISION_MAX_ADVANCE
    {
        return Err(CatalogStorageError::InvalidInput(
            "signed revision exceeds the supported catalog epoch or advance bound",
        ));
    }
    Ok(())
}

fn validate_import_snapshot(
    commit: &CatalogImportCommit<'_>,
    base: Option<&ImportBase>,
    maximum_local_revision: u64,
) -> Result<(), CatalogStorageError> {
    let baseline_revision = commit
        .initial_baseline
        .as_ref()
        .map_or(maximum_local_revision, |value| value.local_revision);
    if commit.snapshot.local_revision <= baseline_revision {
        return Err(CatalogStorageError::InvalidInput(
            "new local revision must exceed all history",
        ));
    }
    let mut expected_chain = base
        .map(|value| value.signed_revision_chain.clone())
        .unwrap_or_default();
    expected_chain.push(commit.update.catalog_revision);
    if commit.snapshot.signed_revision_chain != expected_chain {
        return Err(CatalogStorageError::InvalidInput(
            "snapshot chain must extend the active chain with only the new revision",
        ));
    }
    if let Some(base) = base
        && (commit.snapshot.bundled_revision != base.bundled_revision
            || commit.snapshot.bundled_sha256 != base.bundled_sha256)
    {
        return Err(CatalogStorageError::InvalidInput(
            "snapshot bundled baseline changed",
        ));
    }
    Ok(())
}

fn insert_signed_update(
    transaction: &Transaction<'_>,
    update: &NewSignedCatalogUpdate<'_>,
) -> Result<(), CatalogStorageError> {
    transaction.execute(
        "INSERT INTO provider_catalog_signed_envelopes (
             id, catalog_id, catalog_schema_version, catalog_revision,
             envelope_version, signing_key_id, envelope_bytes,
             envelope_sha256, payload_json, payload_sha256, issued_at,
             effective_at, expires_at, accepted_at
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
         )",
        params![
            update.id,
            update.catalog_id,
            i64::from(update.catalog_schema_version),
            to_i64(update.catalog_revision, "catalog revision is too large")?,
            i64::from(update.envelope_version),
            update.signing_key_id,
            update.envelope,
            update.envelope_sha256,
            update.payload_json,
            update.payload_sha256,
            format_time(update.issued_at),
            format_time(update.effective_at),
            format_time(update.expires_at),
            format_time(update.accepted_at),
        ],
    )?;
    Ok(())
}

fn insert_snapshot(
    transaction: &Transaction<'_>,
    snapshot: &NewCatalogSnapshot<'_>,
    source: &CatalogSnapshotSource,
) -> Result<(), CatalogStorageError> {
    let chain_json = serde_json::to_string(snapshot.signed_revision_chain)
        .map_err(|_| CatalogStorageError::InvalidInput("snapshot chain cannot be serialized"))?;
    let (source_kind, envelope_id, catalog_revision) = match source {
        CatalogSnapshotSource::BundledBaseline => ("bundled_baseline", None, None),
        CatalogSnapshotSource::SignedImport {
            envelope_id,
            catalog_revision,
        } => (
            "signed_import",
            Some(envelope_id.as_str()),
            Some(to_i64(*catalog_revision, "catalog revision is too large")?),
        ),
    };
    transaction.execute(
        "INSERT INTO provider_catalog_snapshots (
             local_revision, snapshot_schema_version, snapshot_json,
             snapshot_sha256, bundled_revision, bundled_sha256,
             signed_revision_chain_json, source_kind, source_envelope_id,
             catalog_revision, captured_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            to_i64(snapshot.local_revision, "local revision is too large")?,
            i64::from(snapshot.snapshot_schema_version),
            snapshot.snapshot_json,
            snapshot.snapshot_sha256,
            to_i64(snapshot.bundled_revision, "bundled revision is too large")?,
            snapshot.bundled_sha256,
            chain_json,
            source_kind,
            envelope_id,
            catalog_revision,
            format_time(snapshot.captured_at),
        ],
    )?;
    Ok(())
}

fn insert_snapshot_chain(
    transaction: &Transaction<'_>,
    local_revision: u64,
    revisions: &[u64],
) -> Result<(), CatalogStorageError> {
    for (ordinal, revision) in revisions.iter().copied().enumerate() {
        let update = load_catalog_update_by_revision(transaction, revision)?.ok_or(
            CatalogStorageError::InvalidInput("snapshot chain references an unknown update"),
        )?;
        transaction.execute(
            "INSERT INTO provider_catalog_snapshot_envelopes (
                 local_revision, ordinal, envelope_id, catalog_revision,
                 payload_sha256
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                to_i64(local_revision, "local revision is too large")?,
                i64::try_from(ordinal)
                    .map_err(|_| CatalogStorageError::InvalidInput("snapshot chain is too long"))?,
                update.id,
                to_i64(revision, "catalog revision is too large")?,
                update.payload_sha256,
            ],
        )?;
    }
    Ok(())
}

fn update_state_for_import(
    transaction: &Transaction<'_>,
    commit: &CatalogImportCommit<'_>,
    next_version: u64,
) -> Result<(), CatalogStorageError> {
    let expected_active_revision = commit
        .expected
        .active
        .as_ref()
        .map(|active| to_i64(active.local_revision, "active revision is too large"))
        .transpose()?;
    let expected_active_hash = commit
        .expected
        .active
        .as_ref()
        .map(|active| active.snapshot_sha256.as_str());
    let changed = transaction.execute(
        "UPDATE provider_catalog_state
         SET state_version = ?1,
             active_local_revision = ?2,
             active_snapshot_sha256 = ?3,
             highest_accepted_revision = ?4,
             latest_issued_at = ?5,
             updated_at = ?6
         WHERE singleton = 1
           AND state_version = ?7
           AND active_local_revision IS ?8
           AND active_snapshot_sha256 IS ?9
           AND highest_accepted_revision = ?10
           AND latest_issued_at IS ?11",
        params![
            to_i64(next_version, "state version is too large")?,
            to_i64(
                commit.snapshot.local_revision,
                "local revision is too large"
            )?,
            commit.snapshot.snapshot_sha256,
            to_i64(
                commit.next_guard.highest_accepted_revision,
                "accepted revision is too large"
            )?,
            commit.next_guard.latest_issued_at.map(format_time),
            format_time(commit.activated_at),
            to_i64(
                commit.expected.state_version,
                "expected state version is too large"
            )?,
            expected_active_revision,
            expected_active_hash,
            to_i64(
                commit.expected.guard.highest_accepted_revision,
                "expected guard revision is too large"
            )?,
            commit.expected.guard.latest_issued_at.map(format_time),
        ],
    )?;
    if changed != 1 {
        return Err(CatalogStorageError::StateConflict);
    }
    Ok(())
}

fn update_state_for_rollback(
    transaction: &Transaction<'_>,
    commit: &CatalogRollbackCommit<'_>,
    next_version: u64,
) -> Result<(), CatalogStorageError> {
    let expected_active = commit
        .expected
        .active
        .as_ref()
        .ok_or(CatalogStorageError::StateConflict)?;
    let changed = transaction.execute(
        "UPDATE provider_catalog_state
         SET state_version = ?1,
             active_local_revision = ?2,
             active_snapshot_sha256 = ?3,
             updated_at = ?4
         WHERE singleton = 1
           AND state_version = ?5
           AND active_local_revision = ?6
           AND active_snapshot_sha256 = ?7
           AND highest_accepted_revision = ?8
           AND latest_issued_at IS ?9",
        params![
            to_i64(next_version, "state version is too large")?,
            to_i64(
                commit.target_local_revision,
                "rollback target revision is too large"
            )?,
            commit.target_snapshot_sha256,
            format_time(commit.activated_at),
            to_i64(
                commit.expected.state_version,
                "expected state version is too large"
            )?,
            to_i64(
                expected_active.local_revision,
                "expected active revision is too large"
            )?,
            expected_active.snapshot_sha256,
            to_i64(
                commit.expected.guard.highest_accepted_revision,
                "expected guard revision is too large"
            )?,
            commit.expected.guard.latest_issued_at.map(format_time),
        ],
    )?;
    if changed != 1 {
        return Err(CatalogStorageError::StateConflict);
    }
    Ok(())
}

fn insert_import_audit(
    transaction: &Transaction<'_>,
    commit: &CatalogImportCommit<'_>,
    state_version: u64,
    from: Option<&StoredCatalogActiveSnapshot>,
) -> Result<(), CatalogStorageError> {
    let (from_revision, from_hash) = optional_active_sql(from)?;
    transaction.execute(
        "INSERT INTO provider_catalog_activation_audit (
             action_id, state_version, activation_kind,
             from_local_revision, from_snapshot_sha256,
             to_local_revision, to_snapshot_sha256, source_envelope_id,
             signed_catalog_revision, signing_key_id, diff_json,
             diff_sha256, rollback_plan_json, plan_sha256, activated_at
         ) VALUES (
             ?1, ?2, 'import', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
             NULL, NULL, ?12
         )",
        params![
            commit.action_id,
            to_i64(state_version, "state version is too large")?,
            from_revision,
            from_hash,
            to_i64(
                commit.snapshot.local_revision,
                "local revision is too large"
            )?,
            commit.snapshot.snapshot_sha256,
            commit.update.id,
            to_i64(
                commit.update.catalog_revision,
                "catalog revision is too large"
            )?,
            commit.update.signing_key_id,
            commit.diff_json,
            sha256_hex(commit.diff_json.as_bytes()),
            format_time(commit.activated_at),
        ],
    )?;
    Ok(())
}

fn insert_rollback_audit(
    transaction: &Transaction<'_>,
    commit: &CatalogRollbackCommit<'_>,
    state_version: u64,
    from: &StoredCatalogActiveSnapshot,
    target: &StoredCatalogSnapshot,
) -> Result<(), CatalogStorageError> {
    transaction.execute(
        "INSERT INTO provider_catalog_activation_audit (
             action_id, state_version, activation_kind,
             from_local_revision, from_snapshot_sha256,
             to_local_revision, to_snapshot_sha256, source_envelope_id,
             signed_catalog_revision, signing_key_id, diff_json,
             diff_sha256, rollback_plan_json, plan_sha256, activated_at
         ) VALUES (
             ?1, ?2, 'rollback', ?3, ?4, ?5, ?6, NULL, NULL, NULL,
             ?7, ?8, ?9, ?10, ?11
         )",
        params![
            commit.action_id,
            to_i64(state_version, "state version is too large")?,
            to_i64(from.local_revision, "source revision is too large")?,
            from.snapshot_sha256,
            to_i64(target.local_revision, "target revision is too large")?,
            target.snapshot_sha256,
            commit.diff_json,
            sha256_hex(commit.diff_json.as_bytes()),
            commit.rollback_plan_json,
            commit.plan_sha256,
            format_time(commit.activated_at),
        ],
    )?;
    Ok(())
}

fn optional_active_sql(
    active: Option<&StoredCatalogActiveSnapshot>,
) -> Result<(Option<i64>, Option<&str>), CatalogStorageError> {
    match active {
        Some(active) => Ok((
            Some(to_i64(
                active.local_revision,
                "active revision is too large",
            )?),
            Some(active.snapshot_sha256.as_str()),
        )),
        None => Ok((None, None)),
    }
}

fn require_expected_state(
    current: &StoredCatalogState,
    expected: &CatalogStateExpectation,
) -> Result<(), CatalogStorageError> {
    if current.state_version != expected.state_version
        || current.active != expected.active
        || current.guard != expected.guard
    {
        return Err(CatalogStorageError::StateConflict);
    }
    Ok(())
}

fn maximum_local_revision(connection: &Connection) -> Result<u64, CatalogStorageError> {
    let maximum = connection.query_row(
        "SELECT COALESCE(MAX(local_revision), 0)
         FROM provider_catalog_snapshots",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    from_i64(maximum, "negative maximum local revision")
}

fn validate_signed_update(update: &NewSignedCatalogUpdate<'_>) -> Result<(), CatalogStorageError> {
    validate_identifier(update.id, 160)?;
    validate_identifier(update.catalog_id, 160)?;
    validate_signing_key_id(update.signing_key_id)?;
    if update.catalog_schema_version == 0
        || update.catalog_revision == 0
        || update.catalog_revision > CATALOG_REVISION_V1_MAX
        || update.envelope_version == 0
    {
        return Err(CatalogStorageError::InvalidInput(
            "catalog versions and revision must be positive",
        ));
    }
    validate_sha(update.envelope_sha256)?;
    validate_sha(update.payload_sha256)?;
    if sha256_hex(update.envelope) != update.envelope_sha256 {
        return Err(CatalogStorageError::InvalidInput(
            "signed envelope hash does not match",
        ));
    }
    if sha256_hex(update.payload_json.as_bytes()) != update.payload_sha256 {
        return Err(CatalogStorageError::InvalidInput(
            "catalog payload hash does not match",
        ));
    }
    if update.issued_at > update.effective_at
        || update.effective_at >= update.expires_at
        || update.accepted_at < update.effective_at
        || update.accepted_at >= update.expires_at
    {
        return Err(CatalogStorageError::InvalidInput(
            "catalog payload timestamps are not ordered",
        ));
    }
    let envelope = parse_json_bytes(update.envelope, MAX_ENVELOPE_BYTES, "signed envelope")?;
    validate_envelope_metadata(&envelope, update)?;
    let payload = parse_json_bytes(
        update.payload_json.as_bytes(),
        MAX_PAYLOAD_BYTES,
        "catalog payload",
    )?;
    validate_payload_metadata(&payload, update)
}

fn validate_snapshot(
    snapshot: &NewCatalogSnapshot<'_>,
    baseline: bool,
) -> Result<(), CatalogStorageError> {
    if snapshot.local_revision == 0
        || snapshot.snapshot_schema_version == 0
        || snapshot.bundled_revision == 0
    {
        return Err(CatalogStorageError::InvalidInput(
            "snapshot versions must be positive",
        ));
    }
    validate_sha(snapshot.snapshot_sha256)?;
    validate_sha(snapshot.bundled_sha256)?;
    if sha256_hex(snapshot.snapshot_json.as_bytes()) != snapshot.snapshot_sha256 {
        return Err(CatalogStorageError::InvalidInput(
            "snapshot hash does not match",
        ));
    }
    validate_revision_chain(snapshot.signed_revision_chain)?;
    if baseline
        && (!snapshot.signed_revision_chain.is_empty()
            || snapshot.local_revision != snapshot.bundled_revision
            || snapshot.snapshot_sha256 != snapshot.bundled_sha256)
    {
        return Err(CatalogStorageError::InvalidInput(
            "bundled baseline metadata is inconsistent",
        ));
    }
    let value = parse_json_bytes(
        snapshot.snapshot_json.as_bytes(),
        MAX_SNAPSHOT_BYTES,
        "catalog snapshot",
    )?;
    require_json_u64(
        &value,
        "snapshot_schema_version",
        u64::from(snapshot.snapshot_schema_version),
        "snapshot schema version differs from metadata",
    )?;
    require_json_u64(
        &value,
        "revision",
        snapshot.local_revision,
        "snapshot revision differs from metadata",
    )?;
    require_json_time(
        &value,
        "captured_at",
        snapshot.captured_at,
        "snapshot capture time differs from metadata",
    )?;
    require_array(&value, "manifests", "snapshot manifests are missing")?;
    require_array(&value, "models", "snapshot models are missing")
}

fn validate_revision_chain(revisions: &[u64]) -> Result<(), CatalogStorageError> {
    if revisions.iter().copied().any(|revision| revision == 0)
        || revisions.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(CatalogStorageError::InvalidInput(
            "signed revision chain must be strictly increasing",
        ));
    }
    Ok(())
}

fn validate_envelope_metadata(
    value: &Value,
    update: &NewSignedCatalogUpdate<'_>,
) -> Result<(), CatalogStorageError> {
    require_json_u64(
        value,
        "envelope_version",
        u64::from(update.envelope_version),
        "envelope version differs from metadata",
    )?;
    require_json_string(
        value,
        "signing_key_id",
        update.signing_key_id,
        "signing key differs from metadata",
    )?;
    require_nonempty_string(value, "payload_base64", "signed envelope has no payload")?;
    require_nonempty_string(
        value,
        "signature_base64",
        "signed envelope has no signature",
    )
}

fn validate_payload_metadata(
    value: &Value,
    update: &NewSignedCatalogUpdate<'_>,
) -> Result<(), CatalogStorageError> {
    require_json_u64(
        value,
        "schema_version",
        u64::from(update.catalog_schema_version),
        "payload schema version differs from metadata",
    )?;
    require_json_string(
        value,
        "catalog_id",
        update.catalog_id,
        "payload catalog ID differs from metadata",
    )?;
    require_json_u64(
        value,
        "revision",
        update.catalog_revision,
        "payload revision differs from metadata",
    )?;
    require_json_time(
        value,
        "issued_at",
        update.issued_at,
        "payload issue time differs from metadata",
    )?;
    require_json_time(
        value,
        "effective_at",
        update.effective_at,
        "payload effective time differs from metadata",
    )?;
    require_json_time(
        value,
        "expires_at",
        update.expires_at,
        "payload expiry time differs from metadata",
    )?;
    require_array(value, "manifests", "payload manifests are missing")?;
    require_array(value, "models", "payload models are missing")
}

fn validate_diff_json(
    json: &str,
    expected_from: u64,
    expected_to: u64,
) -> Result<Value, CatalogStorageError> {
    let value = parse_json_bytes(json.as_bytes(), MAX_DIFF_BYTES, "catalog diff")?;
    let version = value
        .get("diff_schema_version")
        .and_then(Value::as_u64)
        .ok_or(CatalogStorageError::InvalidInput(
            "catalog diff version is missing",
        ))?;
    if version == 0
        || value.get("from_revision").and_then(Value::as_u64) != Some(expected_from)
        || value.get("to_revision").and_then(Value::as_u64) != Some(expected_to)
    {
        return Err(CatalogStorageError::InvalidInput(
            "catalog diff does not match activation",
        ));
    }
    require_array(
        &value,
        "manifest_changes",
        "catalog manifest diff is missing",
    )?;
    require_array(&value, "model_changes", "catalog model diff is missing")?;
    Ok(value)
}

fn validate_rollback_plan(
    commit: &CatalogRollbackCommit<'_>,
    from: &StoredCatalogActiveSnapshot,
    target: &StoredCatalogSnapshot,
) -> Result<(), CatalogStorageError> {
    validate_sha(commit.plan_sha256)?;
    if sha256_hex(commit.rollback_plan_json.as_bytes()) != commit.plan_sha256 {
        return Err(CatalogStorageError::InvalidInput(
            "rollback plan hash does not match",
        ));
    }
    let plan = parse_json_bytes(
        commit.rollback_plan_json.as_bytes(),
        MAX_PLAN_BYTES,
        "rollback plan",
    )?;
    let version = plan
        .get("rollback_plan_version")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if !matches!(
        version,
        ROLLBACK_PLAN_VERSION_WITH_INLINE_DIFF | ROLLBACK_AUDIT_PLAN_VERSION_WITH_DIFF_HASH
    ) || plan.get("from_revision").and_then(Value::as_u64) != Some(from.local_revision)
        || plan.get("to_revision").and_then(Value::as_u64) != Some(target.local_revision)
        || plan.get("expected_active_sha256").and_then(Value::as_str)
            != Some(from.snapshot_sha256.as_str())
        || plan.get("target_sha256").and_then(Value::as_str)
            != Some(target.snapshot_sha256.as_str())
    {
        return Err(CatalogStorageError::InvalidInput(
            "rollback plan does not match current state",
        ));
    }
    let created_at = json_time(
        &plan,
        "created_at",
        "rollback plan creation time is missing",
    )?;
    let expires_at = json_time(&plan, "expires_at", "rollback plan expiry is missing")?;
    if created_at > commit.activated_at
        || expires_at <= commit.activated_at
        || expires_at <= created_at
    {
        return Err(CatalogStorageError::InvalidInput("rollback plan is stale"));
    }
    let supplied_diff =
        validate_diff_json(commit.diff_json, from.local_revision, target.local_revision)?;
    match version {
        ROLLBACK_PLAN_VERSION_WITH_INLINE_DIFF => {
            if plan.get("diff") != Some(&supplied_diff) {
                return Err(CatalogStorageError::InvalidInput(
                    "rollback plan diff was changed",
                ));
            }
        }
        ROLLBACK_AUDIT_PLAN_VERSION_WITH_DIFF_HASH => {
            let supplied_diff_sha256 = sha256_hex(commit.diff_json.as_bytes());
            if plan.get("diff").is_some()
                || plan.get("diff_sha256").and_then(Value::as_str)
                    != Some(supplied_diff_sha256.as_str())
            {
                return Err(CatalogStorageError::InvalidInput(
                    "rollback plan diff hash was changed",
                ));
            }
        }
        _ => unreachable!("rollback plan version was checked above"),
    }
    Ok(())
}

fn validate_action_id(value: &str) -> Result<(), CatalogStorageError> {
    validate_identifier(value, 160)
}

fn validate_identifier(value: &str, maximum: usize) -> Result<(), CatalogStorageError> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CatalogStorageError::InvalidInput(
            "invalid catalog identifier",
        ));
    }
    Ok(())
}

fn validate_signing_key_id(value: &str) -> Result<(), CatalogStorageError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(CatalogStorageError::InvalidInput("invalid signing key ID"));
    }
    Ok(())
}

fn validate_sha(value: &str) -> Result<(), CatalogStorageError> {
    if !is_sha256(value) {
        return Err(CatalogStorageError::InvalidInput(
            "SHA-256 must be lowercase hexadecimal",
        ));
    }
    Ok(())
}

fn checked_stored_sha(value: String) -> Result<String, CatalogStorageError> {
    if !is_sha256(&value) {
        return Err(CatalogStorageError::Corrupted("invalid stored SHA-256"));
    }
    Ok(value)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_json_bytes(
    bytes: &[u8],
    maximum: usize,
    name: &'static str,
) -> Result<Value, CatalogStorageError> {
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(CatalogStorageError::InvalidInput(
            "catalog JSON size is invalid",
        ));
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| CatalogStorageError::InvalidInput(name))?;
    if !value.is_object() {
        return Err(CatalogStorageError::InvalidInput(
            "catalog JSON must be an object",
        ));
    }
    Ok(value)
}

fn require_json_u64(
    value: &Value,
    field: &str,
    expected: u64,
    reason: &'static str,
) -> Result<(), CatalogStorageError> {
    if value.get(field).and_then(Value::as_u64) != Some(expected) {
        return Err(CatalogStorageError::InvalidInput(reason));
    }
    Ok(())
}

fn require_json_string(
    value: &Value,
    field: &str,
    expected: &str,
    reason: &'static str,
) -> Result<(), CatalogStorageError> {
    if value.get(field).and_then(Value::as_str) != Some(expected) {
        return Err(CatalogStorageError::InvalidInput(reason));
    }
    Ok(())
}

fn require_nonempty_string(
    value: &Value,
    field: &str,
    reason: &'static str,
) -> Result<(), CatalogStorageError> {
    if value
        .get(field)
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(CatalogStorageError::InvalidInput(reason));
    }
    Ok(())
}

fn require_array(
    value: &Value,
    field: &str,
    reason: &'static str,
) -> Result<(), CatalogStorageError> {
    if !value.get(field).is_some_and(Value::is_array) {
        return Err(CatalogStorageError::InvalidInput(reason));
    }
    Ok(())
}

fn require_json_time(
    value: &Value,
    field: &str,
    expected: DateTime<Utc>,
    reason: &'static str,
) -> Result<(), CatalogStorageError> {
    if json_time(value, field, reason)? != expected {
        return Err(CatalogStorageError::InvalidInput(reason));
    }
    Ok(())
}

fn json_time(
    value: &Value,
    field: &str,
    reason: &'static str,
) -> Result<DateTime<Utc>, CatalogStorageError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(CatalogStorageError::InvalidInput(reason))
        .and_then(|text| {
            DateTime::parse_from_rfc3339(text)
                .map(|time| time.with_timezone(&Utc))
                .map_err(|_| CatalogStorageError::InvalidInput(reason))
        })
}

fn format_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn parse_stored_time(value: &str) -> Result<DateTime<Utc>, CatalogStorageError> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| CatalogStorageError::Corrupted("invalid stored timestamp"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn to_i64(value: u64, reason: &'static str) -> Result<i64, CatalogStorageError> {
    i64::try_from(value).map_err(|_| CatalogStorageError::InvalidInput(reason))
}

fn from_i64(value: i64, reason: &'static str) -> Result<u64, CatalogStorageError> {
    u64::try_from(value).map_err(|_| CatalogStorageError::Corrupted(reason))
}

fn from_i64_u32(value: i64, reason: &'static str) -> Result<u32, CatalogStorageError> {
    u32::try_from(value).map_err(|_| CatalogStorageError::Corrupted(reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURED: &str = "2026-07-31T00:00:00Z";
    const ISSUED: &str = "2026-08-01T00:00:00Z";
    const EFFECTIVE: &str = "2026-08-01T00:00:01Z";
    const EXPIRED_AFTER_ACCEPTANCE: &str = "2026-08-02T00:00:00Z";
    const EXPIRES: &str = "2027-08-01T00:00:00Z";

    #[test]
    fn import_reopens_and_rollback_preserves_guard() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("catalog.sqlite3");
        let mut connection = Connection::open(&path).expect("open database");
        migrate(&connection);
        let imported = commit_first_update(&mut connection).expect("commit first update");
        assert_eq!(imported.guard.highest_accepted_revision, 2);
        assert_eq!(imported.snapshot_count, 2);
        drop(connection);

        let mut reopened = Connection::open(&path).expect("reopen database");
        reopened
            .execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        let state = load_catalog_state(&reopened).expect("load reopened state");
        let signed = load_catalog_snapshot(&reopened, 2)
            .expect("load snapshot")
            .expect("signed snapshot");
        assert_eq!(signed.signed_revision_chain, vec![2]);
        let baseline = load_catalog_snapshot(&reopened, 1)
            .expect("load snapshot")
            .expect("baseline snapshot");
        let diff = diff_json(2, 1);
        let activated_at = time("2026-08-01T00:05:00Z");
        let plan = rollback_plan_json(
            2,
            1,
            &state.active.as_ref().expect("active").snapshot_sha256,
            &baseline.snapshot_sha256,
            &diff,
        );
        let plan_hash = sha256_hex(plan.as_bytes());
        let rollback = CatalogRollbackCommit {
            expected: state.expectation(),
            action_id: "rollback-to-bundled",
            target_local_revision: 1,
            target_snapshot_sha256: &baseline.snapshot_sha256,
            diff_json: &diff,
            rollback_plan_json: &plan,
            plan_sha256: &plan_hash,
            activated_at,
        };
        let rolled_back =
            activate_catalog_rollback(&mut reopened, &rollback).expect("activate rollback");
        assert_eq!(
            rolled_back
                .active
                .expect("active after rollback")
                .local_revision,
            1
        );
        assert_eq!(rolled_back.guard, state.guard);
        assert_eq!(rolled_back.activation_count, 2);
    }

    #[test]
    fn compact_rollback_plan_accepts_a_diff_larger_than_the_legacy_plan_limit() {
        let mut connection = Connection::open_in_memory().expect("in-memory database");
        migrate(&connection);
        let imported = commit_first_update(&mut connection).expect("first import");
        let baseline = load_catalog_snapshot(&connection, 1)
            .expect("load baseline")
            .expect("baseline snapshot");
        let signed = load_catalog_snapshot(&connection, 2)
            .expect("load signed snapshot")
            .expect("signed snapshot");
        let diff = large_diff_json(2, 1);
        assert!(diff.len() > MAX_PLAN_BYTES);
        assert!(diff.len() <= MAX_DIFF_BYTES);
        let diff_hash = sha256_hex(diff.as_bytes());
        let plan = compact_rollback_plan_json(
            2,
            1,
            &signed.snapshot_sha256,
            &baseline.snapshot_sha256,
            &diff_hash,
        );
        assert!(plan.len() < MAX_PLAN_BYTES);
        let plan_hash = sha256_hex(plan.as_bytes());
        let tampered_plan = compact_rollback_plan_json(
            2,
            1,
            &signed.snapshot_sha256,
            &baseline.snapshot_sha256,
            &sha256_hex(b"different diff"),
        );
        let tampered_plan_hash = sha256_hex(tampered_plan.as_bytes());
        let tampered = CatalogRollbackCommit {
            expected: imported.expectation(),
            action_id: "rollback-with-tampered-large-diff",
            target_local_revision: 1,
            target_snapshot_sha256: &baseline.snapshot_sha256,
            diff_json: &diff,
            rollback_plan_json: &tampered_plan,
            plan_sha256: &tampered_plan_hash,
            activated_at: time("2026-08-01T00:05:00Z"),
        };
        assert!(matches!(
            activate_catalog_rollback(&mut connection, &tampered),
            Err(CatalogStorageError::InvalidInput(
                "rollback plan diff hash was changed"
            ))
        ));
        let rollback = CatalogRollbackCommit {
            expected: imported.expectation(),
            action_id: "rollback-with-large-diff",
            target_local_revision: 1,
            target_snapshot_sha256: &baseline.snapshot_sha256,
            diff_json: &diff,
            rollback_plan_json: &plan,
            plan_sha256: &plan_hash,
            activated_at: time("2026-08-01T00:05:00Z"),
        };

        let rolled_back = activate_catalog_rollback(&mut connection, &rollback)
            .expect("compact rollback proof must not duplicate the large diff");

        assert_eq!(
            rolled_back.active.expect("active snapshot").local_revision,
            1
        );
    }

    #[test]
    fn loading_a_pre_bound_high_revision_guard_remains_compatible() {
        let connection = Connection::open_in_memory().expect("in-memory database");
        migrate(&connection);
        connection
            .execute_batch(
                "DROP TRIGGER provider_catalog_state_guard_matches_history;
                 UPDATE provider_catalog_state
                 SET highest_accepted_revision = 9223372036854775807,
                     latest_issued_at = '2026-08-01T00:00:00Z'
                 WHERE singleton = 1;",
            )
            .expect("simulate a guard persisted by the pre-bound release");

        let state = load_catalog_state(&connection)
            .expect("pre-bound persisted guard must remain readable");

        assert_eq!(state.guard.highest_accepted_revision, i64::MAX as u64);
        assert_eq!(state.guard.latest_issued_at, Some(time(ISSUED)));
    }

    #[test]
    fn stale_import_cas_is_atomic_and_guard_cannot_be_lowered() {
        let mut connection = Connection::open_in_memory().expect("in-memory database");
        migrate(&connection);
        let initial = load_catalog_state(&connection).expect("initial state");
        commit_first_update(&mut connection).expect("first import");
        let result = commit_first_update_with_expected(&mut connection, initial.expectation());
        assert!(matches!(result, Err(CatalogStorageError::StateConflict)));
        let state = load_catalog_state(&connection).expect("state after stale import");
        assert_eq!(state.snapshot_count, 2);
        assert_eq!(state.update_count, 1);
        let lowering = connection.execute(
            "UPDATE provider_catalog_state
             SET highest_accepted_revision = 1
             WHERE singleton = 1",
            [],
        );
        assert!(lowering.is_err());
        assert_eq!(
            load_catalog_state(&connection)
                .expect("state after rejected lowering")
                .guard,
            state.guard
        );
    }

    #[test]
    fn signed_update_rejects_revision_values_that_consume_reserved_epoch_space() {
        let revision = i64::MAX as u64;
        let payload = format!(
            "{{\"schema_version\":1,\"catalog_id\":\"lorepia-provider-model-catalog\",\
             \"revision\":{revision},\"issued_at\":\"{ISSUED}\",\
             \"effective_at\":\"{EFFECTIVE}\",\"expires_at\":\"{EXPIRES}\",\
             \"manifests\":[],\"models\":[]}}"
        );
        let payload_hash = sha256_hex(payload.as_bytes());
        let envelope = br#"{"envelope_version":1,"signing_key_id":"test-key-1","payload_base64":"e30=","signature_base64":"c2ln"}"#;
        let envelope_hash = sha256_hex(envelope);
        let update = NewSignedCatalogUpdate {
            id: "catalog-envelope-exhausting",
            catalog_id: "lorepia-provider-model-catalog",
            catalog_schema_version: 1,
            catalog_revision: revision,
            envelope_version: 1,
            signing_key_id: "test-key-1",
            envelope,
            envelope_sha256: &envelope_hash,
            payload_json: &payload,
            payload_sha256: &payload_hash,
            issued_at: time(ISSUED),
            effective_at: time(EFFECTIVE),
            expires_at: time(EXPIRES),
            accepted_at: time(EFFECTIVE),
        };

        assert!(matches!(
            validate_signed_update(&update),
            Err(CatalogStorageError::InvalidInput(_))
        ));
    }

    #[test]
    fn import_rejects_payload_expired_at_transaction_time_despite_supplied_acceptance_time() {
        let mut connection = Connection::open_in_memory().expect("in-memory database");
        migrate(&connection);
        let expected = load_catalog_state(&connection)
            .expect("initial state")
            .expectation();

        let result = commit_first_update_with_expected_and_expiry(
            &mut connection,
            expected,
            EXPIRED_AFTER_ACCEPTANCE,
        );

        assert!(matches!(
            result,
            Err(CatalogStorageError::InvalidInput(
                "catalog payload is expired"
            ))
        ));
        let state = load_catalog_state(&connection).expect("state after rejected expired import");
        assert_eq!(state.state_version, 0);
        assert_eq!(state.snapshot_count, 0);
        assert_eq!(state.update_count, 0);
        assert_eq!(state.activation_count, 0);
    }

    #[test]
    fn failed_activation_audit_rolls_back_every_import_row() {
        let mut connection = Connection::open_in_memory().expect("in-memory database");
        migrate(&connection);
        connection
            .execute_batch(
                "CREATE TRIGGER test_reject_catalog_audit
                 BEFORE INSERT ON provider_catalog_activation_audit
                 BEGIN
                     SELECT RAISE(ABORT, 'test audit failure');
                 END;",
            )
            .expect("install failure trigger");
        assert!(commit_first_update(&mut connection).is_err());
        let state = load_catalog_state(&connection).expect("state after failed import");
        assert_eq!(state.state_version, 0);
        assert_eq!(state.snapshot_count, 0);
        assert_eq!(state.update_count, 0);
        assert_eq!(state.activation_count, 0);
    }

    #[test]
    fn failed_rollback_audit_restores_active_pointer_and_revision_guard() {
        let mut connection = Connection::open_in_memory().expect("in-memory database");
        migrate(&connection);
        let imported = commit_first_update(&mut connection).expect("first import");
        let baseline = load_catalog_snapshot(&connection, 1)
            .expect("load baseline")
            .expect("baseline");
        let signed = load_catalog_snapshot(&connection, 2)
            .expect("load signed snapshot")
            .expect("signed snapshot");
        connection
            .execute_batch(
                "CREATE TRIGGER test_reject_catalog_rollback_audit
                 BEFORE INSERT ON provider_catalog_activation_audit
                 WHEN NEW.activation_kind = 'rollback'
                 BEGIN
                     SELECT RAISE(ABORT, 'test rollback audit failure');
                 END;",
            )
            .expect("install rollback failure trigger");

        let diff = diff_json(2, 1);
        let plan = rollback_plan_json(
            2,
            1,
            &signed.snapshot_sha256,
            &baseline.snapshot_sha256,
            &diff,
        );
        let plan_hash = sha256_hex(plan.as_bytes());
        let rollback = CatalogRollbackCommit {
            expected: imported.expectation(),
            action_id: "rollback-audit-failure",
            target_local_revision: 1,
            target_snapshot_sha256: &baseline.snapshot_sha256,
            diff_json: &diff,
            rollback_plan_json: &plan,
            plan_sha256: &plan_hash,
            activated_at: time("2026-08-01T00:05:00Z"),
        };
        assert!(
            activate_catalog_rollback(&mut connection, &rollback).is_err(),
            "audit failure must abort the rollback"
        );
        let after = load_catalog_state(&connection).expect("state after failed rollback");
        assert_eq!(after, imported);
        assert_eq!(after.active.expect("active snapshot").local_revision, 2);
        assert_eq!(after.activation_count, 1);
    }

    #[test]
    fn rollback_plan_cannot_be_replayed_after_an_aba_pointer_cycle() {
        let mut connection = Connection::open_in_memory().expect("in-memory database");
        migrate(&connection);
        let imported = commit_first_update(&mut connection).expect("first import");
        let baseline = load_catalog_snapshot(&connection, 1)
            .expect("load baseline")
            .expect("baseline");
        let signed = load_catalog_snapshot(&connection, 2)
            .expect("load signed snapshot")
            .expect("signed snapshot");

        let backward_diff = diff_json(2, 1);
        let backward_plan = rollback_plan_json(
            2,
            1,
            &signed.snapshot_sha256,
            &baseline.snapshot_sha256,
            &backward_diff,
        );
        let backward_hash = sha256_hex(backward_plan.as_bytes());
        let first = CatalogRollbackCommit {
            expected: imported.expectation(),
            action_id: "rollback-first",
            target_local_revision: 1,
            target_snapshot_sha256: &baseline.snapshot_sha256,
            diff_json: &backward_diff,
            rollback_plan_json: &backward_plan,
            plan_sha256: &backward_hash,
            activated_at: time("2026-08-01T00:05:00Z"),
        };
        let at_baseline =
            activate_catalog_rollback(&mut connection, &first).expect("activate first rollback");

        let forward_diff = diff_json(1, 2);
        let forward_plan = rollback_plan_json(
            1,
            2,
            &baseline.snapshot_sha256,
            &signed.snapshot_sha256,
            &forward_diff,
        );
        let forward_hash = sha256_hex(forward_plan.as_bytes());
        let restore = CatalogRollbackCommit {
            expected: at_baseline.expectation(),
            action_id: "restore-signed",
            target_local_revision: 2,
            target_snapshot_sha256: &signed.snapshot_sha256,
            diff_json: &forward_diff,
            rollback_plan_json: &forward_plan,
            plan_sha256: &forward_hash,
            activated_at: time("2026-08-01T00:06:00Z"),
        };
        let restored =
            activate_catalog_rollback(&mut connection, &restore).expect("restore signed snapshot");
        assert_eq!(restored.active.as_ref().expect("active").local_revision, 2);

        assert!(matches!(
            activate_catalog_rollback(&mut connection, &first),
            Err(CatalogStorageError::StateConflict)
        ));
        assert_eq!(
            load_catalog_state(&connection)
                .expect("state after rejected replay")
                .state_version,
            restored.state_version
        );
    }

    fn migrate(connection: &Connection) {
        connection
            .execute_batch(SIGNED_CATALOG_HISTORY_MIGRATION)
            .expect("apply signed catalog migration");
    }

    fn commit_first_update(
        connection: &mut Connection,
    ) -> Result<StoredCatalogState, CatalogStorageError> {
        let expected = load_catalog_state(connection)?.expectation();
        commit_first_update_with_expected(connection, expected)
    }

    fn commit_first_update_with_expected(
        connection: &mut Connection,
        expected: CatalogStateExpectation,
    ) -> Result<StoredCatalogState, CatalogStorageError> {
        commit_first_update_with_expected_and_expiry(connection, expected, EXPIRES)
    }

    fn commit_first_update_with_expected_and_expiry(
        connection: &mut Connection,
        expected: CatalogStateExpectation,
        expires_at: &str,
    ) -> Result<StoredCatalogState, CatalogStorageError> {
        let baseline_json = snapshot_json(1, CAPTURED);
        let baseline_hash = sha256_hex(baseline_json.as_bytes());
        let payload = payload_json(expires_at);
        let payload_hash = sha256_hex(payload.as_bytes());
        let envelope = br#"{"envelope_version":1,"signing_key_id":"test-key-1","payload_base64":"e30=","signature_base64":"c2ln"}"#;
        let envelope_hash = sha256_hex(envelope);
        let signed_json = snapshot_json(2, EFFECTIVE);
        let signed_hash = sha256_hex(signed_json.as_bytes());
        let diff = diff_json(1, 2);
        let baseline = NewCatalogSnapshot {
            local_revision: 1,
            snapshot_schema_version: 1,
            snapshot_json: &baseline_json,
            snapshot_sha256: &baseline_hash,
            bundled_revision: 1,
            bundled_sha256: &baseline_hash,
            signed_revision_chain: &[],
            captured_at: time(CAPTURED),
        };
        let snapshot = NewCatalogSnapshot {
            local_revision: 2,
            snapshot_schema_version: 1,
            snapshot_json: &signed_json,
            snapshot_sha256: &signed_hash,
            bundled_revision: 1,
            bundled_sha256: &baseline_hash,
            signed_revision_chain: &[2],
            captured_at: time(EFFECTIVE),
        };
        let commit = CatalogImportCommit {
            expected,
            action_id: "import-catalog-2",
            update: NewSignedCatalogUpdate {
                id: "catalog-envelope-2",
                catalog_id: "lorepia-provider-model-catalog",
                catalog_schema_version: 1,
                catalog_revision: 2,
                envelope_version: 1,
                signing_key_id: "test-key-1",
                envelope,
                envelope_sha256: &envelope_hash,
                payload_json: &payload,
                payload_sha256: &payload_hash,
                issued_at: time(ISSUED),
                effective_at: time(EFFECTIVE),
                expires_at: time(expires_at),
                accepted_at: time(EFFECTIVE),
            },
            snapshot,
            initial_baseline: Some(baseline),
            next_guard: StoredCatalogRevisionGuard {
                highest_accepted_revision: 2,
                latest_issued_at: Some(time(ISSUED)),
            },
            diff_json: &diff,
            activated_at: time(EFFECTIVE),
        };
        commit_catalog_import(connection, &commit)
    }

    fn snapshot_json(revision: u64, captured_at: &str) -> String {
        format!(
            "{{\"snapshot_schema_version\":1,\"revision\":{revision},\
             \"captured_at\":\"{captured_at}\",\"manifests\":[],\"models\":[]}}"
        )
    }

    fn payload_json(expires_at: &str) -> String {
        format!(
            "{{\"schema_version\":1,\
             \"catalog_id\":\"lorepia-provider-model-catalog\",\
             \"revision\":2,\"issued_at\":\"{ISSUED}\",\
             \"effective_at\":\"{EFFECTIVE}\",\"expires_at\":\"{expires_at}\",\
             \"manifests\":[],\"models\":[]}}"
        )
    }

    fn diff_json(from: u64, to: u64) -> String {
        format!(
            "{{\"diff_schema_version\":1,\"from_revision\":{from},\
             \"to_revision\":{to},\"manifest_changes\":[],\"model_changes\":[]}}"
        )
    }

    fn rollback_plan_json(
        from_revision: u64,
        to_revision: u64,
        active_hash: &str,
        target_hash: &str,
        diff: &str,
    ) -> String {
        format!(
            "{{\"rollback_plan_version\":1,\"from_revision\":{from_revision},\
             \"to_revision\":{to_revision},\"expected_active_sha256\":\"{active_hash}\",\
             \"target_sha256\":\"{target_hash}\",\
             \"created_at\":\"2026-08-01T00:04:00Z\",\
             \"expires_at\":\"2026-08-01T00:20:00Z\",\"diff\":{diff}}}"
        )
    }

    fn large_diff_json(from: u64, to: u64) -> String {
        let model_changes = std::iter::repeat_n("{}", 360_000)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"diff_schema_version\":1,\"from_revision\":{from},\
             \"to_revision\":{to},\"manifest_changes\":[],\
             \"model_changes\":[{model_changes}]}}"
        )
    }

    fn compact_rollback_plan_json(
        from_revision: u64,
        to_revision: u64,
        active_hash: &str,
        target_hash: &str,
        diff_hash: &str,
    ) -> String {
        format!(
            "{{\"rollback_plan_version\":2,\"from_revision\":{from_revision},\
             \"to_revision\":{to_revision},\"expected_active_sha256\":\"{active_hash}\",\
             \"target_sha256\":\"{target_hash}\",\
             \"created_at\":\"2026-08-01T00:04:00Z\",\
             \"expires_at\":\"2026-08-01T00:20:00Z\",\
             \"diff_sha256\":\"{diff_hash}\"}}"
        )
    }

    fn time(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("valid test time")
            .with_timezone(&Utc)
    }
}

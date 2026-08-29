use chrono::{DateTime, Utc};
use lorepia_domain::CoreResult;
use serde_json::Value;

use super::Core;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableRuntimeStateScope {
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub conversation_id: String,
    pub branch_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortableRuntimeStatePayload {
    pub schema_version: u32,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortableRuntimeStateRecord {
    pub scope: PortableRuntimeStateScope,
    pub scope_epoch: u64,
    pub revision: u64,
    pub payload: PortableRuntimeStatePayload,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortableRuntimeStateSnapshot {
    pub scope_epoch: u64,
    pub record: Option<PortableRuntimeStateRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortableRuntimeStateWrite {
    pub scope: PortableRuntimeStateScope,
    pub expected_scope_epoch: u64,
    pub expected_revision: Option<u64>,
    pub payload: PortableRuntimeStatePayload,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PortableRuntimeStateSaveResult {
    Saved {
        record: PortableRuntimeStateRecord,
        evicted_rows: u32,
        evicted_bytes: u64,
    },
    RevisionConflict {
        current: Option<PortableRuntimeStateRecord>,
    },
    ScopeInvalidated {
        current_scope_epoch: u64,
    },
}

impl Core {
    /// Loads the bounded state for one exact character/runtime/branch scope.
    ///
    /// The epoch is returned even when no state exists so the caller can make
    /// an epoch-safe first write without racing an explicit branch rewind.
    pub fn get_portable_runtime_state(
        &self,
        scope: &PortableRuntimeStateScope,
    ) -> CoreResult<PortableRuntimeStateSnapshot> {
        self.inner
            .storage
            .get_portable_runtime_state(&storage_scope(scope))
            .map(core_snapshot)
    }

    /// Saves one runtime state using both branch-epoch and record-revision CAS.
    pub fn put_portable_runtime_state(
        &self,
        write: PortableRuntimeStateWrite,
    ) -> CoreResult<PortableRuntimeStateSaveResult> {
        self.inner
            .storage
            .put_portable_runtime_state(lorepia_storage::PortableRuntimeStateWrite {
                scope: storage_scope(&write.scope),
                expected_scope_epoch: write.expected_scope_epoch,
                expected_revision: write.expected_revision,
                payload: lorepia_storage::PortableRuntimeStatePayload {
                    schema_version: write.payload.schema_version,
                    value: write.payload.value,
                },
            })
            .map(core_save_result)
    }
}

fn storage_scope(scope: &PortableRuntimeStateScope) -> lorepia_storage::PortableRuntimeStateScope {
    lorepia_storage::PortableRuntimeStateScope {
        character_id: scope.character_id.clone(),
        character_content_revision_id: scope.character_content_revision_id.clone(),
        conversation_id: scope.conversation_id.clone(),
        branch_id: scope.branch_id.clone(),
    }
}

fn core_scope(value: lorepia_storage::PortableRuntimeStateScope) -> PortableRuntimeStateScope {
    PortableRuntimeStateScope {
        character_id: value.character_id,
        character_content_revision_id: value.character_content_revision_id,
        conversation_id: value.conversation_id,
        branch_id: value.branch_id,
    }
}

fn core_payload(
    value: lorepia_storage::PortableRuntimeStatePayload,
) -> PortableRuntimeStatePayload {
    PortableRuntimeStatePayload {
        schema_version: value.schema_version,
        value: value.value,
    }
}

fn core_record(value: lorepia_storage::PortableRuntimeStateRecord) -> PortableRuntimeStateRecord {
    PortableRuntimeStateRecord {
        scope: core_scope(value.scope),
        scope_epoch: value.scope_epoch,
        revision: value.revision,
        payload: core_payload(value.payload),
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

fn core_snapshot(
    value: lorepia_storage::PortableRuntimeStateSnapshot,
) -> PortableRuntimeStateSnapshot {
    PortableRuntimeStateSnapshot {
        scope_epoch: value.scope_epoch,
        record: value.record.map(core_record),
    }
}

fn core_save_result(
    value: lorepia_storage::PortableRuntimeStateSaveResult,
) -> PortableRuntimeStateSaveResult {
    match value {
        lorepia_storage::PortableRuntimeStateSaveResult::Saved {
            record,
            evicted_rows,
            evicted_bytes,
        } => PortableRuntimeStateSaveResult::Saved {
            record: core_record(record),
            evicted_rows,
            evicted_bytes,
        },
        lorepia_storage::PortableRuntimeStateSaveResult::RevisionConflict { current } => {
            PortableRuntimeStateSaveResult::RevisionConflict {
                current: current.map(core_record),
            }
        }
        lorepia_storage::PortableRuntimeStateSaveResult::ScopeInvalidated {
            current_scope_epoch,
        } => PortableRuntimeStateSaveResult::ScopeInvalidated {
            current_scope_epoch,
        },
    }
}

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationId, ConversationPersonaSelection, CoreError, CoreErrorCode, CoreResult, PersonaId,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    database::{Storage, storage_db_error},
    orchestration::StoredRevision,
};

/// Reader-safe CAS state for one conversation's persona selection.
///
/// `revision` remains visible after a clear or persona deletion so a restarted
/// client can select another persona without guessing the tombstone revision.
/// Tombstones never expose the formerly selected persona or its immutable
/// revision as though either were still active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationPersonaSelectionState {
    pub conversation_id: ConversationId,
    pub selection: Option<ConversationPersonaSelection>,
    pub revision: Option<u64>,
    pub selected_persona_revision_id: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
    pub cleared_at: Option<DateTime<Utc>>,
}

impl Storage {
    pub fn get_conversation_persona_selection_state(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<ConversationPersonaSelectionState> {
        validate_id("conversation", &conversation_id.0)?;
        let connection = self.connection()?;
        let row = connection
            .query_row(
                "SELECT persona_id, persona_revision_id, revision,
                        updated_at, deleted_at
                 FROM conversation_persona_selections
                 WHERE conversation_id = ?1",
                [&conversation_id.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?;
        let Some((persona_id, persona_revision_id, revision, updated_at, deleted_at)) = row else {
            return Ok(ConversationPersonaSelectionState {
                conversation_id: conversation_id.clone(),
                selection: None,
                revision: None,
                selected_persona_revision_id: None,
                updated_at: None,
                cleared_at: None,
            });
        };
        validate_exact_persona_revision(&connection, &persona_id, &persona_revision_id)?;
        let cleared_at = deleted_at
            .as_deref()
            .map(|value| parse_time("persona selection deleted_at", value))
            .transpose()?;
        let selection = if cleared_at.is_some() {
            None
        } else {
            validate_exact_live_persona_revision(&connection, &persona_id, &persona_revision_id)?;
            Some(ConversationPersonaSelection {
                conversation_id: conversation_id.clone(),
                persona_id: PersonaId::from(persona_id),
            })
        };
        Ok(ConversationPersonaSelectionState {
            conversation_id: conversation_id.clone(),
            selection,
            revision: Some(to_u64("persona selection revision", revision)?),
            selected_persona_revision_id: cleared_at.is_none().then_some(persona_revision_id),
            updated_at: Some(parse_time("persona selection updated_at", &updated_at)?),
            cleared_at,
        })
    }

    pub fn get_conversation_persona_selection(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<Option<StoredRevision<ConversationPersonaSelection>>> {
        validate_id("conversation", &conversation_id.0)?;
        let connection = self.connection()?;
        let row = connection
            .query_row(
                "SELECT persona_id, persona_revision_id, revision,
                        created_at, updated_at, deleted_at
                 FROM conversation_persona_selections
                 WHERE conversation_id = ?1",
                [&conversation_id.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        if row.5.is_some() {
            return Ok(None);
        }
        validate_exact_live_persona_revision(&connection, &row.0, &row.1)?;
        Ok(Some(decode_selection(conversation_id, row)?))
    }

    pub fn save_conversation_persona_selection(
        &self,
        selection: &ConversationPersonaSelection,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<ConversationPersonaSelection>> {
        self.save_conversation_persona_selection_internal(selection, expected_revision, None)
    }

    /// Selects a persona only if the exact immutable persona revision observed
    /// and authorized by `Core` is still active when the selection transaction
    /// begins.
    ///
    /// This prevents an intervening persona update from silently changing the
    /// content revision captured by a selection request.
    pub fn save_conversation_persona_selection_at_revision(
        &self,
        selection: &ConversationPersonaSelection,
        expected_revision: Option<u64>,
        expected_persona_revision_id: &str,
    ) -> CoreResult<StoredRevision<ConversationPersonaSelection>> {
        validate_id("persona revision", expected_persona_revision_id)?;
        self.save_conversation_persona_selection_internal(
            selection,
            expected_revision,
            Some(expected_persona_revision_id),
        )
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one immediate transaction keeps selection CAS, immutable revision pinning, and audit event atomic"
    )]
    fn save_conversation_persona_selection_internal(
        &self,
        selection: &ConversationPersonaSelection,
        expected_revision: Option<u64>,
        expected_persona_revision_id: Option<&str>,
    ) -> CoreResult<StoredRevision<ConversationPersonaSelection>> {
        validate_id("conversation", &selection.conversation_id.0)?;
        validate_id("persona", selection.persona_id.as_str())?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        require_conversation(&transaction, &selection.conversation_id)?;
        let persona_revision_id = active_persona_revision_id(&transaction, &selection.persona_id)?;
        if let Some(expected_persona_revision_id) = expected_persona_revision_id
            && persona_revision_id != expected_persona_revision_id
        {
            return Err(persona_revision_conflict(
                &selection.persona_id,
                expected_persona_revision_id,
                &persona_revision_id,
            ));
        }
        let current = read_selection_state(&transaction, &selection.conversation_id)?;
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let (revision, created_at, event_kind) = match current {
            None => {
                if expected_revision.is_some() {
                    return Err(selection_conflict(
                        &selection.conversation_id,
                        expected_revision,
                        None,
                    ));
                }
                transaction
                    .execute(
                        "INSERT INTO conversation_persona_selections
                         (conversation_id, persona_id, persona_revision_id,
                          revision, created_at, updated_at, deleted_at)
                         VALUES (?1, ?2, ?3, 1, ?4, ?4, NULL)",
                        params![
                            selection.conversation_id.0,
                            selection.persona_id.as_str(),
                            persona_revision_id,
                            now_text,
                        ],
                    )
                    .map_err(storage_db_error)?;
                (1, now, "selected")
            }
            Some(current) => {
                let current_revision = to_u64("persona selection revision", current.2)?;
                if expected_revision != Some(current_revision) {
                    return Err(selection_conflict(
                        &selection.conversation_id,
                        expected_revision,
                        Some(current_revision),
                    ));
                }
                if current.3.is_none()
                    && current.0 == selection.persona_id.as_str()
                    && current.1 == persona_revision_id
                {
                    transaction.commit().map_err(storage_db_error)?;
                    return Ok(StoredRevision {
                        value: selection.clone(),
                        revision: current_revision,
                        revision_id: Some(persona_revision_id),
                        created_at: parse_time("persona selection created_at", &current.4)?,
                        updated_at: parse_time("persona selection updated_at", &current.5)?,
                        deleted_at: None,
                    });
                }
                let next = current_revision
                    .checked_add(1)
                    .ok_or_else(|| CoreError::internal("persona selection revision overflow"))?;
                let changed = transaction
                    .execute(
                        "UPDATE conversation_persona_selections
                         SET persona_id = ?2, persona_revision_id = ?3,
                             revision = ?4, updated_at = ?5, deleted_at = NULL
                         WHERE conversation_id = ?1 AND revision = ?6",
                        params![
                            selection.conversation_id.0,
                            selection.persona_id.as_str(),
                            persona_revision_id,
                            to_i64("persona selection revision", next)?,
                            now_text,
                            to_i64("persona selection revision", current_revision)?,
                        ],
                    )
                    .map_err(storage_db_error)?;
                if changed != 1 {
                    return Err(selection_conflict(
                        &selection.conversation_id,
                        Some(current_revision),
                        None,
                    ));
                }
                (
                    next,
                    parse_time("persona selection created_at", &current.4)?,
                    if current.3.is_some() {
                        "selected"
                    } else {
                        "changed"
                    },
                )
            }
        };
        append_selection_event(
            &transaction,
            &selection.conversation_id,
            revision,
            event_kind,
            &selection.persona_id,
            &persona_revision_id,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(StoredRevision {
            value: selection.clone(),
            revision,
            revision_id: Some(persona_revision_id),
            created_at,
            updated_at: now,
            deleted_at: None,
        })
    }

    pub fn clear_conversation_persona_selection(
        &self,
        conversation_id: &ConversationId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<ConversationPersonaSelection>> {
        validate_id("conversation", &conversation_id.0)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_selection_state(&transaction, conversation_id)?
            .ok_or_else(|| not_found("conversation persona selection"))?;
        let current_revision = to_u64("persona selection revision", current.2)?;
        if current_revision != expected_revision || current.3.is_some() {
            return Err(selection_conflict(
                conversation_id,
                Some(expected_revision),
                Some(current_revision),
            ));
        }
        let next = expected_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("persona selection revision overflow"))?;
        let now = Utc::now();
        let changed = transaction
            .execute(
                "UPDATE conversation_persona_selections
                 SET revision = ?2, updated_at = ?3, deleted_at = ?3
                 WHERE conversation_id = ?1 AND revision = ?4
                   AND deleted_at IS NULL",
                params![
                    conversation_id.0,
                    to_i64("persona selection revision", next)?,
                    now.to_rfc3339(),
                    to_i64("persona selection revision", expected_revision)?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(selection_conflict(
                conversation_id,
                Some(expected_revision),
                None,
            ));
        }
        append_selection_event(
            &transaction,
            conversation_id,
            next,
            "cleared",
            &PersonaId::from(current.0.clone()),
            &current.1,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(StoredRevision {
            value: ConversationPersonaSelection {
                conversation_id: conversation_id.clone(),
                persona_id: PersonaId::from(current.0),
            },
            revision: next,
            revision_id: Some(current.1),
            created_at: parse_time("persona selection created_at", &current.4)?,
            updated_at: now,
            deleted_at: Some(now),
        })
    }
}

pub(crate) fn clear_persona_selections_in_transaction(
    transaction: &Transaction<'_>,
    persona_id: &PersonaId,
    cleared_at: DateTime<Utc>,
) -> CoreResult<()> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT conversation_id, persona_revision_id, revision
                 FROM conversation_persona_selections
                 WHERE persona_id = ?1 AND deleted_at IS NULL
                 ORDER BY conversation_id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([persona_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    for (conversation_id, persona_revision_id, revision) in rows {
        let revision = to_u64("persona selection revision", revision)?;
        let next = revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("persona selection revision overflow"))?;
        let changed = transaction
            .execute(
                "UPDATE conversation_persona_selections
                 SET revision = ?2, updated_at = ?3, deleted_at = ?3
                 WHERE conversation_id = ?1 AND revision = ?4
                   AND deleted_at IS NULL",
                params![
                    conversation_id,
                    to_i64("persona selection revision", next)?,
                    cleared_at.to_rfc3339(),
                    to_i64("persona selection revision", revision)?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(storage_corrupted(
                "selected persona could not atomically tombstone its conversation selection",
            ));
        }
        append_selection_event(
            transaction,
            &ConversationId(conversation_id),
            next,
            "persona_deleted",
            persona_id,
            &persona_revision_id,
            cleared_at,
        )?;
    }
    Ok(())
}

type SelectionState = (String, String, i64, Option<String>, String, String);
type SelectionRow = (String, String, i64, String, String, Option<String>);

fn read_selection_state(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
) -> CoreResult<Option<SelectionState>> {
    transaction
        .query_row(
            "SELECT persona_id, persona_revision_id, revision, deleted_at,
                    created_at, updated_at
             FROM conversation_persona_selections
             WHERE conversation_id = ?1",
            [&conversation_id.0],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)
}

fn decode_selection(
    conversation_id: &ConversationId,
    row: SelectionRow,
) -> CoreResult<StoredRevision<ConversationPersonaSelection>> {
    Ok(StoredRevision {
        value: ConversationPersonaSelection {
            conversation_id: conversation_id.clone(),
            persona_id: PersonaId::from(row.0),
        },
        revision: to_u64("persona selection revision", row.2)?,
        revision_id: Some(row.1),
        created_at: parse_time("persona selection created_at", &row.3)?,
        updated_at: parse_time("persona selection updated_at", &row.4)?,
        deleted_at: row
            .5
            .as_deref()
            .map(|value| parse_time("persona selection deleted_at", value))
            .transpose()?,
    })
}

fn active_persona_revision_id(
    transaction: &Transaction<'_>,
    persona_id: &PersonaId,
) -> CoreResult<String> {
    transaction
        .query_row(
            "SELECT state.active_revision_id
             FROM content_objects AS object
             JOIN content_object_state AS state ON state.object_id = object.id
             JOIN persona_revisions AS revision
               ON revision.persona_id = object.id
              AND revision.revision_id = state.active_revision_id
             WHERE object.id = ?1 AND object.object_kind = 'persona'
               AND object.deleted_at IS NULL",
            [persona_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("live persona"))
}

fn validate_exact_live_persona_revision(
    connection: &rusqlite::Connection,
    persona_id: &str,
    persona_revision_id: &str,
) -> CoreResult<()> {
    let valid = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM content_objects AS object
                JOIN persona_revisions AS revision
                  ON revision.persona_id = object.id
                WHERE object.id = ?1 AND object.object_kind = 'persona'
                  AND object.deleted_at IS NULL
                  AND revision.revision_id = ?2
             )",
            params![persona_id, persona_revision_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if valid {
        Ok(())
    } else {
        Err(storage_corrupted(
            "active persona selection no longer matches an exact revision of a live persona",
        ))
    }
}

fn validate_exact_persona_revision(
    connection: &rusqlite::Connection,
    persona_id: &str,
    persona_revision_id: &str,
) -> CoreResult<()> {
    let valid = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM persona_revisions AS revision
                JOIN content_revisions AS content_revision
                  ON content_revision.object_id = revision.persona_id
                 AND content_revision.id = revision.revision_id
                 AND content_revision.object_kind = 'persona'
                WHERE revision.persona_id = ?1
                  AND revision.revision_id = ?2
             )",
            params![persona_id, persona_revision_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if valid {
        Ok(())
    } else {
        Err(storage_corrupted(
            "persona selection no longer matches an exact immutable persona revision",
        ))
    }
}

fn require_conversation(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
) -> CoreResult<()> {
    let exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
            [&conversation_id.0],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if exists {
        Ok(())
    } else {
        Err(not_found("conversation"))
    }
}

fn append_selection_event(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    revision: u64,
    event_kind: &str,
    persona_id: &PersonaId,
    persona_revision_id: &str,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO conversation_persona_selection_events
             (id, conversation_id, selection_revision, event_kind,
              persona_id, persona_revision_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                Uuid::new_v4().to_string(),
                conversation_id.0,
                to_i64("persona selection revision", revision)?,
                event_kind,
                persona_id.as_str(),
                persona_revision_id,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn validate_id(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(format!("{label} id is invalid")));
    }
    Ok(())
}

fn parse_time(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| storage_corrupted(format!("{label} is invalid: {error}")))
}

fn to_i64(label: &str, value: u64) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid(format!("{label} exceeds SQLite range")))
}

fn to_u64(label: &str, value: i64) -> CoreResult<u64> {
    u64::try_from(value).map_err(|_| storage_corrupted(format!("{label} is negative")))
}

fn selection_conflict(
    conversation_id: &ConversationId,
    expected: Option<u64>,
    actual: Option<u64>,
) -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        format!(
            "conversation persona selection revision conflict for {}: expected {}, current {}",
            conversation_id.0,
            expected.map_or_else(|| "new".to_owned(), |value| value.to_string()),
            actual.map_or_else(|| "missing".to_owned(), |value| value.to_string()),
        ),
        true,
    )
}

fn persona_revision_conflict(persona_id: &PersonaId, expected: &str, actual: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        format!(
            "persona revision conflict for {}: expected {expected}, current {actual}",
            persona_id.as_str()
        ),
        true,
    )
}

fn not_found(kind: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{kind} was not found"),
        false,
    )
}

fn storage_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{Conversation, Persona, Provenance, SourceKind};
    use tempfile::tempdir;

    use super::*;

    fn persona(id: &str) -> Persona {
        let now = Utc::now();
        Persona {
            id: PersonaId::from(id),
            name: format!("Persona {id}"),
            description: String::new(),
            schema_version: 1,
            provenance: Provenance {
                source_kind: SourceKind::UserCreated,
                source_id: None,
                source_hash: None,
                author: None,
                license: None,
                imported_at: None,
            },
            created_at: now,
            updated_at: now,
        }
    }

    fn seed_conversation(storage: &Storage) -> Conversation {
        let now = Utc::now().to_rfc3339();
        let source_hash = "ab".repeat(32);
        let connection = storage.connection().expect("connection");
        connection
            .execute(
                "INSERT INTO content_sources
                 (sha256, relative_path, size_bytes, created_at)
                 VALUES (?1, ?2, 1, ?3)",
                params![source_hash, "sources/fixture", now],
            )
            .expect("source");
        connection
            .execute(
                "INSERT INTO characters
                 (id, name, description, source_hash, avatar_asset_hash, created_at)
                 VALUES ('character-persona-test', 'Character', '', ?1, NULL, ?2)",
                params![source_hash, now],
            )
            .expect("character");
        // Storage methods acquire the same connection mutex. Release this
        // fixture-only guard before calling the public repository API.
        drop(connection);
        let conversation = Conversation::new("character-persona-test", "Persona selection test");
        storage
            .save_conversation(&conversation)
            .expect("conversation");
        conversation
    }

    #[test]
    fn selection_tombstone_revision_survives_restart_and_allows_reselection() {
        let root = tempdir().expect("root");
        let storage = Storage::open(root.path()).expect("open");
        let conversation = seed_conversation(&storage);
        let first = storage
            .save_persona(&persona("persona-first"), None)
            .expect("first persona");
        let second = storage
            .save_persona(&persona("persona-second"), None)
            .expect("second persona");
        let absent = storage
            .get_conversation_persona_selection_state(&conversation.id)
            .expect("absent state");
        assert_eq!(absent.revision, None);

        let selected = storage
            .save_conversation_persona_selection(
                &ConversationPersonaSelection {
                    conversation_id: conversation.id.clone(),
                    persona_id: first.value.id,
                },
                None,
            )
            .expect("select");
        assert_eq!(selected.revision, 1);
        storage
            .clear_conversation_persona_selection(&conversation.id, selected.revision)
            .expect("clear");
        drop(storage);

        let reopened = Storage::open(root.path()).expect("reopen");
        let tombstone = reopened
            .get_conversation_persona_selection_state(&conversation.id)
            .expect("tombstone");
        assert!(tombstone.selection.is_none());
        assert_eq!(tombstone.revision, Some(2));
        assert!(tombstone.selected_persona_revision_id.is_none());
        assert!(tombstone.cleared_at.is_some());

        let reselected = reopened
            .save_conversation_persona_selection(
                &ConversationPersonaSelection {
                    conversation_id: conversation.id.clone(),
                    persona_id: second.value.id.clone(),
                },
                tombstone.revision,
            )
            .expect("reselect after restart");
        assert_eq!(reselected.revision, 3);
        reopened
            .soft_delete_persona(&second.value.id, second.revision)
            .expect("delete selected persona");
        let deleted_state = reopened
            .get_conversation_persona_selection_state(&conversation.id)
            .expect("delete tombstone");
        assert!(deleted_state.selection.is_none());
        assert_eq!(deleted_state.revision, Some(4));
        assert!(
            reopened
                .save_conversation_persona_selection(
                    &ConversationPersonaSelection {
                        conversation_id: conversation.id,
                        persona_id: second.value.id,
                    },
                    deleted_state.revision,
                )
                .is_err()
        );
    }

    #[test]
    fn exact_selection_rejects_an_intervening_persona_revision() {
        let root = tempdir().expect("root");
        let storage = Storage::open(root.path()).expect("open");
        let conversation = seed_conversation(&storage);
        let original = storage
            .save_persona(&persona("persona-race"), None)
            .expect("original persona");
        let original_revision_id = original
            .revision_id
            .clone()
            .expect("original immutable revision");
        let mut edited = original.value.clone();
        edited.name = "Edited before selection".to_owned();
        edited.updated_at = Utc::now();
        storage
            .save_persona(&edited, Some(original.revision))
            .expect("intervening persona edit");

        let conflict = storage
            .save_conversation_persona_selection_at_revision(
                &ConversationPersonaSelection {
                    conversation_id: conversation.id.clone(),
                    persona_id: edited.id,
                },
                None,
                &original_revision_id,
            )
            .expect_err("selection must not silently pin the intervening edit");
        assert_eq!(conflict.code, CoreErrorCode::InvalidInput);
        assert!(conflict.recoverable);
        assert!(
            storage
                .get_conversation_persona_selection_state(&conversation.id)
                .expect("selection state")
                .revision
                .is_none(),
            "failed exact selection must leave no CAS row"
        );
    }
}

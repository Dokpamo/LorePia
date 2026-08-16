//! Safe local persona CRUD and conversation-scoped selection APIs.

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationId, ConversationPersonaSelection, CoreError, CoreErrorCode, CoreResult,
    LocalUserId, Persona, PersonaId, Provenance, Sha256Digest, SourceKind, ValidateOrchestration,
};
use lorepia_storage::{
    ConversationPersonaSelectionState, ObjectRevision, PersonaCatalogPage, StoredRevision,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Core;

const PERSONA_SCHEMA_VERSION: u32 = 1;
pub const MAX_PERSONA_LIST_LIMIT: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaCreateRequest {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaUpdateRequest {
    pub persona_id: PersonaId,
    pub expected_revision: u64,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaDeleteRequest {
    pub persona_id: PersonaId,
    pub expected_revision: u64,
}

/// Stable continuation boundary for the mutable local persona catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaListCursor {
    pub catalog_revision: Sha256Digest,
    pub updated_at: DateTime<Utc>,
    pub persona_id: PersonaId,
}

/// One bounded persona catalog page in `(updated_at DESC, id ASC)` order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersonaListPage {
    Page {
        catalog_revision: Sha256Digest,
        items: Vec<StoredRevision<Persona>>,
        next_cursor: Option<PersonaListCursor>,
    },
    RestartRequired {
        current_catalog_revision: Sha256Digest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationPersonaSelectionRequest {
    pub conversation_id: ConversationId,
    pub persona_id: PersonaId,
    /// `None` is valid only when no selection row has ever existed. After a
    /// clear or persona deletion callers use the tombstone revision returned
    /// by `get_conversation_persona_selection_state`.
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationPersonaClearRequest {
    pub conversation_id: ConversationId,
    pub expected_revision: u64,
}

impl Core {
    pub fn create_persona(
        &self,
        request: &PersonaCreateRequest,
    ) -> CoreResult<StoredRevision<Persona>> {
        let local_user_id = self.authoritative_local_user_id()?;
        let now = Utc::now();
        let stored = self.storage().save_persona(
            &Persona {
                id: PersonaId::from(Uuid::new_v4().to_string()),
                name: request.name.clone(),
                description: request.description.clone(),
                schema_version: PERSONA_SCHEMA_VERSION,
                provenance: Provenance {
                    source_kind: SourceKind::UserCreated,
                    source_id: Some(local_user_id.as_str().to_owned()),
                    source_hash: None,
                    author: None,
                    license: None,
                    imported_at: None,
                },
                created_at: now,
                updated_at: now,
            },
            None,
        )?;
        validate_owned_persona(&local_user_id, Some(&stored.value.id), &stored.value)?;
        Ok(stored)
    }

    pub fn get_persona(&self, persona_id: &PersonaId) -> CoreResult<StoredRevision<Persona>> {
        let local_user_id = self.authoritative_local_user_id()?;
        let stored = self.storage().get_persona(persona_id)?;
        validate_owned_persona(&local_user_id, Some(persona_id), &stored.value)?;
        Ok(stored)
    }

    pub fn list_personas(&self, limit: u32) -> CoreResult<Vec<StoredRevision<Persona>>> {
        match self.list_persona_page(None, limit)? {
            PersonaListPage::Page { items, .. } => Ok(items),
            PersonaListPage::RestartRequired { .. } => Err(CoreError::internal(
                "an initial persona catalog page unexpectedly required restart",
            )),
        }
    }

    /// Lists one bounded persona catalog page without making older personas
    /// unreachable once the catalog exceeds the single-page limit.
    pub fn list_persona_page(
        &self,
        after: Option<&PersonaListCursor>,
        limit: u32,
    ) -> CoreResult<PersonaListPage> {
        if !(1..=MAX_PERSONA_LIST_LIMIT).contains(&limit) {
            return Err(CoreError::invalid(format!(
                "persona list limit must be between 1 and {MAX_PERSONA_LIST_LIMIT}"
            )));
        }
        let local_user_id = self.authoritative_local_user_id()?;
        let query_limit = limit.saturating_add(1);
        let page = self.storage().list_personas_page(
            after.map(|cursor| &cursor.catalog_revision),
            after.map(|cursor| (&cursor.updated_at, &cursor.persona_id)),
            query_limit,
        )?;
        let (catalog_revision, mut items) = match page {
            PersonaCatalogPage::Page {
                catalog_revision,
                items,
            } => (catalog_revision, items),
            PersonaCatalogPage::RestartRequired {
                current_catalog_revision,
            } => {
                return Ok(PersonaListPage::RestartRequired {
                    current_catalog_revision,
                });
            }
        };
        for persona in &items {
            validate_owned_persona(&local_user_id, Some(&persona.value.id), &persona.value)?;
        }
        let has_more = items.len() > limit as usize;
        if has_more {
            items.truncate(limit as usize);
        }
        let next_cursor =
            has_more
                .then(|| items.last())
                .flatten()
                .map(|persona| PersonaListCursor {
                    catalog_revision: catalog_revision.clone(),
                    updated_at: persona.updated_at,
                    persona_id: persona.value.id.clone(),
                });
        Ok(PersonaListPage::Page {
            catalog_revision,
            items,
            next_cursor,
        })
    }

    pub fn update_persona(
        &self,
        request: &PersonaUpdateRequest,
    ) -> CoreResult<StoredRevision<Persona>> {
        let local_user_id = self.authoritative_local_user_id()?;
        let current = self.storage().get_persona(&request.persona_id)?;
        validate_owned_persona(&local_user_id, Some(&request.persona_id), &current.value)?;
        let mut persona = current.value;
        persona.name.clone_from(&request.name);
        persona.description.clone_from(&request.description);
        persona.updated_at = Utc::now();
        let stored = self
            .storage()
            .save_persona(&persona, Some(request.expected_revision))?;
        validate_owned_persona(&local_user_id, Some(&request.persona_id), &stored.value)?;
        Ok(stored)
    }

    pub fn list_persona_revisions(
        &self,
        persona_id: &PersonaId,
    ) -> CoreResult<Vec<ObjectRevision<Persona>>> {
        let local_user_id = self.authoritative_local_user_id()?;
        let current = self.storage().get_persona(persona_id)?;
        validate_owned_persona(&local_user_id, Some(persona_id), &current.value)?;
        let revisions = self.storage().list_persona_revisions(persona_id)?;
        for revision in &revisions {
            validate_persona_revision(&local_user_id, persona_id, revision)?;
        }
        Ok(revisions)
    }

    pub fn get_persona_revision(
        &self,
        persona_id: &PersonaId,
        revision_id: &str,
    ) -> CoreResult<ObjectRevision<Persona>> {
        let local_user_id = self.authoritative_local_user_id()?;
        let revision = self
            .storage()
            .get_persona_revision(persona_id, revision_id)?;
        validate_persona_revision(&local_user_id, persona_id, &revision)?;
        Ok(revision)
    }

    pub fn delete_persona(
        &self,
        request: &PersonaDeleteRequest,
    ) -> CoreResult<StoredRevision<Persona>> {
        let local_user_id = self.authoritative_local_user_id()?;
        let current = self.storage().get_persona(&request.persona_id)?;
        validate_owned_persona(&local_user_id, Some(&request.persona_id), &current.value)?;
        self.storage()
            .soft_delete_persona(&request.persona_id, request.expected_revision)
    }

    pub fn get_conversation_persona_selection_state(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<ConversationPersonaSelectionState> {
        let local_user_id = self.validate_persona_conversation_context(conversation_id)?;
        let state = self
            .storage()
            .get_conversation_persona_selection_state(conversation_id)?;
        validate_selection_state_shape(conversation_id, &state)?;
        if let Some(selection) = state.selection.as_ref() {
            let revision_id = state
                .selected_persona_revision_id
                .as_deref()
                .ok_or_else(|| {
                    storage_corrupted(
                        "active conversation persona selection has no immutable revision",
                    )
                })?;
            let revision = self
                .storage()
                .get_persona_revision(&selection.persona_id, revision_id)?;
            validate_persona_revision(&local_user_id, &selection.persona_id, &revision)?;
        }
        Ok(state)
    }

    pub fn select_conversation_persona(
        &self,
        request: &ConversationPersonaSelectionRequest,
    ) -> CoreResult<ConversationPersonaSelectionState> {
        let local_user_id = self.validate_persona_conversation_context(&request.conversation_id)?;
        let persona = self.storage().get_persona(&request.persona_id)?;
        validate_owned_persona(&local_user_id, Some(&request.persona_id), &persona.value)?;
        let persona_revision_id = persona
            .revision_id
            .as_deref()
            .ok_or_else(|| storage_corrupted("active persona has no immutable content revision"))?;
        let stored = self
            .storage()
            .save_conversation_persona_selection_at_revision(
                &ConversationPersonaSelection {
                    conversation_id: request.conversation_id.clone(),
                    persona_id: request.persona_id.clone(),
                },
                request.expected_revision,
                persona_revision_id,
            )?;
        active_selection_state(stored)
    }

    pub fn clear_conversation_persona(
        &self,
        request: &ConversationPersonaClearRequest,
    ) -> CoreResult<ConversationPersonaSelectionState> {
        // The read validates the authoritative conversation/character/local
        // user context and the ownership of the exact pinned persona revision.
        // The following storage CAS rejects any intervening selection change.
        self.get_conversation_persona_selection_state(&request.conversation_id)?;
        let stored = self.storage().clear_conversation_persona_selection(
            &request.conversation_id,
            request.expected_revision,
        )?;
        cleared_selection_state(stored)
    }

    fn authoritative_local_user_id(&self) -> CoreResult<LocalUserId> {
        let local_user_id = self.storage().load_settings()?.local_user_id;
        let parsed = Uuid::parse_str(local_user_id.as_str())
            .map_err(|_| storage_corrupted("stored local user id is not a canonical UUID"))?;
        if parsed.get_version_num() != 4
            || parsed.hyphenated().to_string() != local_user_id.as_str()
        {
            return Err(storage_corrupted(
                "stored local user id must be a lowercase canonical UUID v4",
            ));
        }
        Ok(local_user_id)
    }

    fn validate_persona_conversation_context(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<LocalUserId> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        if conversation.id != *conversation_id {
            return Err(storage_corrupted(
                "stored conversation identity does not match its lookup key",
            ));
        }
        let character = self.storage().get_character(&conversation.character_id)?;
        if character.id != conversation.character_id {
            return Err(storage_corrupted(
                "stored conversation character identity is inconsistent",
            ));
        }
        self.authoritative_local_user_id()
    }
}

fn validate_owned_persona(
    local_user_id: &LocalUserId,
    expected_id: Option<&PersonaId>,
    persona: &Persona,
) -> CoreResult<()> {
    persona
        .validate()
        .map_err(|error| storage_corrupted(format!("stored persona is invalid: {error}")))?;
    if expected_id.is_some_and(|expected_id| expected_id != &persona.id) {
        return Err(storage_corrupted(
            "stored persona identity does not match its lookup key",
        ));
    }
    if persona.schema_version != PERSONA_SCHEMA_VERSION {
        return Err(storage_corrupted(format!(
            "stored local persona schema version must be {PERSONA_SCHEMA_VERSION}"
        )));
    }
    if persona.provenance.source_kind != SourceKind::UserCreated
        || persona.provenance.source_id.as_deref() != Some(local_user_id.as_str())
    {
        return Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "persona is not owned by the current local user",
            false,
        ));
    }
    Ok(())
}

fn validate_persona_revision(
    local_user_id: &LocalUserId,
    persona_id: &PersonaId,
    revision: &ObjectRevision<Persona>,
) -> CoreResult<()> {
    if revision.object_kind != "persona"
        || revision.object_id != persona_id.as_str()
        || revision.value.id != *persona_id
    {
        return Err(storage_corrupted(
            "immutable persona revision identity does not match its owner",
        ));
    }
    validate_owned_persona(local_user_id, Some(persona_id), &revision.value)
}

fn active_selection_state(
    stored: StoredRevision<ConversationPersonaSelection>,
) -> CoreResult<ConversationPersonaSelectionState> {
    if stored.deleted_at.is_some() {
        return Err(storage_corrupted(
            "new persona selection was unexpectedly stored as a tombstone",
        ));
    }
    let selected_persona_revision_id = stored.revision_id.ok_or_else(|| {
        storage_corrupted("new persona selection has no immutable persona revision")
    })?;
    Ok(ConversationPersonaSelectionState {
        conversation_id: stored.value.conversation_id.clone(),
        selection: Some(stored.value),
        revision: Some(stored.revision),
        selected_persona_revision_id: Some(selected_persona_revision_id),
        updated_at: Some(stored.updated_at),
        cleared_at: None,
    })
}

fn cleared_selection_state(
    stored: StoredRevision<ConversationPersonaSelection>,
) -> CoreResult<ConversationPersonaSelectionState> {
    let cleared_at = stored.deleted_at.ok_or_else(|| {
        storage_corrupted("cleared persona selection was not stored as a tombstone")
    })?;
    Ok(ConversationPersonaSelectionState {
        conversation_id: stored.value.conversation_id,
        selection: None,
        revision: Some(stored.revision),
        selected_persona_revision_id: None,
        updated_at: Some(stored.updated_at),
        cleared_at: Some(cleared_at),
    })
}

fn validate_selection_state_shape(
    conversation_id: &ConversationId,
    state: &ConversationPersonaSelectionState,
) -> CoreResult<()> {
    if state.conversation_id != *conversation_id {
        return Err(storage_corrupted(
            "persona selection state belongs to another conversation",
        ));
    }
    match (
        state.selection.as_ref(),
        state.revision,
        state.selected_persona_revision_id.as_ref(),
        state.updated_at.as_ref(),
        state.cleared_at.as_ref(),
    ) {
        (None, None, None, None, None) => Ok(()),
        (Some(selection), Some(revision), Some(_), Some(_), None)
            if revision > 0 && selection.conversation_id == *conversation_id =>
        {
            Ok(())
        }
        (None, Some(revision), None, Some(updated_at), Some(cleared_at))
            if revision > 0 && updated_at == cleared_at =>
        {
            Ok(())
        }
        _ => Err(storage_corrupted(
            "stored persona selection state has an inconsistent active or tombstone shape",
        )),
    }
}

fn storage_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{PersonaCreateRequest, PersonaUpdateRequest};

    #[test]
    fn persona_write_requests_reject_storage_owned_fields() {
        assert!(
            serde_json::from_value::<PersonaCreateRequest>(json!({
                "name": "Local",
                "description": "",
                "id": "caller-controlled"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<PersonaUpdateRequest>(json!({
                "persona_id": "persona",
                "expected_revision": 1,
                "name": "Local",
                "description": "",
                "provenance": {
                    "source_kind": "user_created"
                }
            }))
            .is_err()
        );
    }
}

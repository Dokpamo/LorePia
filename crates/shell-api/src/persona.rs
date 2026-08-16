//! UI-safe local persona CRUD and conversation-scoped selection projections.
//!
//! Core owns local-user provenance, immutable persona revisions, conversation
//! validation, and compare-and-swap selection state. The webview sees only
//! creator-editable text plus opaque identifiers and exact revisions.

use chrono::{DateTime, Utc};
use lorepia_core::{
    ConversationId, ConversationPersonaClearRequest, ConversationPersonaSelectionRequest,
    ConversationPersonaSelectionState, CoreError, CoreErrorCode, ObjectRevision, Persona,
    PersonaCreateRequest, PersonaDeleteRequest, PersonaId, PersonaListCursor, PersonaListPage,
    PersonaUpdateRequest, Sha256Digest, StoredRevision,
};
use serde::{Deserialize, Serialize};

use crate::{ShellApi, ShellError, ShellResult, api::validate_identifier};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaDocumentDto {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaDto {
    pub value: PersonaDocumentDto,
    pub revision: u64,
    pub revision_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaPageCursorDto {
    pub catalog_revision: String,
    pub updated_at: DateTime<Utc>,
    pub persona_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersonaListPageDto {
    Page {
        catalog_revision: String,
        items: Vec<PersonaDto>,
        next_cursor: Option<PersonaPageCursorDto>,
    },
    RestartRequired {
        current_catalog_revision: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedPersonaSnapshotDto {
    pub value: PersonaDocumentDto,
    pub revision: u64,
    pub revision_id: String,
    /// Time at which this immutable content snapshot was written.
    pub snapshot_created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationPersonaSelectionDto {
    pub conversation_id: String,
    /// Compare-and-swap revision of the conversation selection row.
    ///
    /// This remains present after a clear so the next selection cannot race a
    /// stale restarted client.
    pub state_revision: Option<u64>,
    /// Exact immutable persona content selected for this conversation.
    pub selected_persona: Option<SelectedPersonaSnapshotDto>,
    pub updated_at: Option<DateTime<Utc>>,
    pub cleared_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaDeletionReceiptDto {
    pub persona_id: String,
    pub revision: u64,
    pub deleted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePersonaInput {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePersonaInput {
    pub persona_id: String,
    pub expected_revision: u64,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetPersonaInput {
    pub persona_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListPersonasInput {
    pub limit: u32,
    #[serde(default)]
    pub after: Option<PersonaPageCursorDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeletePersonaInput {
    pub persona_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetConversationPersonaSelectionInput {
    pub conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectConversationPersonaInput {
    pub conversation_id: String,
    pub persona_id: String,
    /// `None` is valid only before the conversation has ever stored a
    /// selection. A cleared selection exposes its tombstone revision here.
    pub expected_state_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClearConversationPersonaInput {
    pub conversation_id: String,
    pub expected_state_revision: u64,
}

impl ShellApi {
    pub fn create_persona(&self, input: CreatePersonaInput) -> ShellResult<PersonaDto> {
        self.core
            .create_persona(&PersonaCreateRequest {
                name: input.name,
                description: input.description,
            })
            .map_err(ShellError::from)
            .and_then(project_stored_persona)
    }

    pub fn update_persona(&self, input: UpdatePersonaInput) -> ShellResult<PersonaDto> {
        validate_identifier("persona_id", &input.persona_id)?;
        self.core
            .update_persona(&PersonaUpdateRequest {
                persona_id: PersonaId::from(input.persona_id),
                expected_revision: input.expected_revision,
                name: input.name,
                description: input.description,
            })
            .map_err(ShellError::from)
            .and_then(project_stored_persona)
    }

    pub fn get_persona(&self, input: GetPersonaInput) -> ShellResult<PersonaDto> {
        validate_identifier("persona_id", &input.persona_id)?;
        self.core
            .get_persona(&PersonaId::from(input.persona_id))
            .map_err(ShellError::from)
            .and_then(project_stored_persona)
    }

    pub fn list_personas(&self, input: ListPersonasInput) -> ShellResult<Vec<PersonaDto>> {
        match self.list_persona_page(input)? {
            PersonaListPageDto::Page { items, .. } => Ok(items),
            PersonaListPageDto::RestartRequired { .. } => Err(ShellError::from(
                CoreError::invalid("persona catalog cursor is stale; restart from the first page"),
            )),
        }
    }

    pub fn list_persona_page(&self, input: ListPersonasInput) -> ShellResult<PersonaListPageDto> {
        let after = input
            .after
            .map(|cursor| -> ShellResult<PersonaListCursor> {
                validate_identifier("persona_id", &cursor.persona_id)?;
                if cursor.catalog_revision.len() != 64
                    || !cursor
                        .catalog_revision
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(ShellError::from(CoreError::invalid(
                        "catalog_revision is not an exact lowercase SHA-256 digest",
                    )));
                }
                let catalog_revision =
                    Sha256Digest::parse(cursor.catalog_revision).map_err(|_| {
                        ShellError::from(CoreError::invalid(
                            "catalog_revision is not a canonical SHA-256 digest",
                        ))
                    })?;
                Ok(PersonaListCursor {
                    catalog_revision,
                    updated_at: cursor.updated_at,
                    persona_id: PersonaId::from(cursor.persona_id),
                })
            })
            .transpose()?;
        let page = self
            .core
            .list_persona_page(after.as_ref(), input.limit)
            .map_err(ShellError::from)?;
        match page {
            PersonaListPage::Page {
                catalog_revision,
                items,
                next_cursor,
            } => {
                let next_cursor = next_cursor.map(|cursor| PersonaPageCursorDto {
                    catalog_revision: cursor.catalog_revision.into_inner(),
                    updated_at: cursor.updated_at,
                    persona_id: cursor.persona_id.0,
                });
                let items = items
                    .into_iter()
                    .map(project_stored_persona)
                    .collect::<ShellResult<Vec<_>>>()?;
                Ok(PersonaListPageDto::Page {
                    catalog_revision: catalog_revision.into_inner(),
                    items,
                    next_cursor,
                })
            }
            PersonaListPage::RestartRequired {
                current_catalog_revision,
            } => Ok(PersonaListPageDto::RestartRequired {
                current_catalog_revision: current_catalog_revision.into_inner(),
            }),
        }
    }

    pub fn delete_persona(
        &self,
        input: DeletePersonaInput,
    ) -> ShellResult<PersonaDeletionReceiptDto> {
        validate_identifier("persona_id", &input.persona_id)?;
        let persona_id = PersonaId::from(input.persona_id);
        let deleted = self
            .core
            .delete_persona(&PersonaDeleteRequest {
                persona_id: persona_id.clone(),
                expected_revision: input.expected_revision,
            })
            .map_err(ShellError::from)?;
        let deleted_at = deleted.deleted_at.ok_or_else(|| {
            storage_corrupted("deleted persona did not produce a tombstone timestamp")
        })?;
        Ok(PersonaDeletionReceiptDto {
            persona_id: persona_id.0,
            revision: deleted.revision,
            deleted_at,
        })
    }

    pub fn get_conversation_persona_selection(
        &self,
        input: GetConversationPersonaSelectionInput,
    ) -> ShellResult<ConversationPersonaSelectionDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        let state = self
            .core
            .get_conversation_persona_selection_state(&ConversationId(input.conversation_id))
            .map_err(ShellError::from)?;
        self.project_conversation_persona_selection(state)
    }

    pub fn select_conversation_persona(
        &self,
        input: SelectConversationPersonaInput,
    ) -> ShellResult<ConversationPersonaSelectionDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("persona_id", &input.persona_id)?;
        let state = self
            .core
            .select_conversation_persona(&ConversationPersonaSelectionRequest {
                conversation_id: ConversationId(input.conversation_id),
                persona_id: PersonaId::from(input.persona_id),
                expected_revision: input.expected_state_revision,
            })
            .map_err(ShellError::from)?;
        self.project_conversation_persona_selection(state)
    }

    pub fn clear_conversation_persona(
        &self,
        input: ClearConversationPersonaInput,
    ) -> ShellResult<ConversationPersonaSelectionDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        let state = self
            .core
            .clear_conversation_persona(&ConversationPersonaClearRequest {
                conversation_id: ConversationId(input.conversation_id),
                expected_revision: input.expected_state_revision,
            })
            .map_err(ShellError::from)?;
        self.project_conversation_persona_selection(state)
    }

    fn project_conversation_persona_selection(
        &self,
        state: ConversationPersonaSelectionState,
    ) -> ShellResult<ConversationPersonaSelectionDto> {
        let selected_persona = match (
            state.selection.as_ref(),
            state.selected_persona_revision_id.as_deref(),
        ) {
            (Some(selection), Some(revision_id)) => Some(project_persona_snapshot(
                self.core
                    .get_persona_revision(&selection.persona_id, revision_id)
                    .map_err(ShellError::from)?,
            )?),
            (None, None) => None,
            _ => {
                return Err(storage_corrupted(
                    "persona selection did not identify one exact immutable persona revision",
                ));
            }
        };
        Ok(ConversationPersonaSelectionDto {
            conversation_id: state.conversation_id.0,
            state_revision: state.revision,
            selected_persona,
            updated_at: state.updated_at,
            cleared_at: state.cleared_at,
        })
    }
}

fn project_persona_document(value: Persona) -> PersonaDocumentDto {
    PersonaDocumentDto {
        id: value.id.0,
        name: value.name,
        description: value.description,
    }
}

fn project_stored_persona(value: StoredRevision<Persona>) -> ShellResult<PersonaDto> {
    if value.deleted_at.is_some() {
        return Err(storage_corrupted(
            "active persona projection unexpectedly contained a tombstone",
        ));
    }
    let revision_id = value
        .revision_id
        .ok_or_else(|| storage_corrupted("active persona has no immutable revision identifier"))?;
    Ok(PersonaDto {
        value: project_persona_document(value.value),
        revision: value.revision,
        revision_id,
        created_at: value.created_at,
        updated_at: value.updated_at,
    })
}

fn project_persona_snapshot(
    value: ObjectRevision<Persona>,
) -> ShellResult<SelectedPersonaSnapshotDto> {
    if value.object_kind != "persona" || value.object_id != value.value.id.as_str() {
        return Err(storage_corrupted(
            "immutable persona snapshot identity is inconsistent",
        ));
    }
    Ok(SelectedPersonaSnapshotDto {
        value: project_persona_document(value.value),
        revision: value.revision,
        revision_id: value.revision_id,
        snapshot_created_at: value.created_at,
    })
}

fn storage_corrupted(message: impl Into<String>) -> ShellError {
    ShellError::from(CoreError::new(
        CoreErrorCode::StorageCorrupted,
        message,
        false,
    ))
}

#[cfg(test)]
mod tests {
    use lorepia_core::{Core, CoreConfig};
    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        CreatePersonaInput, DeletePersonaInput, ListPersonasInput, PersonaDocumentDto,
        PersonaListPageDto, UpdatePersonaInput,
    };
    use crate::{ShellApi, ShellErrorCode};

    #[test]
    fn persona_documents_reject_security_owned_fields() {
        let value = json!({
            "id": "persona",
            "name": "Name",
            "description": "Description",
            "schema_version": 1,
            "provenance": {
                "source_kind": "user_created"
            }
        });
        assert!(serde_json::from_value::<PersonaDocumentDto>(value).is_err());
    }

    #[test]
    fn persona_crud_exposes_safe_exact_revisions() {
        let root = tempdir().expect("temporary data root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let shell = ShellApi::from_core(core);

        let created = shell
            .create_persona(CreatePersonaInput {
                name: "Narrator".to_owned(),
                description: "A concise local persona.".to_owned(),
            })
            .expect("create persona");
        assert_eq!(created.revision, 1);
        assert!(!created.revision_id.is_empty());

        let updated = shell
            .update_persona(UpdatePersonaInput {
                persona_id: created.value.id.clone(),
                expected_revision: created.revision,
                name: "Narrator".to_owned(),
                description: "An updated local persona.".to_owned(),
            })
            .expect("update persona");
        assert_eq!(updated.revision, 2);
        assert_ne!(updated.revision_id, created.revision_id);

        let personas = shell
            .list_personas(super::ListPersonasInput {
                limit: 100,
                after: None,
            })
            .expect("list personas");
        assert_eq!(personas, vec![updated.clone()]);

        let stale = shell
            .delete_persona(DeletePersonaInput {
                persona_id: updated.value.id.clone(),
                expected_revision: created.revision,
            })
            .expect_err("stale delete must fail");
        assert_eq!(stale.code, ShellErrorCode::InvalidInput);

        let deleted = shell
            .delete_persona(DeletePersonaInput {
                persona_id: updated.value.id,
                expected_revision: updated.revision,
            })
            .expect("delete persona");
        assert_eq!(deleted.revision, 3);
    }

    #[test]
    fn persona_page_dto_preserves_the_continuation_boundary() {
        let root = tempdir().expect("temporary data root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let shell = ShellApi::from_core(core);
        for name in ["First", "Second"] {
            shell
                .create_persona(CreatePersonaInput {
                    name: name.to_owned(),
                    description: String::new(),
                })
                .expect("create paged persona");
        }

        let legacy_input = serde_json::from_value::<ListPersonasInput>(json!({ "limit": 1 }))
            .expect("legacy list input");
        assert!(legacy_input.after.is_none());
        let first = shell
            .list_persona_page(legacy_input)
            .expect("first persona DTO page");
        let encoded = serde_json::to_value(&first).expect("persona page DTO must serialize");
        assert_eq!(encoded["kind"], "page");
        assert_eq!(encoded["catalog_revision"].as_str().map(str::len), Some(64));
        let PersonaListPageDto::Page {
            catalog_revision,
            items: first_items,
            next_cursor,
        } = first
        else {
            panic!("an initial persona page cannot require restart");
        };
        assert_eq!(catalog_revision.len(), 64);
        assert_eq!(first_items.len(), 1);
        let next_cursor = next_cursor.expect("first page continuation");
        assert_eq!(next_cursor.catalog_revision, catalog_revision);
        let second = shell
            .list_persona_page(ListPersonasInput {
                limit: 1,
                after: Some(next_cursor),
            })
            .expect("second persona DTO page");
        let PersonaListPageDto::Page {
            items: second_items,
            next_cursor,
            ..
        } = second
        else {
            panic!("an unchanged persona page cannot require restart");
        };
        assert_eq!(second_items.len(), 1);
        assert!(next_cursor.is_none());
        assert_ne!(first_items[0].value.id, second_items[0].value.id);
    }

    #[test]
    fn persona_page_dto_returns_a_typed_restart_for_a_stale_catalog_cursor() {
        let root = tempdir().expect("temporary data root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let shell = ShellApi::from_core(core);
        for name in ["First", "Second"] {
            shell
                .create_persona(CreatePersonaInput {
                    name: name.to_owned(),
                    description: String::new(),
                })
                .expect("create paged persona");
        }

        let first = shell
            .list_persona_page(ListPersonasInput {
                limit: 1,
                after: None,
            })
            .expect("first persona DTO page");
        let PersonaListPageDto::Page {
            next_cursor: Some(cursor),
            ..
        } = first
        else {
            panic!("a full first page must expose a cursor");
        };
        let second = shell
            .list_persona_page(ListPersonasInput {
                limit: 1,
                after: Some(cursor.clone()),
            })
            .expect("second persona DTO page");
        let PersonaListPageDto::Page { mut items, .. } = second else {
            panic!("an unchanged catalog must continue normally");
        };
        let unseen = items.pop().expect("persona after the cursor");
        shell
            .update_persona(UpdatePersonaInput {
                persona_id: unseen.value.id,
                expected_revision: unseen.revision,
                name: "Moved before the cursor".to_owned(),
                description: String::new(),
            })
            .expect("move the unseen persona before the old cursor");

        let stale = shell
            .list_persona_page(ListPersonasInput {
                limit: 1,
                after: Some(cursor),
            })
            .expect("stale cursor must return a typed safe restart");
        let encoded = serde_json::to_value(&stale).expect("restart DTO must serialize");
        assert_eq!(encoded["kind"], "restart_required");
        let PersonaListPageDto::RestartRequired {
            current_catalog_revision,
        } = stale
        else {
            panic!("the mutated catalog must require restart");
        };
        assert_eq!(current_catalog_revision.len(), 64);
    }

    #[test]
    fn persona_page_dto_rejects_a_noncanonical_catalog_revision() {
        let root = tempdir().expect("temporary data root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let shell = ShellApi::from_core(core);
        let error = shell
            .list_persona_page(ListPersonasInput {
                limit: 1,
                after: Some(super::PersonaPageCursorDto {
                    catalog_revision: "A".repeat(64),
                    updated_at: chrono::Utc::now(),
                    persona_id: "persona-1".to_owned(),
                }),
            })
            .expect_err("uppercase catalog revisions are not canonical IPC input");
        assert_eq!(error.code, ShellErrorCode::InvalidInput);
    }
}

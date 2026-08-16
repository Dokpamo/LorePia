use std::{collections::BTreeSet, io::Write};

use chrono::Utc;
use lorepia_core::{
    ConversationId, ConversationMode, ConversationPersonaClearRequest,
    ConversationPersonaSelectionRequest, Core, CoreConfig, CoreErrorCode, PersonaCreateRequest,
    PersonaDeleteRequest, PersonaListPage, PersonaUpdateRequest,
};
use lorepia_domain::{Persona, PersonaId, Provenance, SourceKind};
use lorepia_storage::Storage;
use tempfile::{NamedTempFile, tempdir};
use uuid::Uuid;

fn import_synthetic_character(core: &Core) -> String {
    let mut source = NamedTempFile::new().expect("temporary synthetic character");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Ari","description":"Synthetic persona-test character."}}}}"#
    )
    .expect("write synthetic character");
    let review = core
        .inspect_import(source.path())
        .expect("inspect character");
    core.commit_import(&review.id).expect("commit character").id
}

fn create_persona(core: &Core, name: &str) -> lorepia_core::StoredRevision<Persona> {
    core.create_persona(&PersonaCreateRequest {
        name: name.to_owned(),
        description: format!("{name} description"),
    })
    .expect("create persona")
}

#[test]
fn persona_catalog_keyset_pages_recover_the_101st_persona() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    for index in 0..101 {
        create_persona(&core, &format!("Paged persona {index:03}"));
    }

    let first = core
        .list_persona_page(None, 100)
        .expect("first persona page");
    let PersonaListPage::Page {
        items: first_items,
        next_cursor: Some(cursor),
        ..
    } = first
    else {
        panic!("a full first page must expose a continuation cursor");
    };
    assert_eq!(first_items.len(), 100);

    let second = core
        .list_persona_page(Some(&cursor), 100)
        .expect("second persona page");
    let PersonaListPage::Page {
        items: second_items,
        next_cursor: None,
        ..
    } = second
    else {
        panic!("an unchanged catalog must reach its final page");
    };
    assert_eq!(second_items.len(), 1);

    let ids = first_items
        .iter()
        .chain(&second_items)
        .map(|persona| persona.value.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids.len(),
        101,
        "keyset pages must not skip or duplicate personas"
    );
}

#[test]
fn persona_catalog_cursor_requests_restart_when_an_unseen_persona_moves_before_it() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    for index in 0..101 {
        create_persona(&core, &format!("Mutable paged persona {index:03}"));
    }

    let first = core
        .list_persona_page(None, 100)
        .expect("first persona page");
    let PersonaListPage::Page {
        next_cursor: Some(cursor),
        ..
    } = first
    else {
        panic!("a full first page must expose a continuation cursor");
    };
    let second = core
        .list_persona_page(Some(&cursor), 100)
        .expect("page containing the unseen persona");
    let PersonaListPage::Page { items, .. } = second else {
        panic!("an unchanged catalog must continue normally");
    };
    let unseen = items.into_iter().next().expect("101st persona");

    core.update_persona(&PersonaUpdateRequest {
        persona_id: unseen.value.id,
        expected_revision: unseen.revision,
        name: "Moved before the old cursor".to_owned(),
        description: "This update changes the persona catalog revision.".to_owned(),
    })
    .expect("move the unseen persona to the front of the catalog");

    assert!(matches!(
        core.list_persona_page(Some(&cursor), 100)
            .expect("stale cursor is a typed safe-restart result"),
        PersonaListPage::RestartRequired { .. }
    ));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one contract fixture proves CRUD, ownership, CAS, and immutable revision behavior"
)]
fn persona_crud_preserves_local_ownership_and_exact_revisions() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let local_user_id = core
        .get_settings()
        .expect("settings")
        .local_user_id
        .as_str()
        .to_owned();

    let created = create_persona(&core, "Local persona");
    assert_eq!(created.revision, 1);
    assert!(created.revision_id.is_some());
    assert_eq!(
        created.value.provenance.source_kind,
        SourceKind::UserCreated
    );
    assert_eq!(
        created.value.provenance.source_id.as_deref(),
        Some(local_user_id.as_str())
    );
    assert!(Uuid::parse_str(created.value.id.as_str()).is_ok());
    assert_eq!(
        core.get_persona(&created.value.id)
            .expect("get persona")
            .value,
        created.value
    );
    let _second = create_persona(&core, "Second local persona");
    assert_eq!(
        core.list_personas(1).expect("bounded persona list").len(),
        1
    );
    assert_eq!(core.list_personas(100).expect("full persona list").len(), 2);
    assert_eq!(
        core.list_personas(0).expect_err("zero limit").code,
        CoreErrorCode::InvalidInput
    );
    assert_eq!(
        core.list_personas(101).expect_err("oversized limit").code,
        CoreErrorCode::InvalidInput
    );

    let updated = core
        .update_persona(&PersonaUpdateRequest {
            persona_id: created.value.id.clone(),
            expected_revision: created.revision,
            name: "Updated local persona".to_owned(),
            description: "Updated description".to_owned(),
        })
        .expect("update persona");
    assert_eq!(updated.revision, 2);
    assert_ne!(updated.revision_id, created.revision_id);
    assert_eq!(updated.value.created_at, created.value.created_at);
    assert_eq!(updated.value.provenance, created.value.provenance);

    let stale = core
        .update_persona(&PersonaUpdateRequest {
            persona_id: created.value.id.clone(),
            expected_revision: created.revision,
            name: "Stale overwrite".to_owned(),
            description: String::new(),
        })
        .expect_err("stale update must fail");
    assert_eq!(stale.code, CoreErrorCode::InvalidInput);
    assert!(stale.recoverable);
    assert_eq!(
        core.get_persona(&created.value.id)
            .expect("persona after stale update")
            .value
            .name,
        "Updated local persona"
    );

    let revisions = core
        .list_persona_revisions(&created.value.id)
        .expect("list persona revisions");
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0].revision, 1);
    assert_eq!(revisions[0].value.name, "Local persona");
    assert_eq!(revisions[1].revision, 2);
    let original = core
        .get_persona_revision(
            &created.value.id,
            created.revision_id.as_deref().expect("created revision id"),
        )
        .expect("get exact original revision");
    assert_eq!(original.value.name, "Local persona");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one durable lifecycle fixture proves pinning, clear, restart, reselection, deletion, and stale CAS"
)]
fn clear_restart_reselect_delete_and_stale_selection_cas_are_durable() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic persona conversation",
            ConversationMode::Chat,
        )
        .expect("create conversation");
    let persona = create_persona(&core, "Selected persona");

    let absent = core
        .get_conversation_persona_selection_state(&conversation.id)
        .expect("initial selection state");
    assert!(absent.selection.is_none());
    assert_eq!(absent.revision, None);

    let selected = core
        .select_conversation_persona(&ConversationPersonaSelectionRequest {
            conversation_id: conversation.id.clone(),
            persona_id: persona.value.id.clone(),
            expected_revision: None,
        })
        .expect("select persona");
    assert_eq!(selected.revision, Some(1));
    assert_eq!(
        selected
            .selection
            .as_ref()
            .expect("active selection")
            .persona_id,
        persona.value.id
    );
    assert_eq!(selected.selected_persona_revision_id, persona.revision_id);

    let updated = core
        .update_persona(&PersonaUpdateRequest {
            persona_id: persona.value.id.clone(),
            expected_revision: persona.revision,
            name: "Edited after selection".to_owned(),
            description: "The room must remain pinned to revision one.".to_owned(),
        })
        .expect("edit selected persona");
    let still_pinned = core
        .get_conversation_persona_selection_state(&conversation.id)
        .expect("pinned state");
    assert_eq!(
        still_pinned.selected_persona_revision_id, persona.revision_id,
        "editing a persona must not silently move an existing room selection"
    );

    let stale_select = core
        .select_conversation_persona(&ConversationPersonaSelectionRequest {
            conversation_id: conversation.id.clone(),
            persona_id: persona.value.id.clone(),
            expected_revision: None,
        })
        .expect_err("stale selection CAS");
    assert_eq!(stale_select.code, CoreErrorCode::InvalidInput);

    let cleared = core
        .clear_conversation_persona(&ConversationPersonaClearRequest {
            conversation_id: conversation.id.clone(),
            expected_revision: 1,
        })
        .expect("clear selection");
    assert!(cleared.selection.is_none());
    assert_eq!(cleared.revision, Some(2));
    assert!(cleared.selected_persona_revision_id.is_none());
    assert!(cleared.cleared_at.is_some());
    let stale_clear = core
        .clear_conversation_persona(&ConversationPersonaClearRequest {
            conversation_id: conversation.id.clone(),
            expected_revision: 1,
        })
        .expect_err("stale clear");
    assert_eq!(stale_clear.code, CoreErrorCode::InvalidInput);
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen Core");
    let tombstone = reopened
        .get_conversation_persona_selection_state(&conversation.id)
        .expect("restart tombstone");
    assert!(tombstone.selection.is_none());
    assert_eq!(tombstone.revision, Some(2));
    assert!(tombstone.selected_persona_revision_id.is_none());

    let reselected = reopened
        .select_conversation_persona(&ConversationPersonaSelectionRequest {
            conversation_id: conversation.id.clone(),
            persona_id: persona.value.id.clone(),
            expected_revision: tombstone.revision,
        })
        .expect("reselect from tombstone");
    assert_eq!(reselected.revision, Some(3));
    assert_eq!(reselected.selected_persona_revision_id, updated.revision_id);
    let stale_delete = reopened
        .delete_persona(&PersonaDeleteRequest {
            persona_id: persona.value.id.clone(),
            expected_revision: persona.revision,
        })
        .expect_err("stale persona delete");
    assert_eq!(stale_delete.code, CoreErrorCode::InvalidInput);
    assert!(stale_delete.recoverable);

    reopened
        .delete_persona(&PersonaDeleteRequest {
            persona_id: persona.value.id.clone(),
            expected_revision: updated.revision,
        })
        .expect("delete selected persona");
    let deleted_state = reopened
        .get_conversation_persona_selection_state(&conversation.id)
        .expect("selection tombstoned by persona deletion");
    assert!(deleted_state.selection.is_none());
    assert_eq!(deleted_state.revision, Some(4));
    assert!(deleted_state.selected_persona_revision_id.is_none());
    let deleted_select = reopened
        .select_conversation_persona(&ConversationPersonaSelectionRequest {
            conversation_id: conversation.id.clone(),
            persona_id: persona.value.id,
            expected_revision: deleted_state.revision,
        })
        .expect_err("deleted persona cannot be selected");
    assert_eq!(deleted_select.code, CoreErrorCode::NotFound);
    assert_eq!(
        reopened
            .get_conversation_persona_selection_state(&conversation.id)
            .expect("state after deleted selection")
            .revision,
        Some(4)
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one authority fixture proves foreign ownership and missing conversation rejection"
)]
fn persona_operations_validate_conversation_character_and_local_user_authority() {
    let root = tempdir().expect("temporary Core root");
    let storage = Storage::open(root.path()).expect("open Storage");
    let now = Utc::now();
    let foreign_id = PersonaId::from(Uuid::new_v4().to_string());
    storage
        .save_persona(
            &Persona {
                id: foreign_id.clone(),
                name: "Foreign persona".to_owned(),
                description: String::new(),
                schema_version: 1,
                provenance: Provenance {
                    source_kind: SourceKind::UserCreated,
                    source_id: Some(Uuid::new_v4().to_string()),
                    source_hash: None,
                    author: None,
                    license: None,
                    imported_at: None,
                },
                created_at: now,
                updated_at: now,
            },
            None,
        )
        .expect("seed foreign-owned persona");
    drop(storage);

    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    assert_eq!(
        core.get_persona(&foreign_id).expect_err("foreign get").code,
        CoreErrorCode::PermissionDenied
    );
    assert_eq!(
        core.list_personas(100).expect_err("foreign list").code,
        CoreErrorCode::PermissionDenied
    );
    assert_eq!(
        core.update_persona(&PersonaUpdateRequest {
            persona_id: foreign_id.clone(),
            expected_revision: 1,
            name: "Unauthorized edit".to_owned(),
            description: String::new(),
        })
        .expect_err("foreign update")
        .code,
        CoreErrorCode::PermissionDenied
    );
    assert_eq!(
        core.delete_persona(&PersonaDeleteRequest {
            persona_id: foreign_id.clone(),
            expected_revision: 1,
        })
        .expect_err("foreign delete")
        .code,
        CoreErrorCode::PermissionDenied
    );

    let character_id = import_synthetic_character(&core);
    let conversation = core
        .create_conversation(
            &character_id,
            "Ownership validation",
            ConversationMode::Chat,
        )
        .expect("create conversation");
    let conversation_id = conversation.id.clone();
    assert_eq!(
        core.select_conversation_persona(&ConversationPersonaSelectionRequest {
            conversation_id: conversation.id,
            persona_id: foreign_id.clone(),
            expected_revision: None,
        })
        .expect_err("foreign selection")
        .code,
        CoreErrorCode::PermissionDenied
    );

    let local = create_persona(&core, "Local validation persona");
    let missing_conversation = ConversationId(Uuid::new_v4().to_string());
    assert_eq!(
        core.get_conversation_persona_selection_state(&missing_conversation)
            .expect_err("missing conversation state")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        core.select_conversation_persona(&ConversationPersonaSelectionRequest {
            conversation_id: missing_conversation.clone(),
            persona_id: local.value.id,
            expected_revision: None,
        })
        .expect_err("missing conversation selection")
        .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        core.clear_conversation_persona(&ConversationPersonaClearRequest {
            conversation_id: missing_conversation,
            expected_revision: 1,
        })
        .expect_err("missing conversation clear")
        .code,
        CoreErrorCode::NotFound
    );

    drop(core);
    let storage = Storage::open(root.path()).expect("reopen Storage");
    storage
        .save_conversation_persona_selection(
            &lorepia_domain::ConversationPersonaSelection {
                conversation_id: conversation_id.clone(),
                persona_id: foreign_id,
            },
            None,
        )
        .expect("seed unauthorized active selection below Core");
    drop(storage);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen Core");
    assert_eq!(
        reopened
            .get_conversation_persona_selection_state(&conversation_id)
            .expect_err("foreign active state")
            .code,
        CoreErrorCode::PermissionDenied
    );
    assert_eq!(
        reopened
            .clear_conversation_persona(&ConversationPersonaClearRequest {
                conversation_id,
                expected_revision: 1,
            })
            .expect_err("foreign active clear")
            .code,
        CoreErrorCode::PermissionDenied
    );
}

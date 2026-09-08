use std::io::Write;

use lorepia_core::{ConversationMode, Core, CoreConfig};
use tempfile::NamedTempFile;

#[test]
fn committed_character_greeting_can_fork_and_reopen_without_a_generation_attempt() {
    let root = tempfile::tempdir().expect("data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let mut source = NamedTempFile::new_in(root.path()).expect("synthetic card");
    source.write_all(br#"{"spec":"chara_card_v3","data":{"name":"Greeting branch","description":"Project-owned synthetic fixture.","first_mes":"A durable greeting."}}"#).expect("write card");
    let review = core.inspect_import(source.path()).expect("inspect card");
    let character = core.commit_import(&review.id).expect("import card");
    let catalog = core
        .get_character_greeting_catalog(&character.id)
        .expect("catalog");
    let start = core
        .create_conversation_with_greeting(
            &character.id,
            "Greeting fork",
            ConversationMode::Chat,
            catalog.character_content_revision_id.as_deref(),
            Some("default"),
        )
        .expect("conversation start");
    let greeting = start.initial_message.expect("greeting");
    let fork = core
        .create_conversation_branch(&start.conversation.id, Some(&greeting.id), None)
        .expect("fork at committed greeting without provider generation");
    assert_eq!(fork.head_message_id, Some(greeting.id.clone()));
    core.select_conversation_branch(&start.conversation.id, &fork.id)
        .expect("select greeting fork");
    let child = core
        .create_conversation_branch(&start.conversation.id, Some(&greeting.id), None)
        .expect("fork an inherited greeting on a child branch");
    assert_eq!(child.head_message_id, Some(greeting.id.clone()));
    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen Core");
    let messages = reopened
        .list_branch_messages(&fork.id)
        .expect("fork lineage");
    assert_eq!(messages, vec![greeting]);
}

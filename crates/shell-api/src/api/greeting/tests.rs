use crate::{ShellApi, ShellErrorCode, StagedImportFile};
use lorepia_core::CoreConfig;
use std::io::Write;
use tempfile::{NamedTempFile, tempdir};

#[test]
fn reader_returns_exact_full_opening_and_rejects_stale_or_noncanonical_selection() {
    let root = tempdir().unwrap();
    let shell = ShellApi::open(CoreConfig::new(root.path())).unwrap();
    let body = format!(
        "{}The final paragraph.",
        "A long synthetic scene. ".repeat(500)
    );
    let mut file = NamedTempFile::new().unwrap();
    write!(file, "{}", serde_json::json!({"spec":"chara_card_v3","data":{"name":"Reader","description":"Synthetic","first_mes":"Start", "alternate_greetings":["",body]}})).unwrap();
    let inspection = shell
        .inspect_import(&StagedImportFile::new(file.path()))
        .unwrap();
    let character = shell.commit_import(&inspection.inspection_id).unwrap();
    let catalog = shell.get_character_greeting_catalog(&character.id).unwrap();
    let revision = catalog.character_content_revision_id.unwrap();
    let detail = shell
        .get_character_greeting_detail(&character.id, &revision, "alternate-1")
        .unwrap();
    assert_eq!(detail.text, body);
    assert_eq!(detail.greeting_id, "alternate-1");
    assert_eq!(detail.character_content_revision_id, revision);
    assert_eq!(
        shell
            .get_character_greeting_detail(&character.id, "stale", "alternate-1")
            .unwrap_err()
            .code,
        ShellErrorCode::InvalidInput
    );
    for id in [
        "alternate-0",
        "alternate-01",
        "alternate-2",
        "alternate-+1",
        "default-0",
    ] {
        assert_eq!(
            shell
                .get_character_greeting_detail(&character.id, &revision, id)
                .unwrap_err()
                .code,
            ShellErrorCode::NotFound
        );
    }
    assert!(shell.list_conversations().unwrap().is_empty());
    assert_eq!(
        shell
            .get_character_greeting_detail(&character.id, &revision, "default")
            .unwrap()
            .text,
        "Start"
    );
}

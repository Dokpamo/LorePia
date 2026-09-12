use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;
use crate::ShellErrorCode;

const NOW: &str = "2026-09-12T00:00:00Z";
const CANONICAL: &str = "canonical terminal response";
const DISPLAY: &str = "display-only terminal response";

mod snapshots;

struct Fixture {
    root: TempDir,
    shell: ShellApi,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let shell = ShellApi::open_data_root(root.path()).unwrap();
        let fixture = Self { root, shell };
        let mut connection = fixture.connection();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        let transaction = connection.transaction().unwrap();
        transaction.execute("INSERT INTO content_sources(sha256,relative_path,size_bytes,created_at) VALUES(?1,'source.bin',1,?2)",params!["a".repeat(64),NOW]).unwrap();
        transaction.execute("INSERT INTO characters(id,name,description,source_hash,created_at) VALUES('character','synthetic','',?1,?2)",params!["a".repeat(64),NOW]).unwrap();
        transaction.execute("INSERT INTO conversations(id,character_id,title,created_at,updated_at) VALUES('room','character','synthetic',?1,?1)",[NOW]).unwrap();
        transaction.execute("INSERT INTO conversation_branches(id,conversation_id,created_at,updated_at) VALUES('main','room',?1,?1)",[NOW]).unwrap();
        for index in 0..2000 {
            let assistant = index == 1999;
            transaction.execute(
                "INSERT INTO messages(id,conversation_id,parent_id,role,content,status,generation_id,created_at) VALUES(?1,'room',?2,?3,?4,'complete',?5,?6)",
                params![format!("m{index}"), (index > 0).then(||format!("m{}",index-1)),if assistant {"assistant"} else {"user"},if assistant {CANONICAL.to_owned()} else {"x".repeat(1024)},assistant.then_some("generation"),NOW],
            ).unwrap();
        }
        transaction
            .execute(
                "UPDATE conversation_branches SET head_message_id='m1999' WHERE id='main'",
                [],
            )
            .unwrap();
        transaction.execute("INSERT INTO generations(id,conversation_id,branch_id,user_message_id,assistant_message_id,mode,model,status,started_at,finished_at) VALUES('generation','room','main','m1998','m1999','chat','synthetic','complete',?1,?1)",[NOW]).unwrap();
        transaction.execute("INSERT INTO message_display_projections(message_id,generation_id,canonical_content_sha256,display_content,display_content_sha256,pipeline_diagnostics_json,diagnostics_sha256,created_at) VALUES('m1999','generation',?1,?2,?3,?4,?5,?6)",
            params![digest(CANONICAL),DISPLAY,digest(DISPLAY),r#"{"schema_version":1,"failures":[]}"#,digest(r#"{"schema_version":1,"diagnostics":[]}"#),NOW],
        ).unwrap();
        transaction.commit().unwrap();
        fixture
    }

    fn connection(&self) -> Connection {
        let entries = std::fs::read_dir(self.root.path().join("db/schema-cutover")).unwrap();
        let (_, relative) = entries
            .map(Result::unwrap)
            .filter(|entry| entry.path().join("generation-committed.json").is_file())
            .map(|entry| {
                let bytes = std::fs::read(entry.path().join("generation-manifest.json")).unwrap();
                let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                (
                    manifest["activation_sequence"].as_u64().unwrap(),
                    manifest["active_database_relative_path"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                )
            })
            .max_by_key(|(sequence, _)| *sequence)
            .unwrap();
        let path = self.root.path().join(relative);
        assert!(
            path.canonicalize()
                .unwrap()
                .starts_with(self.root.path().canonicalize().unwrap())
        );
        assert!(
            path.exists(),
            "only the fresh synthetic database may be opened"
        );
        Connection::open(path).unwrap()
    }

    fn page(&self, before: Option<&str>, after: Option<&str>) -> BranchMessagesPageDto {
        self.shell
            .list_branch_messages_page(input(before, after))
            .unwrap()
    }
}

fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn input(before: Option<&str>, after: Option<&str>) -> ListBranchMessagesPageInput {
    ListBranchMessagesPageInput {
        branch_id: "main".into(),
        before_message_id: before.map(str::to_owned),
        after_message_id: after.map(str::to_owned),
        limit: 30,
        check_message_ids: None,
        include_last_assistant: false,
    }
}

#[test]
fn shell_pages_preserve_absolute_indexes_display_projection_and_legacy_results() {
    let fixture = Fixture::new();
    let latest = fixture.page(None, None);
    assert_eq!((latest.total_messages, latest.start_index), (2000, 1970));
    assert_eq!(latest.messages.len(), 30);
    assert_eq!(latest.messages.first().unwrap().id, "m1970");
    assert_eq!(latest.head_message_id.as_deref(), Some("m1999"));
    assert!(latest.has_older && !latest.has_newer);
    let terminal = latest.messages.last().unwrap();
    assert_eq!(terminal.content, DISPLAY);
    let projection = terminal.display_projection.as_ref().unwrap();
    assert_eq!(projection.canonical_content_sha256, digest(CANONICAL));
    assert_eq!(projection.display_content_sha256, digest(DISPLAY));
    assert!(projection.diagnostics.is_empty());
    let older = fixture.page(Some("m1970"), None);
    assert_eq!(older.start_index, 1940);
    assert_eq!(older.messages.last().unwrap().id, "m1969");
    assert_eq!(fixture.page(None, Some("m1969")), latest);
    let legacy = fixture.shell.list_branch_messages("main").unwrap();
    assert_eq!(legacy.len(), 2000);
    assert_eq!(legacy[1970..], latest.messages);
    let encoded = serde_json::to_value(&latest).unwrap();
    let mut keys = encoded
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "has_newer",
            "has_older",
            "head_message_id",
            "messages",
            "snapshot_token",
            "start_index",
            "total_messages"
        ]
    );
    let text = encoded.to_string();
    assert!(!text.contains(fixture.root.path().to_str().unwrap()));
    assert!(!text.contains("relative_path") && !text.contains("database_path"));
    assert_eq!(
        fixture
            .connection()
            .query_row("SELECT content FROM messages WHERE id='m1999'", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        CANONICAL
    );
}

#[test]
fn page_input_is_strict_and_invalid_limits_or_anchors_do_not_reach_history() {
    for value in [
        serde_json::json!({"branch_id":"main","limit":30,"unknown":true}),
        serde_json::json!({"limit":30}),
        serde_json::json!({"branch_id":"main"}),
        serde_json::json!({"branch_id":"main","limit":-1}),
    ] {
        assert!(serde_json::from_value::<ListBranchMessagesPageInput>(value).is_err());
    }
    let minimal: ListBranchMessagesPageInput =
        serde_json::from_value(serde_json::json!({"branch_id":"main","limit":30})).unwrap();
    assert!(minimal.before_message_id.is_none() && minimal.after_message_id.is_none());
    let fixture = Fixture::new();
    for limit in [0, 129, u32::MAX] {
        let error = fixture
            .shell
            .list_branch_messages_page(ListBranchMessagesPageInput {
                limit,
                ..input(None, None)
            })
            .unwrap_err();
        assert_eq!(error.code, ShellErrorCode::InvalidInput);
    }
    for invalid in ["", "line\nbreak"] {
        let error = fixture
            .shell
            .list_branch_messages_page(ListBranchMessagesPageInput {
                branch_id: invalid.into(),
                ..input(None, None)
            })
            .unwrap_err();
        assert_eq!(error.code, ShellErrorCode::InvalidInput);
        assert_eq!(
            fixture
                .shell
                .list_branch_messages_page(input(Some(invalid), None))
                .unwrap_err()
                .code,
            ShellErrorCode::InvalidInput
        );
    }
    assert_eq!(
        fixture
            .shell
            .list_branch_messages_page(input(Some("m1"), Some("m2")))
            .unwrap_err()
            .code,
        ShellErrorCode::InvalidInput
    );
    assert_eq!(
        fixture
            .shell
            .list_branch_messages_page(input(Some("missing"), None))
            .unwrap_err()
            .code,
        ShellErrorCode::InvalidInput
    );
    fixture
        .connection()
        .execute(
            "UPDATE conversation_branches SET head_message_id='m100' WHERE id='main'",
            [],
        )
        .unwrap();
    assert_eq!(
        fixture
            .shell
            .list_branch_messages_page(input(Some("m1999"), None))
            .unwrap_err()
            .code,
        ShellErrorCode::InvalidInput
    );
}

#[test]
fn selected_projection_tampering_fails_closed_but_does_not_poison_other_pages() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.page(None, None).messages.last().unwrap().content,
        DISPLAY
    );
    fixture.connection().execute_batch("DROP TRIGGER message_display_projections_no_update; UPDATE message_display_projections SET display_content='tampered' WHERE message_id='m1999';").unwrap();
    let error = fixture
        .shell
        .list_branch_messages_page(input(None, None))
        .unwrap_err();
    assert_eq!(error.code, ShellErrorCode::StorageCorrupted);
    assert_eq!(fixture.page(Some("m1970"), None).messages.len(), 30);
    let error_json = serde_json::to_string(&error).unwrap();
    assert!(!error_json.contains(fixture.root.path().to_str().unwrap()));
    assert!(!error_json.contains(CANONICAL));
}

#[test]
fn old_unselected_body_is_not_decoded_or_projected_through_core_and_shell() {
    let fixture = Fixture::new();
    fixture.connection().execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE messages SET status='invalid-status',content=zeroblob(4194304) WHERE id='m0'; PRAGMA ignore_check_constraints=OFF;").unwrap();
    let latest = fixture.page(None, None);
    assert_eq!(latest.messages.len(), 30);
    assert_eq!(latest.messages.last().unwrap().content, DISPLAY);
    assert!(
        fixture
            .shell
            .list_branch_messages_page(input(Some("m1"), None))
            .is_err()
    );
    assert!(fixture.shell.list_branch_messages("main").is_err());
}

#[test]
fn membership_evidence_is_optional_bounded_and_preserves_requested_order() {
    let fixture = Fixture::new();
    let page = fixture
        .shell
        .list_branch_messages_page(ListBranchMessagesPageInput {
            check_message_ids: Some(vec![
                "m0".into(),
                "absent".into(),
                "m1999".into(),
                "m0".into(),
            ]),
            ..input(None, None)
        })
        .unwrap();
    assert_eq!(
        page.retained_message_ids,
        Some(vec!["m0".into(), "m1999".into(), "m0".into()])
    );
    assert_eq!(page.messages.len(), 30);
    assert_eq!(page.start_index, 1970);
    let empty = fixture
        .shell
        .list_branch_messages_page(ListBranchMessagesPageInput {
            check_message_ids: Some(vec![]),
            ..input(None, None)
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(empty).unwrap()["retained_message_ids"],
        serde_json::json!([])
    );
    assert!(
        serde_json::to_value(fixture.page(None, None))
            .unwrap()
            .get("retained_message_ids")
            .is_none()
    );
    for ids in [
        vec!["m0".into(); 257],
        vec![String::new()],
        vec!["bad\nidentifier".into()],
        vec!["x".repeat(513)],
    ] {
        assert_eq!(
            fixture
                .shell
                .list_branch_messages_page(ListBranchMessagesPageInput {
                    check_message_ids: Some(ids),
                    ..input(None, None)
                })
                .unwrap_err()
                .code,
            ShellErrorCode::InvalidInput
        );
    }
    assert!(
        serde_json::from_value::<ListBranchMessagesPageInput>(
            serde_json::json!({"branch_id":"main","limit":30,"check_message_ids":[1]})
        )
        .is_err()
    );
}

#[test]
fn distant_last_assistant_keeps_display_only_validation_without_duplicate_page_content() {
    let fixture = Fixture::new();
    let load = |before: Option<&str>, include| {
        fixture
            .shell
            .list_branch_messages_page(ListBranchMessagesPageInput {
                include_last_assistant: include,
                limit: 128,
                ..input(before, None)
            })
    };
    assert!(load(None, true).unwrap().last_assistant_message.is_none());
    {
        let mut connection = fixture.connection();
        let transaction = connection.transaction().unwrap();
        for index in 2000..2200 {
            transaction.execute("INSERT INTO messages(id,conversation_id,parent_id,role,content,status,created_at) VALUES(?1,'room',?2,'user','tail','complete',?3)",params![format!("m{index}"),format!("m{}",index-1),NOW]).unwrap();
        }
        transaction
            .execute(
                "UPDATE conversation_branches SET head_message_id='m2199' WHERE id='main'",
                [],
            )
            .unwrap();
        transaction.commit().unwrap();
    }
    assert!(load(None, false).unwrap().last_assistant_message.is_none());
    let page = load(None, true).unwrap();
    assert_eq!(page.messages.len(), 128);
    let assistant = page.last_assistant_message.unwrap();
    assert_eq!(assistant.id, "m1999");
    assert_eq!(assistant.content, DISPLAY);
    assert_eq!(
        assistant
            .display_projection
            .unwrap()
            .canonical_content_sha256,
        digest(CANONICAL)
    );
    assert!(
        load(Some("m2000"), true)
            .unwrap()
            .last_assistant_message
            .is_none()
    );
    assert_eq!(
        load(Some("m1970"), true)
            .unwrap()
            .last_assistant_message
            .unwrap()
            .id,
        "m1999"
    );
    fixture.connection().execute_batch("DROP TRIGGER message_display_projections_no_update; UPDATE message_display_projections SET display_content='tampered' WHERE message_id='m1999';").unwrap();
    assert!(load(None, false).is_ok());
    assert_eq!(
        load(None, true).unwrap_err().code,
        ShellErrorCode::StorageCorrupted
    );
    assert!(
        serde_json::from_value::<ListBranchMessagesPageInput>(
            serde_json::json!({"branch_id":"main","limit":30,"include_last_assistant":"true"})
        )
        .is_err()
    );
}

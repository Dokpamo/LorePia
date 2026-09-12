use std::sync::{Arc, Barrier};

use super::*;
use tempfile::TempDir;

const NOW: &str = "2026-09-12T00:00:00Z";

struct Fixture {
    _root: TempDir,
    storage: Arc<Storage>,
}

impl Fixture {
    fn new(count: usize) -> Self {
        let root = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(root.path()).unwrap());
        {
            let mut connection = storage.connection().unwrap();
            let transaction = connection.transaction().unwrap();
            transaction.execute("INSERT INTO content_sources(sha256,relative_path,size_bytes,created_at) VALUES(?1,'source.bin',1,?2)",params!["a".repeat(64),NOW]).unwrap();
            transaction.execute("INSERT INTO characters(id,name,description,source_hash,created_at) VALUES('character','character','',?1,?2)", params!["a".repeat(64),NOW]).unwrap();
            for room in ["room", "foreign"] {
                transaction.execute("INSERT INTO conversations(id,character_id,title,created_at,updated_at) VALUES(?1,'character','',?2,?2)",params![room,NOW]).unwrap();
            }
            for index in 0..count {
                insert(
                    &transaction,
                    &format!("m{index}"),
                    index.checked_sub(1).map(|i| format!("m{i}")).as_deref(),
                    "room",
                );
            }
            insert(&transaction, "foreign-message", None, "foreign");
            for (branch, head) in [
                ("main", count.checked_sub(1).map(|i| format!("m{i}"))),
                ("empty", None),
            ] {
                transaction.execute("INSERT INTO conversation_branches(id,conversation_id,head_message_id,created_at,updated_at) VALUES(?1,'room',?2,?3,?3)",params![branch,head,NOW]).unwrap();
            }
            transaction.commit().unwrap();
        }
        Self {
            _root: root,
            storage,
        }
    }

    fn page(&self, before: Option<&str>, after: Option<&str>, limit: u32) -> BranchMessagePage {
        self.storage
            .list_branch_messages_page(
                &ConversationBranchId("main".into()),
                before.map(|s| MessageId(s.into())).as_ref(),
                after.map(|s| MessageId(s.into())).as_ref(),
                limit,
                None,
                false,
            )
            .unwrap()
    }
}

fn insert(connection: &Connection, id: &str, parent: Option<&str>, room: &str) {
    connection.execute("INSERT INTO messages(id,conversation_id,parent_id,role,content,status,created_at) VALUES(?1,?2,?3,'user',?4,'complete',?5)",params![id,room,parent,"x".repeat(1024),NOW]).unwrap();
}

fn ids(page: &BranchMessagePage) -> Vec<&str> {
    page.messages
        .iter()
        .map(|message| message.id.0.as_str())
        .collect()
}

#[test]
fn two_thousand_messages_page_in_both_directions_with_absolute_offsets() {
    let fixture = Fixture::new(2000);
    let latest = fixture.page(None, None, 30);
    assert_eq!(latest.messages.len(), 30);
    assert_eq!(latest.total_messages, 2000);
    assert_eq!(latest.start_index, 1970);
    assert_eq!(ids(&latest).first(), Some(&"m1970"));
    assert_eq!(ids(&latest).last(), Some(&"m1999"));
    assert_eq!(latest.head_message_id, Some(MessageId("m1999".into())));
    assert!(latest.has_older);
    assert!(!latest.has_newer);
    let older = fixture.page(Some("m1970"), None, 30);
    assert_eq!(older.start_index, 1940);
    assert_eq!(ids(&older).last(), Some(&"m1969"));
    assert!(older.has_older && older.has_newer);
    assert_eq!(fixture.page(None, Some("m1969"), 30), latest);
    let first = fixture.page(Some("m20"), None, 30);
    assert_eq!(first.messages.len(), 20);
    assert_eq!(first.start_index, 0);
    assert!(!first.has_older);
    assert!(first.has_newer);
    let before_first = fixture.page(Some("m0"), None, 30);
    assert!(before_first.messages.is_empty());
    assert_eq!(before_first.start_index, 0);
    let after_last = fixture.page(None, Some("m1999"), 30);
    assert!(after_last.messages.is_empty());
    assert_eq!(after_last.start_index, 2000);
    assert_eq!(fixture.page(None, None, 128).messages.len(), 128);
}

#[test]
fn anchors_are_exclusive_current_branch_members_and_limits_are_checked() {
    let fixture = Fixture::new(60);
    {
        let connection = fixture.storage.connection().unwrap();
        insert(&connection, "forked", Some("m10"), "room");
        connection.execute("INSERT INTO conversation_branches(id,conversation_id,head_message_id,created_at,updated_at) VALUES('fork','room','forked',?1,?1)",[NOW]).unwrap();
    }
    let branch = ConversationBranchId("main".into());
    for anchor in ["missing", "foreign-message", "forked"] {
        let error = fixture
            .storage
            .list_branch_messages_page(
                &branch,
                Some(&MessageId(anchor.into())),
                None,
                30,
                None,
                false,
            )
            .unwrap_err();
        assert_eq!(error.code, lorepia_domain::CoreErrorCode::InvalidInput);
    }
    for limit in [0, 129, u32::MAX] {
        assert!(
            fixture
                .storage
                .list_branch_messages_page(&branch, None, None, limit, None, false)
                .is_err()
        );
    }
    assert!(
        fixture
            .storage
            .list_branch_messages_page(
                &branch,
                Some(&MessageId("m1".into())),
                Some(&MessageId("m2".into())),
                30,
                None,
                false
            )
            .is_err()
    );
    let fork = fixture
        .storage
        .list_branch_messages_page(
            &ConversationBranchId("fork".into()),
            None,
            None,
            30,
            None,
            false,
        )
        .unwrap();
    assert_eq!(fork.total_messages, 12);
    assert_eq!(ids(&fork).last(), Some(&"forked"));
    assert!(
        fixture
            .storage
            .list_branch_messages_page(
                &ConversationBranchId("fork".into()),
                None,
                Some(&MessageId("m11".into())),
                30,
                None,
                false
            )
            .is_err()
    );
    let empty = fixture
        .storage
        .list_branch_messages_page(
            &ConversationBranchId("empty".into()),
            None,
            None,
            30,
            None,
            false,
        )
        .unwrap();
    assert!(empty.messages.is_empty());
    assert_eq!((empty.total_messages, empty.start_index), (0, 0));
    assert!(empty.head_message_id.is_none());
    assert!(!empty.has_newer && !empty.has_older);
    let missing = fixture
        .storage
        .list_branch_messages_page(
            &ConversationBranchId("missing".into()),
            None,
            None,
            30,
            None,
            false,
        )
        .unwrap_err();
    assert_eq!(missing.code, lorepia_domain::CoreErrorCode::NotFound);
}

#[test]
fn old_unselected_content_is_not_materialized_or_decoded() {
    let fixture = Fixture::new(2000);
    {
        let connection = fixture.storage.connection().unwrap();
        connection.execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE messages SET status='not-a-status',content=zeroblob(4194304) WHERE id='m0'; PRAGMA ignore_check_constraints=OFF;").unwrap();
    }
    let page = fixture.page(None, None, 30);
    assert_eq!(page.messages.len(), 30);
    assert_eq!(
        page.messages.iter().map(|m| m.content.len()).sum::<usize>(),
        30 * 1024
    );
    assert!(
        fixture
            .storage
            .list_branch_messages_page(
                &ConversationBranchId("main".into()),
                Some(&MessageId("m1".into())),
                None,
                30,
                None,
                false
            )
            .is_err()
    );
}

#[test]
fn concurrent_append_returns_one_coherent_head_and_count_snapshot() {
    let fixture = Fixture::new(2000);
    let barrier = Arc::new(Barrier::new(2));
    let writer_storage = fixture.storage.clone();
    let writer_barrier = barrier.clone();
    let writer = std::thread::spawn(move || {
        writer_barrier.wait();
        let mut connection = writer_storage.connection().unwrap();
        let transaction = connection.transaction().unwrap();
        insert(&transaction, "m2000", Some("m1999"), "room");
        transaction
            .execute(
                "UPDATE conversation_branches SET head_message_id='m2000' WHERE id='main'",
                [],
            )
            .unwrap();
        transaction.commit().unwrap();
    });
    barrier.wait();
    let page = fixture.page(None, None, 30);
    writer.join().unwrap();
    assert!(page.total_messages == 2000 || page.total_messages == 2001);
    assert_eq!(page.start_index, page.total_messages - 30);
    assert_eq!(
        page.head_message_id.as_ref(),
        page.messages.last().map(|m| &m.id)
    );
    assert_eq!(fixture.page(None, None, 30).total_messages, 2001);
}

#[test]
fn rewound_anchors_and_cyclic_ancestry_fail_closed() {
    let fixture = Fixture::new(60);
    let branch = ConversationBranchId("main".into());
    fixture
        .storage
        .connection()
        .unwrap()
        .execute(
            "UPDATE conversation_branches SET head_message_id='m10' WHERE id='main'",
            [],
        )
        .unwrap();
    let error = fixture
        .storage
        .list_branch_messages_page(
            &branch,
            Some(&MessageId("m59".into())),
            None,
            30,
            None,
            false,
        )
        .unwrap_err();
    assert_eq!(error.code, lorepia_domain::CoreErrorCode::InvalidInput);
    assert_eq!(fixture.page(None, None, 30).total_messages, 11);
    fixture
        .storage
        .connection()
        .unwrap()
        .execute("UPDATE messages SET parent_id='m10' WHERE id='m0'", [])
        .unwrap();
    let error = fixture
        .storage
        .list_branch_messages_page(&branch, None, None, 30, None, false)
        .unwrap_err();
    assert_eq!(error.code, lorepia_domain::CoreErrorCode::StorageCorrupted);
}

#[test]
fn membership_is_bounded_ordered_and_independent_of_page_bodies() {
    let fixture = Fixture::new(2000);
    let branch = ConversationBranchId("main".into());
    insert(
        &fixture.storage.connection().unwrap(),
        "sibling",
        Some("m0"),
        "room",
    );
    let requested = ["m0", "foreign-message", "sibling", "missing", "m1999", "m0"]
        .map(|id| MessageId(id.into()));
    let boundary_candidates = vec![MessageId("m0".into()); 256];
    assert_eq!(
        fixture
            .storage
            .list_branch_messages_page(&branch, None, None, 1, Some(&boundary_candidates), false)
            .unwrap()
            .retained_message_ids
            .unwrap()
            .len(),
        256
    );
    let page = fixture
        .storage
        .list_branch_messages_page(&branch, None, None, 30, Some(&requested), false)
        .unwrap();
    assert_eq!(
        page.retained_message_ids,
        Some(
            ["m0", "m1999", "m0"]
                .map(|id| MessageId(id.into()))
                .to_vec()
        )
    );
    assert_eq!(page.messages.len(), 30);
    assert_eq!(page.start_index, 1970);
    assert!(fixture.page(None, None, 30).retained_message_ids.is_none());
    assert_eq!(
        fixture
            .storage
            .list_branch_messages_page(&branch, None, None, 30, Some(&[]), false)
            .unwrap()
            .retained_message_ids,
        Some(vec![])
    );
    for requested in [
        vec![MessageId("m0".into()); 257],
        vec![MessageId(String::new())],
        vec![MessageId("bad\nidentifier".into())],
        vec![MessageId("x".repeat(513))],
    ] {
        assert_eq!(
            fixture
                .storage
                .list_branch_messages_page(&branch, None, None, 30, Some(&requested), false)
                .unwrap_err()
                .code,
            lorepia_domain::CoreErrorCode::InvalidInput
        );
    }
    fixture.storage.connection().unwrap().execute_batch(
        "PRAGMA ignore_check_constraints=ON; UPDATE messages SET status='invalid-status',content=zeroblob(4194304) WHERE id='m0'; PRAGMA ignore_check_constraints=OFF;",
    ).unwrap();
    assert_eq!(
        fixture
            .storage
            .list_branch_messages_page(&branch, None, None, 30, Some(&requested), false)
            .unwrap()
            .retained_message_ids,
        page.retained_message_ids
    );
}

#[test]
fn last_assistant_supplement_is_optional_outside_page_and_loads_only_latest_body() {
    let fixture = Fixture::new(2000);
    let branch = ConversationBranchId("main".into());
    let load = |before: Option<&MessageId>, include| {
        fixture
            .storage
            .list_branch_messages_page(&branch, before, None, 128, None, include)
            .unwrap()
    };
    assert!(load(None, true).last_assistant_message.is_none());
    fixture.storage.connection().unwrap().execute_batch(
        "UPDATE messages SET role='assistant',generation_id='old-generation',status='pending' WHERE id='m1';
         PRAGMA ignore_check_constraints=ON;
         UPDATE messages SET role='assistant',generation_id='corrupt-generation',content=zeroblob(4194304) WHERE id='m0';
         PRAGMA ignore_check_constraints=OFF;",
    ).unwrap();
    assert!(load(None, false).last_assistant_message.is_none());
    let page = load(None, true);
    assert_eq!(page.messages.len(), 128);
    let assistant = page.last_assistant_message.unwrap();
    assert_eq!(assistant.id.0, "m1");
    assert_eq!(assistant.status, lorepia_domain::MessageStatus::Pending);
    let included = fixture
        .storage
        .list_branch_messages_page(&branch, Some(&MessageId("m2".into())), None, 1, None, true)
        .unwrap();
    assert_eq!(included.messages[0].id.0, "m1");
    assert!(included.last_assistant_message.is_none());
}

use super::*;

#[test]
fn sql_trim_set_matches_rust_unicode_whitespace() {
    for character in (0..=0x0010_ffff).filter_map(char::from_u32) {
        assert_eq!(WHITE_SPACE.contains(character), character.is_whitespace());
    }
}

#[test]
fn long_branch_selects_only_source_bodies_and_preserves_identity_filtering() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(directory.path()).unwrap();
    let room = ConversationId("room".into());
    let branch = ConversationBranchId("main".into());
    {
        let mut connection = storage.connection().unwrap();
        let tx = connection.transaction().unwrap();
        tx.execute("INSERT INTO content_sources(sha256,relative_path,size_bytes,created_at) VALUES(?1,'source.bin',1,?2)", params!["a".repeat(64),NOW]).unwrap();
        tx.execute("INSERT INTO characters(id,name,description,source_hash,created_at) VALUES('character','character','',?1,?2)", params!["a".repeat(64),NOW]).unwrap();
        tx.execute("INSERT INTO conversations(id,character_id,title,created_at,updated_at) VALUES('room','character','',?1,?1)",[NOW]).unwrap();
        for index in 0usize..2000 {
            tx.execute("INSERT INTO messages(id,conversation_id,parent_id,role,content,status,created_at) VALUES(?1,'room',?2,'user',?3,'complete',?4)",
                params![format!("m{index}"),index.checked_sub(1).map(|i|format!("m{i}")),"x".repeat(1024),NOW]).unwrap();
        }
        tx.execute("INSERT INTO conversation_branches(id,conversation_id,head_message_id,created_at,updated_at) VALUES('main','room','m1999',?1,?1)",[NOW]).unwrap();
        tx.execute(
            "UPDATE messages SET content=?1 WHERE id='m0'",
            ["z".repeat(5 * 1024 * 1024)],
        )
        .unwrap();
        tx.execute(
            "UPDATE messages SET content=?1 WHERE id='m1'",
            [WHITE_SPACE],
        )
        .unwrap();
        tx.execute("UPDATE messages SET content=?1 WHERE id='m2'", ["\0"])
            .unwrap();
        tx.commit().unwrap();
    }
    let identities = storage
        .list_branch_memory_source_identities(&room, &branch)
        .unwrap();
    assert_eq!(identities.len(), 1999);
    assert_eq!(identities[1].id.0, "m2");
    let selected = storage
        .list_branch_memory_source(
            &room,
            &branch,
            &MessageId("m1970".into()),
            &MessageId("m1999".into()),
        )
        .unwrap();
    assert_eq!(selected.len(), 30);
    assert_eq!(
        selected.iter().map(|m| m.content.len()).sum::<usize>(),
        30 * 1024
    );
    assert!(
        storage
            .list_branch_memory_source(
                &room,
                &branch,
                &MessageId("m0".into()),
                &MessageId("m0".into())
            )
            .is_err()
    );
    assert!(
        storage
            .list_branch_memory_source(
                &room,
                &branch,
                &MessageId("m1".into()),
                &MessageId("m2".into())
            )
            .is_err()
    );
    assert!(
        storage
            .list_branch_memory_source(
                &room,
                &branch,
                &MessageId("m10".into()),
                &MessageId("m9".into())
            )
            .is_err()
    );
    // Off-range metadata is still decoded and corruption fails closed.
    storage
        .connection()
        .unwrap()
        .execute("UPDATE messages SET created_at='invalid' WHERE id='m0'", [])
        .unwrap();
    assert!(
        storage
            .list_branch_memory_source(
                &room,
                &branch,
                &MessageId("m1970".into()),
                &MessageId("m1999".into())
            )
            .is_err()
    );
}

const NOW: &str = "2026-09-12T00:00:00Z";

use crate::database::{ConversationBranchId, GenerationId, MessageId, Storage, params};

#[test]
fn hundred_thousand_messages_checkpoint_and_page_interleaving_reuses_one_validation() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(root.path()).unwrap();
    {
        let mut connection = storage.connection().unwrap();
        let transaction = connection.transaction().unwrap();
        transaction.execute_batch("INSERT INTO content_sources VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','source.bin',1,'2026-09-12T00:00:00+00:00');
          INSERT INTO characters(id,name,description,source_hash,created_at) VALUES('c','c','','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','2026-09-12T00:00:00+00:00');
          INSERT INTO conversations(id,character_id,title,created_at,updated_at) VALUES('room','c','','2026-09-12T00:00:00+00:00','2026-09-12T00:00:00+00:00');").unwrap();
        {
            let mut insert = transaction.prepare("INSERT INTO messages(id,conversation_id,parent_id,role,content,status,created_at) VALUES(?1,'room',?2,'user','','complete','2026-09-12T00:00:00+00:00')").unwrap();
            for index in 0usize..100_000 {
                insert
                    .execute(params![
                        format!("m{index}"),
                        index.checked_sub(1).map(|parent| format!("m{parent}"))
                    ])
                    .unwrap();
            }
        }
        transaction.execute_batch("UPDATE messages SET role='assistant',status='pending',generation_id='g' WHERE id='m99999';
          INSERT INTO conversation_branches(id,conversation_id,head_message_id,created_at,updated_at) VALUES('main','room','m99999','2026-09-12T00:00:00+00:00','2026-09-12T00:00:00+00:00');").unwrap();
        transaction.commit().unwrap();
    }
    let branch = ConversationBranchId("main".into());
    let candidates = [
        MessageId("m0".into()),
        MessageId("absent".into()),
        MessageId("m99999".into()),
        MessageId("m0".into()),
    ];
    let mut previous_token = None;
    for offset in 0..20 {
        let page = storage
            .list_branch_messages_page(
                &branch,
                Some(&MessageId("m50000".into())),
                None,
                30,
                Some(&candidates),
                false,
            )
            .unwrap();
        if let Some(previous) = previous_token.replace(page.snapshot_token.clone()) {
            assert_ne!(previous, page.snapshot_token);
        }
        assert_eq!(page.total_messages, 100_000);
        assert_eq!(page.start_index, 49_970);
        assert_eq!(page.messages.first().unwrap().id.0, "m49970");
        assert_eq!(
            page.retained_message_ids.unwrap(),
            [
                candidates[0].clone(),
                candidates[2].clone(),
                candidates[3].clone()
            ]
        );
        storage
            .append_pending_assistant_checkpoint(
                &MessageId("m99999".into()),
                &GenerationId("g".into()),
                offset,
                "x",
            )
            .unwrap();
        storage
            .connection()
            .unwrap()
            .execute(
                "UPDATE conversations SET title=?1",
                [format!("setting {offset}")],
            )
            .unwrap();
    }
    let cache = storage.lineage_cache.lock().unwrap();
    assert_eq!(cache.validations, 1);
    assert!(cache.entry.is_some());
}

use super::*;

#[test]
fn snapshot_changes_for_body_status_and_projection_without_head_or_count_changes() {
    let fixture = Fixture::new();
    let first = fixture.page(None, None);
    let older = fixture.page(Some("m1970"), None);
    assert_eq!(first.snapshot_token, older.snapshot_token);
    assert_eq!(first.snapshot_token.len(), 64);
    assert!(
        first
            .snapshot_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    let connection = fixture.connection();
    connection
        .execute(
            "UPDATE messages SET content='edited user body' WHERE id='m1998'",
            [],
        )
        .unwrap();
    let body = fixture.page(Some("m1970"), None);
    assert_ne!(body.snapshot_token, first.snapshot_token);
    assert_eq!(body.messages, older.messages);
    assert_eq!(body.head_message_id, first.head_message_id);
    assert_eq!(body.total_messages, first.total_messages);
    connection
        .execute(
            "UPDATE messages SET status='cancelled' WHERE id='m1999'",
            [],
        )
        .unwrap();
    let status = fixture.page(Some("m1970"), None);
    assert_ne!(status.snapshot_token, body.snapshot_token);

    // Synthetic fixture only: remove immutability to model changed valid sidecars.
    connection
        .execute_batch("DROP TRIGGER message_display_projections_no_update")
        .unwrap();
    let before_projection = fixture.page(None, None);
    connection.execute("UPDATE message_display_projections SET display_content=?1, display_content_sha256=?2 WHERE message_id='m1999'", params!["new display", digest("new display")]).unwrap();
    let projection = fixture.page(None, None);
    assert_ne!(projection.snapshot_token, before_projection.snapshot_token);
    assert_eq!(projection.messages.last().unwrap().content, "new display");
    assert_eq!(projection.head_message_id, first.head_message_id);
    assert_eq!(projection.total_messages, first.total_messages);
}

#[test]
fn reopening_never_reuses_an_old_instance_snapshot() {
    let Fixture { root, shell } = Fixture::new();
    let first = shell.list_branch_messages_page(input(None, None)).unwrap();
    drop(shell);
    let reopened = ShellApi::open_data_root(root.path()).unwrap();
    let latest = reopened
        .list_branch_messages_page(input(None, None))
        .unwrap();
    assert_eq!(first.messages, latest.messages);
    assert_ne!(first.snapshot_token, latest.snapshot_token);
}

#[test]
fn concurrent_canonical_and_display_changes_are_read_from_one_transaction() {
    let fixture = Fixture::new();
    let mut writer = fixture.connection();
    writer
        .execute_batch("DROP TRIGGER message_display_projections_no_update")
        .unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let barrier = &barrier;
        scope.spawn(move || {
            barrier.wait();
            for index in 0..32 {
                let canonical = format!("canonical-{index}");
                let display = format!("display-{index}");
                let transaction = writer.transaction().unwrap();
                transaction.execute("UPDATE messages SET content=?1 WHERE id='m1999'", [&canonical]).unwrap();
                transaction.execute("UPDATE message_display_projections SET canonical_content_sha256=?1, display_content=?2, display_content_sha256=?3 WHERE message_id='m1999'", params![digest(&canonical), display, digest(&display)]).unwrap();
                transaction.commit().unwrap();
                std::thread::yield_now();
            }
        });
        barrier.wait();
        for _ in 0..32 {
            let page = fixture.page(None, None);
            let message = page.messages.last().unwrap();
            let canonical = if message.content == DISPLAY {
                CANONICAL.to_owned()
            } else {
                message.content.replacen("display-", "canonical-", 1)
            };
            let projection = message.display_projection.as_ref().unwrap();
            assert_eq!(projection.canonical_content_sha256, digest(&canonical));
            assert_eq!(projection.display_content_sha256, digest(&message.content));
        }
    });
}

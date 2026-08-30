use super::*;

#[test]
fn effect_outbox_retries_after_lease_and_acknowledges_once() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, request_rule_id, _approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    persist_proposal_request(
        &storage,
        InteractionStateKey {
            state_id: "effect-outbox-state".to_owned(),
            conversation_id,
            branch_id,
        },
        "effect-outbox-proposal",
        &rule_set_id,
        &request_rule_id,
        &rule_set_revision_id,
    );

    let poll_at = Utc::now() + Duration::seconds(1);
    let pending = storage
        .list_pending_interaction_effects(poll_at, 8)
        .expect("list pending effect");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].delivery_attempts, 0);
    let first_lease = poll_at + Duration::seconds(30);
    let first_claim = storage
        .claim_pending_interaction_effects(poll_at, first_lease, 8)
        .expect("claim pending effect");
    assert_eq!(first_claim.len(), 1);
    assert_eq!(first_claim[0].delivery_attempts, 1);
    assert!(
        storage
            .list_pending_interaction_effects(poll_at + Duration::seconds(1), 8)
            .expect("poll during lease")
            .is_empty()
    );
    let retry_at = poll_at + Duration::seconds(2);
    storage
        .retry_interaction_effect_after(
            &first_claim[0].event_id,
            first_claim[0].sequence,
            first_claim[0].delivery_attempts,
            retry_at,
        )
        .expect("release effect for retry");
    let second_lease = retry_at + Duration::seconds(30);
    let second_claim = storage
        .claim_pending_interaction_effects(retry_at, second_lease, 8)
        .expect("claim explicit retry");
    assert_eq!(second_claim.len(), 1);
    assert_eq!(second_claim[0].effect_id, first_claim[0].effect_id);
    assert_eq!(second_claim[0].delivery_attempts, 2);
    let third_lease = second_lease + Duration::seconds(30);
    let third_claim = storage
        .claim_pending_interaction_effects(second_lease, third_lease, 8)
        .expect("reclaim after crashed lease expiry");
    assert_eq!(third_claim.len(), 1);
    assert_eq!(third_claim[0].effect_id, first_claim[0].effect_id);
    assert_eq!(third_claim[0].delivery_attempts, 3);
    storage
        .mark_interaction_effect_delivered(
            &third_claim[0].event_id,
            third_claim[0].sequence,
            third_claim[0].delivery_attempts,
            third_lease,
        )
        .expect("ack effect");
    assert!(
        storage
            .mark_interaction_effect_delivered(
                &third_claim[0].event_id,
                third_claim[0].sequence,
                third_claim[0].delivery_attempts,
                third_lease,
            )
            .is_err(),
        "effect acknowledgement must be exactly once"
    );
    assert!(
        storage
            .list_pending_interaction_effects(third_lease + Duration::seconds(1), 8)
            .expect("poll after acknowledgement")
            .is_empty(),
        "acknowledged effect must not be delivered again"
    );
}

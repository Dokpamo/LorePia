use super::*;

#[test]
fn acknowledged_effect_history_reconstructs_room_without_replaying_audio() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let key = InteractionStateKey {
        state_id: "reopen-effect-state".to_owned(),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
    };
    let created_at = Utc::now();
    persist_effect_bundle(
        &storage,
        &key,
        vec![
            InteractionEffect::AssetShown {
                asset_id: AssetId::from("background-asset"),
                region: UiRegion::Background,
            },
            InteractionEffect::AssetShown {
                asset_id: AssetId::from("status-asset"),
                region: UiRegion::StatusPanel,
            },
            InteractionEffect::AudioRequested {
                asset_id: AssetId::from("one-shot-audio"),
            },
            InteractionEffect::ChoicesPresented {
                choices: vec![
                    choice_spec("left", "Go left"),
                    choice_spec("right", "Go right"),
                ],
            },
            InteractionEffect::VisibleSystemEvent {
                text: "A durable event".to_owned(),
            },
            InteractionEffect::DiceRolled {
                expression: DiceExpression {
                    count: 1,
                    sides: 6,
                    modifier: 0,
                },
                rolls: vec![4],
                total: 4,
                target: None,
            },
        ],
        created_at,
    );

    let claim_at = created_at + Duration::seconds(1);
    let lease_until = claim_at + Duration::seconds(30);
    let claimed = storage
        .claim_pending_interaction_effects(claim_at, lease_until, 16)
        .expect("claim full effect bundle");
    assert_eq!(claimed.len(), 6, "dice must also enter the durable outbox");
    for effect in &claimed {
        assert_eq!(effect.conversation_id, conversation_id);
        assert_eq!(effect.branch_id, branch_id);
        assert_eq!(effect.interaction_state_id, key.state_id);
        assert_eq!(effect.resulting_state_revision, 1);
        storage
            .mark_interaction_effect_delivered(
                &effect.event_id,
                effect.sequence,
                effect.delivery_attempts,
                lease_until,
            )
            .expect("acknowledge durable effect");
    }

    let history = storage
        .list_interaction_effect_history(&conversation_id, &branch_id, None, 16)
        .expect("load acknowledged effect history");
    assert_eq!(history.len(), 6);
    assert!(
        history
            .iter()
            .all(|entry| entry.stored.delivered_at.is_some()),
        "delivery acknowledgement must not erase immutable history"
    );
    let first_page = storage
        .list_interaction_effect_history(&conversation_id, &branch_id, None, 2)
        .expect("load first effect history page");
    let cursor = InteractionEffectHistoryCursor {
        resulting_state_revision: first_page[1].stored.resulting_state_revision,
        sequence: first_page[1].stored.sequence,
    };
    let remaining = storage
        .list_interaction_effect_history(&conversation_id, &branch_id, Some(cursor), 16)
        .expect("load remaining effect history");
    assert_eq!(remaining.len(), 4);

    let reopen = storage
        .list_reopen_interaction_effects(&conversation_id, &branch_id, None, 16)
        .expect("load reopen reconstruction effects");
    assert_eq!(reopen.len(), 5);
    assert!(reopen.iter().all(|entry| entry.replay_on_reopen));
    assert!(
        !reopen.iter().any(|entry| matches!(
            &entry.stored.effect,
            InteractionEffect::AudioRequested { .. }
        )),
        "one-shot audio must not replay after reopen"
    );
    assert!(reopen.iter().any(|entry| matches!(
        &entry.stored.effect,
        InteractionEffect::AssetShown {
            region: UiRegion::Background,
            ..
        }
    )));
    assert!(reopen.iter().any(|entry| matches!(
        &entry.stored.effect,
        InteractionEffect::AssetShown {
            region: UiRegion::StatusPanel,
            ..
        }
    )));
    assert!(
        reopen
            .iter()
            .any(|entry| matches!(&entry.stored.effect, InteractionEffect::DiceRolled { .. }))
    );
    assert!(reopen.iter().any(|entry| {
        matches!(
            &entry.stored.effect,
            InteractionEffect::ChoicesPresented { .. }
        ) && entry.stored.choice_status == Some(InteractionChoiceEffectStatus::Pending)
    }));
    let regions = storage
        .get_interaction_region_effects(&conversation_id, &branch_id)
        .expect("load latest region state");
    assert_eq!(regions.len(), 2);
    let pending_choices = storage
        .list_pending_interaction_choice_effects(&conversation_id, &branch_id, 8)
        .expect("load actionable choices");
    assert_eq!(pending_choices.len(), 1);
    let projection = storage
        .get_interaction_reopen_projection(&conversation_id, &branch_id, 2, 8)
        .expect("load one-snapshot reopen projection");
    assert_eq!(
        projection
            .iter()
            .map(|entry| entry.stored.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 4, 5, 6],
        "region state and pending choices must survive a short recent tail"
    );
    assert_eq!(
        projection
            .iter()
            .map(|entry| entry.stored.effect_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        projection.len(),
        "overlapping projection sources must be deduplicated"
    );
    let recent = storage
        .list_recent_reopen_interaction_effects(&conversation_id, &branch_id, 2)
        .expect("load newest reopen window");
    assert_eq!(
        recent
            .iter()
            .map(|entry| entry.stored.sequence)
            .collect::<Vec<_>>(),
        vec![5, 6]
    );
    let older = storage
        .list_older_reopen_interaction_effects(
            &conversation_id,
            &branch_id,
            InteractionEffectHistoryCursor {
                resulting_state_revision: recent[0].stored.resulting_state_revision,
                sequence: recent[0].stored.sequence,
            },
            2,
        )
        .expect("load older reopen window");
    assert_eq!(
        older
            .iter()
            .map(|entry| entry.stored.sequence)
            .collect::<Vec<_>>(),
        vec![2, 4]
    );
    assert!(
        storage
            .list_reopen_interaction_effects(
                &ConversationId("another-room".to_owned()),
                &branch_id,
                None,
                16,
            )
            .expect("query another room")
            .is_empty(),
        "room-scoped history must not leak effects across conversations"
    );
}
#[test]
fn choice_selection_is_fixed_to_durable_effect_and_consumed_exactly_once() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let key = InteractionStateKey {
        state_id: "choice-state".to_owned(),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
    };
    let created_at = Utc::now();
    persist_effect_bundle(
        &storage,
        &key,
        vec![InteractionEffect::ChoicesPresented {
            choices: vec![
                choice_spec("left", "Go left"),
                choice_spec("right", "Go right"),
            ],
        }],
        created_at,
    );
    let choice_effect = storage
        .list_interaction_effect_history(&conversation_id, &branch_id, None, 8)
        .expect("load choice effect")
        .pop()
        .expect("one choice effect");

    let invalid = storage.consume_interaction_choice(&InteractionChoiceSelectionCommit {
        effect_id: choice_effect.stored.effect_id.clone(),
        choice_id: "caller-injected-action".to_owned(),
        expected_state_revision: 1,
        selected_at_epoch_seconds: created_at.timestamp() + 1,
        current_policy: empty_policy(),
        derived: InteractionDerivedEventCommit {
            event_id: "invalid-choice-event".to_owned(),
            idempotency_key: "invalid-choice-event-key".to_owned(),
            policy: empty_policy(),
            evaluation_seal: None,
            deterministic_seed: None,
            next_state: empty_state(2),
            knowledge: Vec::new(),
            action_results: Vec::new(),
            effects: Vec::new(),
            derived_events: Vec::new(),
            proposals: Vec::new(),
            created_at: created_at + Duration::seconds(1),
        },
    });
    assert!(
        invalid.is_err(),
        "an action absent from the durable choice payload must be rejected"
    );

    let selection = InteractionChoiceSelectionCommit {
        effect_id: choice_effect.stored.effect_id.clone(),
        choice_id: "left".to_owned(),
        expected_state_revision: 1,
        selected_at_epoch_seconds: created_at.timestamp() + 1,
        current_policy: empty_policy(),
        derived: InteractionDerivedEventCommit {
            event_id: "selected-choice-event".to_owned(),
            idempotency_key: "selected-choice-event-key".to_owned(),
            policy: empty_policy(),
            evaluation_seal: None,
            deterministic_seed: None,
            next_state: empty_state(2),
            knowledge: Vec::new(),
            action_results: Vec::new(),
            effects: Vec::new(),
            derived_events: Vec::new(),
            proposals: Vec::new(),
            created_at: created_at + Duration::seconds(1),
        },
    };
    let receipt = storage
        .consume_interaction_choice(&selection)
        .expect("consume exact durable choice");
    assert_eq!(receipt.resulting_state_revision, 2);
    assert_eq!(
        receipt.choice_effect.stored.choice_status,
        Some(InteractionChoiceEffectStatus::Consumed)
    );
    assert_eq!(
        receipt.choice_effect.stored.selected_choice_id.as_deref(),
        Some("left")
    );
    let stored_event_json = storage
        .connection()
        .expect("open test connection")
        .query_row(
            "SELECT event_argument_json FROM interaction_events WHERE id = ?1",
            ["selected-choice-event"],
            |row| row.get::<_, String>(0),
        )
        .expect("load fixed choice event");
    let stored_event: InteractionEvent =
        serde_json::from_str(&stored_event_json).expect("decode fixed choice event");
    assert_eq!(
        stored_event,
        InteractionEvent::UserAction {
            action_id: "left".to_owned()
        }
    );

    let mut replay = selection;
    replay.expected_state_revision = 2;
    replay.derived.event_id = "second-choice-event".to_owned();
    replay.derived.idempotency_key = "second-choice-event-key".to_owned();
    replay.derived.next_state = empty_state(3);
    assert!(
        storage.consume_interaction_choice(&replay).is_err(),
        "a consumed choice effect must reject every second selection"
    );
    assert_eq!(
        storage
            .get_interaction_state(&conversation_id, &branch_id)
            .expect("load state after rejected replay")
            .revision,
        2
    );
}

#[test]
fn choice_selection_rejects_policy_update_after_presentation() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, _request_rule_id, _approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    let origin_policy = policy_for_rule_set(&storage, &rule_set_id, &rule_set_revision_id);
    let key = InteractionStateKey {
        state_id: "stale-choice-policy-state".to_owned(),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
    };
    let created_at = Utc::now();
    storage
        .get_or_init_interaction_state(&key, &empty_state(0), &[], created_at)
        .expect("initialize stale-policy choice state");
    storage
        .commit_interaction_event(&InteractionEventCommit {
            event_id: "stale-policy-choice-presented".to_owned(),
            idempotency_key: "stale-policy-choice-presented-key".to_owned(),
            key,
            expected_state_revision: 0,
            event: InteractionEvent::ConversationOpened,
            generation_attempt_id: None,
            owner_message_id: None,
            policy: origin_policy,
            evaluation_seal: None,
            deterministic_seed: None,
            next_state: empty_state(1),
            knowledge: Vec::new(),
            action_results: Vec::new(),
            effects: vec![InteractionEffect::ChoicesPresented {
                choices: vec![choice_spec("continue", "Continue")],
            }],
            derived_events: Vec::new(),
            proposals: Vec::new(),
            created_at,
        })
        .expect("persist policy-bound choice");
    let choice = storage
        .list_pending_interaction_choice_effects(&conversation_id, &branch_id, 8)
        .expect("load policy-bound choice")
        .pop()
        .expect("one policy-bound choice");
    let stale = storage.consume_interaction_choice(&InteractionChoiceSelectionCommit {
        effect_id: choice.stored.effect_id.clone(),
        choice_id: "continue".to_owned(),
        expected_state_revision: 1,
        selected_at_epoch_seconds: created_at.timestamp() + 1,
        current_policy: empty_policy(),
        derived: InteractionDerivedEventCommit {
            event_id: "stale-policy-choice-click".to_owned(),
            idempotency_key: "stale-policy-choice-click-key".to_owned(),
            policy: empty_policy(),
            evaluation_seal: None,
            deterministic_seed: None,
            next_state: empty_state(2),
            knowledge: Vec::new(),
            action_results: Vec::new(),
            effects: Vec::new(),
            derived_events: Vec::new(),
            proposals: Vec::new(),
            created_at: created_at + Duration::seconds(1),
        },
    });
    assert!(
        stale.is_err(),
        "choice click must fail when its evaluation policy changed"
    );
    assert_eq!(
        storage
            .get_interaction_effect(&choice.stored.effect_id)
            .expect("reload stale-policy choice")
            .stored
            .choice_status,
        Some(InteractionChoiceEffectStatus::Pending)
    );
    assert_eq!(
        storage
            .get_interaction_state(&conversation_id, &branch_id)
            .expect("reload stale-policy state")
            .revision,
        1
    );
}

#[test]
fn expired_choice_cannot_be_consumed_and_does_not_advance_state() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let key = InteractionStateKey {
        state_id: "expired-choice-state".to_owned(),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
    };
    let created_at = Utc::now();
    persist_effect_bundle(
        &storage,
        &key,
        vec![InteractionEffect::ChoicesPresented {
            choices: vec![choice_spec("continue", "Continue")],
        }],
        created_at,
    );
    let choice_effect = storage
        .list_reopen_interaction_effects(&conversation_id, &branch_id, None, 8)
        .expect("load pending choice")
        .pop()
        .expect("one pending choice");
    let expired = storage
        .expire_interaction_choice(&InteractionChoiceExpirationCommit {
            effect_id: choice_effect.stored.effect_id.clone(),
            expired_at_epoch_seconds: created_at.timestamp() + 10,
        })
        .expect("expire pending choice");
    assert_eq!(
        expired.stored.choice_status,
        Some(InteractionChoiceEffectStatus::Expired)
    );
    assert_eq!(
        storage
            .get_interaction_state(&conversation_id, &branch_id)
            .expect("load state after expiry")
            .revision,
        1,
        "choice expiration is UI lifecycle state, not a domain transition"
    );
    assert!(
        storage
            .consume_interaction_choice(&InteractionChoiceSelectionCommit {
                effect_id: choice_effect.stored.effect_id,
                choice_id: "continue".to_owned(),
                expected_state_revision: 1,
                selected_at_epoch_seconds: created_at.timestamp() + 11,
                current_policy: empty_policy(),
                derived: InteractionDerivedEventCommit {
                    event_id: "expired-choice-event".to_owned(),
                    idempotency_key: "expired-choice-event-key".to_owned(),
                    policy: empty_policy(),
                    evaluation_seal: None,
                    deterministic_seed: None,
                    next_state: empty_state(2),
                    knowledge: Vec::new(),
                    action_results: Vec::new(),
                    effects: Vec::new(),
                    derived_events: Vec::new(),
                    proposals: Vec::new(),
                    created_at: created_at + Duration::seconds(11),
                },
            })
            .is_err(),
        "expired choice must never dispatch a UserAction"
    );
}

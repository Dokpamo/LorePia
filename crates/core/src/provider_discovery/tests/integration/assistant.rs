
#[test]
#[allow(clippy::too_many_lines)]
fn assistant_question_reopens_accepts_fresh_evidence_and_resumes_high_level_turn() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let assistant_route_id = configure_synthetic_assistant(&core, &provider);

    let started = core
        .begin_provider_discovery_site(assistant_discovery_input(
            &provider,
            "assistant-discovery-connection",
            assistant_route_id,
        ))
        .expect("begin assistant discovery");
    let awaiting_consent = if started.session.state == DiscoveryState::AwaitingTemplateSelection {
        continue_with(
            &core,
            &started,
            ProviderDiscoveryAction::ContinueWithoutTemplate,
            None,
        )
    } else {
        started
    };
    assert_eq!(
        awaiting_consent.session.state,
        DiscoveryState::AwaitingAssistantConsent,
        "assistant discovery failed: {:?}; requests: {:?}",
        awaiting_consent.session.failure,
        provider
            .captured_requests()
            .iter()
            .map(|request| String::from_utf8_lossy(request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned())
            .collect::<Vec<_>>()
    );
    let approval = core
        .get_provider_discovery_approval_proposal(&awaiting_consent.session.id)
        .expect("load assistant approval")
        .expect("assistant approval proposal");
    assert!(matches!(
        approval.grant,
        DiscoveryApprovalGrant::AssistantConsent { .. }
    ));
    let ready = continue_with(
        &core,
        &awaiting_consent,
        ProviderDiscoveryAction::ApproveAssistant {
            approval_id: approval.id,
            approval_grant_sha256: approval.grant_sha256,
        },
        None,
    );
    assert_eq!(
        core.get_provider_discovery_assistant_resume_boundary(&ready.session.id)
            .expect("load ready assistant boundary")
            .expect("ready assistant boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::RunAssistant
    );

    let session_id = ready.session.id.clone();
    let persisted_options = ready.session.input.connection_options.clone();
    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen ready assistant");
    let reopened_ready = reopened
        .get_provider_discovery(&session_id)
        .expect("reload ready assistant");
    assert_eq!(
        reopened_ready.session.state,
        DiscoveryState::BuildingAssistantManifestDraft
    );
    assert_eq!(
        reopened_ready.session.input.connection_options,
        persisted_options
    );
    assert_eq!(
        reopened
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("load reopened ready boundary")
            .expect("reopened ready boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::RunAssistant
    );

    let estimate = AssistantCallEstimate {
        input_tokens: 128,
        maximum_output_tokens: 256,
        maximum_cost_micro_units: 1_000,
    };
    let first_action = reopened
        .run_provider_discovery_assistant_turn(&session_id, estimate, Some(SECRET_CANARY))
        .expect("run first high-level assistant turn");
    let AssistantHostAction::RequestMoreEvidence { questions, .. } = first_action else {
        panic!("assistant must request fresh evidence");
    };
    assert_eq!(questions.len(), 1);
    let awaiting_evidence = reopened
        .get_provider_discovery(&session_id)
        .expect("load assistant question boundary");
    assert_eq!(
        awaiting_evidence.session.state,
        DiscoveryState::AwaitingMoreEvidence
    );
    assert_eq!(
        reopened
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("load question boundary")
            .expect("question boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::SupplyMoreEvidence
    );

    drop(reopened);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen assistant question");
    let question_boundary = reopened
        .get_provider_discovery_assistant_resume_boundary(&session_id)
        .expect("reload persisted question boundary")
        .expect("persisted question boundary");
    assert_eq!(
        question_boundary.action,
        ProviderDiscoveryAssistantResumeAction::SupplyMoreEvidence
    );
    assert_eq!(question_boundary.questions.len(), 1);
    let awaiting_evidence = reopened
        .get_provider_discovery(&session_id)
        .expect("reload awaiting evidence");
    let resumed = reopened
        .supply_provider_discovery_evidence(
            &session_id,
            awaiting_evidence.session.revision,
            ProviderDiscoveryAdditionalEvidence::document_url(
                HttpUrl::parse(&format!("{}/fresh.txt", provider.origin))
                    .expect("fresh evidence URL"),
            ),
        )
        .expect("supply fresh assistant evidence");
    assert_eq!(
        resumed.session.state,
        DiscoveryState::BuildingAssistantManifestDraft
    );
    assert_eq!(
        reopened
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("load resumed assistant boundary")
            .expect("resumed assistant boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::RunAssistant
    );

    drop(reopened);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen resumed assistant");
    assert_eq!(
        reopened
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("load second ready boundary")
            .expect("second ready boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::RunAssistant
    );
    provider.queue_assistant_response(assistant_turn_sse(&AssistantTurn::CallTool {
        call: AssistantToolCall::ShowUnresolvedQuestions,
    }));
    provider.queue_assistant_response(assistant_turn_sse(&AssistantTurn::NeedMoreEvidence {
        questions: vec![UnresolvedQuestion {
            id: "still-unresolved-after-show".to_owned(),
            field: None,
            question: "Provide one final current official endpoint excerpt.".to_owned(),
            required_evidence: "A different bounded excerpt from the approved origin.".to_owned(),
        }],
    }));
    let second_action = reopened
        .run_provider_discovery_assistant_turn(&session_id, estimate, Some(SECRET_CANARY))
        .expect("run second high-level assistant turn");
    let AssistantHostAction::RequestMoreEvidence { questions, .. } = second_action else {
        panic!("assistant must return the next unresolved evidence boundary");
    };
    assert_eq!(
        questions
            .iter()
            .map(|question| question.id.as_str())
            .collect::<Vec<_>>(),
        vec!["still-unresolved-after-show"]
    );
    let follow_up_boundary = reopened
        .get_provider_discovery_assistant_resume_boundary(&session_id)
        .expect("load follow-up unresolved boundary")
        .expect("follow-up unresolved boundary");
    assert_eq!(
        follow_up_boundary.action,
        ProviderDiscoveryAssistantResumeAction::SupplyMoreEvidence
    );
    assert_eq!(
        follow_up_boundary
            .questions
            .iter()
            .map(|question| question.id.as_str())
            .collect::<Vec<_>>(),
        vec!["still-unresolved-after-show"],
        "the consumed question set must be replaced by the exact follow-up set"
    );
    let assistant_prompt_bodies = provider
        .captured_requests()
        .into_iter()
        .filter_map(|request| {
            let request_line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            if !request_line.contains(&format!(" {} ", provider.generation_path())) {
                return None;
            }
            let header_end = find_bytes(&request, b"\r\n\r\n")?;
            Some(String::from_utf8_lossy(&request[header_end + 4..]).into_owned())
        })
        .collect::<Vec<_>>();
    assert!(
        assistant_prompt_bodies.len() >= 3,
        "need-more-evidence, ShowUnresolvedQuestions, and follow-up turns must all run"
    );
    for raw_body in &assistant_prompt_bodies {
        let body: serde_json::Value =
            serde_json::from_str(raw_body).expect("parse captured assistant request body");
        let format = &body["response_format"];
        assert_eq!(format["type"], "json_schema");
        assert_eq!(
            format["json_schema"]["name"],
            "lorepia_setup_assistant_turn_v1"
        );
        assert_eq!(format["json_schema"]["strict"], true);
        let schema = &format["json_schema"]["schema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["turn"]["$ref"],
            "#/$defs/assistant_turn"
        );
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        let untrusted_payload = body["messages"][1]["content"]
            .as_str()
            .expect("assistant data-channel payload");
        assert!(untrusted_payload.contains("\"unresolved_questions\""));
        assert!(untrusted_payload.contains("lorepia_setup_assistant_turn_v1"));
    }
    let post_evidence_prompt = &assistant_prompt_bodies[assistant_prompt_bodies.len() - 2];
    assert!(
        [
            "unresolved_questions",
            "need-current-endpoint",
            "Provide one more current official endpoint excerpt.",
            "A bounded official document excerpt from the approved origin.",
        ]
        .iter()
        .all(|expected| post_evidence_prompt.contains(expected)),
        "the post-evidence prompt must preserve the full typed durable unresolved question"
    );
    assert!(
        assistant_prompt_bodies.last().is_some_and(|body| {
            body.contains("question_ids") && body.contains("need-current-endpoint")
        }),
        "the follow-up prompt must contain the typed ShowUnresolvedQuestions result"
    );
    assert_public_surfaces_are_secret_free(&reopened);
    assert_prompt_bodies_are_secret_free(&provider);
    assert_data_root_is_secret_free(root.path());
}

#[test]
#[allow(clippy::too_many_lines)]
fn structural_assistant_draft_is_claim_bound_reviewed_committed_and_reopened() {
    let root = tempdir().expect("temporary Core root");
    let assistant_provider = SyntheticProvider::start();
    let target_provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let assistant_route_id = configure_synthetic_assistant(&core, &assistant_provider);

    let mut input = discovery_input(&target_provider, "assistant-committed-connection");
    input.preferred_assistant = Some(assistant_route_id);
    let awaiting_consent = core
        .begin_provider_discovery_site(input)
        .expect("begin structural assistant discovery");
    assert_eq!(
        awaiting_consent.session.state,
        DiscoveryState::AwaitingAssistantConsent,
        "structural evidence with an explicitly selected assistant must reach consent: {:?}",
        awaiting_consent.session.failure
    );
    assistant_provider.queue_assistant_response(assistant_turn_sse(&claim_bound_assistant_draft(
        &core,
        &target_provider,
        &awaiting_consent.session.id,
    )));

    let approval = core
        .get_provider_discovery_approval_proposal(&awaiting_consent.session.id)
        .expect("load assistant approval")
        .expect("assistant approval proposal");
    let ready = continue_with(
        &core,
        &awaiting_consent,
        ProviderDiscoveryAction::ApproveAssistant {
            approval_id: approval.id,
            approval_grant_sha256: approval.grant_sha256,
        },
        None,
    );
    let session_id = ready.session.id.clone();
    let action = core
        .run_provider_discovery_assistant_turn(
            &session_id,
            AssistantCallEstimate {
                input_tokens: 512,
                maximum_output_tokens: 2_048,
                maximum_cost_micro_units: 10_000,
            },
            Some(SECRET_CANARY),
        )
        .expect("run claim-bound assistant draft");
    let AssistantHostAction::ReviewDraft(review) = action else {
        panic!("assistant must return a claim-bound draft");
    };
    assert!(review.draft.manifest.endpoints.models.is_some());
    assert_eq!(
        core.get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("load draft-ready boundary")
            .expect("draft-ready boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::ReviewDraft
    );

    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen draft-ready assistant");
    assert_eq!(
        reopened
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("reload draft-ready boundary")
            .expect("persisted draft-ready boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::ReviewDraft
    );
    let accepted = reopened
        .accept_provider_discovery_assistant_draft(&session_id)
        .expect("accept claim-bound assistant draft");
    assert_eq!(
        accepted.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval
    );
    let reviewed = approve_to_review(&reopened, &accepted, &target_provider, SECRET_CANARY, false);
    let committing = approve_review(&reopened, &reviewed, &target_provider);
    let committed = commit_credential_bound_discovery(&reopened, &session_id);
    assert_eq!(
        committed.id,
        ProviderConnectionId::from("assistant-committed-connection")
    );
    assert_eq!(committed.api_origin.as_str(), target_provider.origin);
    assert!(
        committed.config.api_base_path.is_none(),
        "the assistant-discovered template must own its API base path"
    );
    let committed_template = reopened
        .list_provider_templates()
        .expect("list assistant-discovered templates")
        .into_iter()
        .find(|template| template.id == committed.template_id)
        .expect("assistant-discovered template");
    assert_eq!(
        committed_template
            .default_manifest
            .endpoints
            .generate
            .path
            .as_str(),
        target_provider.generation_path()
    );
    assert_eq!(committing.session.state, DiscoveryState::Committing);

    drop(reopened);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen committed provider");
    let persisted = reopened
        .list_provider_connections()
        .expect("list reopened connections")
        .into_iter()
        .find(|connection| connection.id == committed.id)
        .expect("reopened assistant-discovered connection");
    assert_eq!(persisted, committed);
    assert_public_surfaces_are_secret_free(&reopened);
    assert_prompt_bodies_are_secret_free(&assistant_provider);
    assert_data_root_is_secret_free(root.path());
}

#[test]
fn legacy_bare_assistant_turn_is_rejected_with_explicit_retry_and_no_fallback_request() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let assistant_route_id = configure_synthetic_assistant(&core, &provider);
    let started = core
        .begin_provider_discovery_site(assistant_discovery_input(
            &provider,
            "assistant-bare-envelope-connection",
            assistant_route_id,
        ))
        .expect("begin assistant bare-envelope discovery");
    let awaiting_consent = if started.session.state == DiscoveryState::AwaitingTemplateSelection {
        continue_with(
            &core,
            &started,
            ProviderDiscoveryAction::ContinueWithoutTemplate,
            None,
        )
    } else {
        started
    };
    assert_eq!(
        awaiting_consent.session.state,
        DiscoveryState::AwaitingAssistantConsent
    );
    let approval = core
        .get_provider_discovery_approval_proposal(&awaiting_consent.session.id)
        .expect("load assistant approval")
        .expect("assistant approval proposal");
    let ready = continue_with(
        &core,
        &awaiting_consent,
        ProviderDiscoveryAction::ApproveAssistant {
            approval_id: approval.id,
            approval_grant_sha256: approval.grant_sha256,
        },
        None,
    );
    provider.queue_assistant_response(bare_assistant_turn_sse(&AssistantTurn::NeedMoreEvidence {
        questions: vec![UnresolvedQuestion {
            id: "legacy-bare-question".to_owned(),
            field: None,
            question: "Which endpoint is current?".to_owned(),
            required_evidence: "A current official endpoint table.".to_owned(),
        }],
    }));
    let generation_path = provider.generation_path();
    let generation_count = || {
        provider
            .captured_requests()
            .iter()
            .filter(|request| {
                String::from_utf8_lossy(request)
                    .lines()
                    .next()
                    .is_some_and(|line| line.contains(&format!(" {generation_path} ")))
            })
            .count()
    };
    let before = generation_count();
    let error = core
        .run_provider_discovery_assistant_turn(
            &ready.session.id,
            AssistantCallEstimate {
                input_tokens: 128,
                maximum_output_tokens: 256,
                maximum_cost_micro_units: 1_000,
            },
            Some(SECRET_CANARY),
        )
        .expect_err("legacy bare turn must not bypass the response envelope");
    assert_eq!(error.code, lorepia_core::CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert_eq!(generation_count(), before + 1);
    assert_eq!(
        core.get_provider_discovery_assistant_resume_boundary(&ready.session.id)
            .expect("load invalid-envelope boundary")
            .expect("invalid-envelope retry boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::ApproveRetry
    );
    assert_public_surfaces_are_secret_free(&core);
    assert_prompt_bodies_are_secret_free(&provider);
    drop(core);
    assert_data_root_is_secret_free(root.path());
}

#[test]
fn assistant_response_reflecting_split_credential_fails_closed_without_persistence() {
    let root = tempdir().expect("temporary Core root");
    let assistant_provider = SyntheticProvider::start();
    let target_provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let assistant_route_id = configure_synthetic_assistant(&core, &assistant_provider);

    let mut input = discovery_input(&target_provider, "assistant-reflection-connection");
    input.preferred_assistant = Some(assistant_route_id);
    let awaiting_consent = core
        .begin_provider_discovery_site(input)
        .expect("begin assistant reflection discovery");
    assert_eq!(
        awaiting_consent.session.state,
        DiscoveryState::AwaitingAssistantConsent
    );
    let approval = core
        .get_provider_discovery_approval_proposal(&awaiting_consent.session.id)
        .expect("load assistant approval")
        .expect("assistant approval proposal");
    let ready = continue_with(
        &core,
        &awaiting_consent,
        ProviderDiscoveryAction::ApproveAssistant {
            approval_id: approval.id,
            approval_grant_sha256: approval.grant_sha256,
        },
        None,
    );
    assistant_provider.queue_assistant_response(assistant_credential_reflection_sse(SECRET_CANARY));
    let error = core
        .run_provider_discovery_assistant_turn(
            &ready.session.id,
            AssistantCallEstimate {
                input_tokens: 128,
                maximum_output_tokens: 256,
                maximum_cost_micro_units: 1_000,
            },
            Some(SECRET_CANARY),
        )
        .expect_err("credential-reflecting assistant response must fail closed");
    assert_eq!(error.code, lorepia_core::CoreErrorCode::ProviderUnavailable);
    assert!(!format!("{error:?}").contains(SECRET_CANARY));
    let recovered = core
        .get_provider_discovery(&ready.session.id)
        .expect("load assistant after rejected reflection");
    assert_eq!(
        core.get_provider_discovery_assistant_resume_boundary(&ready.session.id)
            .expect("load rejected reflection boundary")
            .expect("rejected reflection boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::ApproveRetry
    );
    assert!(recovered.review.is_none());
    assert_public_surfaces_are_secret_free(&core);
    assert_prompt_bodies_are_secret_free(&assistant_provider);
    drop(core);
    assert_data_root_is_secret_free(root.path());
}

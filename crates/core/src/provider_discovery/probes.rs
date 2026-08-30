use super::{
    AdapterRegistry, Arc, BTreeSet, CapabilityObservation, CapabilityProbeEngine,
    CapabilityProbeKind, ConnectionStatus, CoreError, CoreErrorCode, CoreResult, DateTime,
    DiscoveryCandidate, DiscoveryCandidateId, DiscoveryCandidateSummary, DiscoveryEvidenceKind,
    DiscoveryEvidenceRecord, DiscoveryProbeBudget, DiscoverySessionSnapshot, DiscoveryState,
    DiscoveryWorkingDraft, Duration, Handle, HttpUrl, ModelListRequest, ModelRoute, ProbeBudget,
    ProbeConsent, ProbeRunOutcome, ProviderCapabilityProbeAdapter, STANDARD_DISCOVERY_PROBE_PLAN,
    StoredDiscoveryCandidate, Utc, canonical_sha256, deterministic_id, initial_generation_preset,
    provider_api_capability_observations, reconcile_input_routes, template_accepts_empty_preset,
    watch,
};

pub(super) enum ProbeExecution {
    Completed {
        evidence: Vec<DiscoveryEvidenceRecord>,
    },
    Unknown,
}

pub(super) fn standard_probe_budget(route_count: usize) -> CoreResult<DiscoveryProbeBudget> {
    DiscoveryProbeBudget::standard_for_plan(route_count, STANDARD_DISCOVERY_PROBE_PLAN.len())
        .map_err(|error| CoreError::invalid(format!("invalid capability probe budget: {error}")))
}

pub(super) fn approved_probe_routes(
    draft: &DiscoveryWorkingDraft,
    approved_budget: DiscoveryProbeBudget,
) -> CoreResult<Vec<ModelRoute>> {
    if approved_budget != standard_probe_budget(draft.probe_route_ids.len())? {
        return Err(CoreError::invalid(
            "capability probe budget does not match the exact approved route set",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut routes = Vec::with_capacity(draft.probe_route_ids.len());
    for route_id in &draft.probe_route_ids {
        if !seen.insert(route_id.clone()) {
            return Err(CoreError::invalid(
                "capability probe route set contains a duplicate route",
            ));
        }
        let mut matches = draft.routes.iter().filter(|route| route.id == *route_id);
        let route = matches.next().ok_or_else(|| {
            CoreError::invalid("capability probe route is outside the approved working graph")
        })?;
        if matches.next().is_some() {
            return Err(CoreError::invalid(
                "capability probe working graph contains a duplicate route",
            ));
        }
        routes.push(route.clone());
    }
    Ok(routes)
}

#[allow(clippy::too_many_lines)]
pub(super) fn probe_draft(
    runtime: &Handle,
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    credential: Option<&str>,
    approved_budget: DiscoveryProbeBudget,
    cancelled: watch::Receiver<bool>,
) -> CoreResult<ProbeExecution> {
    let approved_routes = approved_probe_routes(draft, approved_budget)?;
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("capability probe has no template"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("capability probe has no connection"))?;
    let budget = ProbeBudget::new(
        approved_budget.max_total_tokens_per_request,
        approved_budget.max_output_tokens_per_request,
        approved_budget.max_cost_micro_usd_per_request,
        Duration::from_millis(approved_budget.max_duration_millis_per_request),
        approved_budget.max_calls_per_request,
    )?;
    let registry = AdapterRegistry::new();
    let evidence_source_url = HttpUrl::parse(connection.api_origin.as_str())
        .map_err(|error| CoreError::invalid(format!("invalid probe evidence origin: {error}")))?;
    let engine = CapabilityProbeEngine::new();
    let mut request_count = 0_u32;
    let mut evidence = Vec::new();
    for route in approved_routes {
        if *cancelled.borrow() {
            return if request_count == 0 {
                Err(CoreError::new(
                    CoreErrorCode::Cancelled,
                    "provider discovery was cancelled before capability probing started",
                    false,
                ))
            } else {
                Ok(ProbeExecution::Unknown)
            };
        }
        let provider =
            registry.build_provider_for_route_with_plan(template, connection, &route, None)?;
        for probe in STANDARD_DISCOVERY_PROBE_PLAN {
            if *cancelled.borrow() {
                return if request_count == 0 {
                    Err(CoreError::new(
                        CoreErrorCode::Cancelled,
                        "provider discovery was cancelled before capability probing started",
                        false,
                    ))
                } else {
                    Ok(ProbeExecution::Unknown)
                };
            }
            request_count = request_count
                .checked_add(1)
                .ok_or_else(|| CoreError::invalid("capability probe request count overflowed"))?;
            if request_count > approved_budget.max_requests {
                return Err(CoreError::invalid(
                    "capability probe execution exceeds the approved request count",
                ));
            }
            let Ok(adapter) = ProviderCapabilityProbeAdapter::new(
                route.api_family,
                route.id.clone(),
                route.model_id.clone(),
                Arc::clone(&provider),
                credential,
                probe,
                approved_budget.max_cost_micro_usd_per_request,
            ) else {
                draft.probe_failure_count = draft.probe_failure_count.saturating_add(1);
                continue;
            };
            let consent_id = deterministic_id(
                &snapshot.session.id,
                snapshot.session.revision,
                &format!("probe:{}:{}", route.id.as_str(), probe_slug(probe)),
            );
            let consent = ProbeConsent::new(consent_id, route.id.clone(), probe, budget)?;
            match runtime.block_on(engine.run(
                Arc::new(adapter),
                &route.id,
                probe,
                consent,
                cancelled.clone(),
            )) {
                ProbeRunOutcome::Observed(observation) => {
                    evidence.push(capability_probe_evidence(
                        snapshot,
                        &evidence_source_url,
                        &observation,
                    )?);
                    draft.observations.push(observation);
                }
                ProbeRunOutcome::Failed(_) | ProbeRunOutcome::CancelledBeforeStart => {
                    draft.probe_failure_count = draft.probe_failure_count.saturating_add(1);
                }
                ProbeRunOutcome::UnknownOutcome(_) => return Ok(ProbeExecution::Unknown),
            }
        }
    }
    if request_count != approved_budget.max_requests {
        return Err(CoreError::invalid(
            "capability probe execution did not match the approved request count",
        ));
    }
    Ok(ProbeExecution::Completed { evidence })
}

fn capability_probe_evidence(
    snapshot: &DiscoverySessionSnapshot,
    source_url: &HttpUrl,
    observation: &CapabilityObservation,
) -> CoreResult<DiscoveryEvidenceRecord> {
    let id = observation.evidence_ref.clone().ok_or_else(|| {
        CoreError::internal("capability probe observation has no evidence reference")
    })?;
    let extracted_json = serde_json::json!({
        "kind": "capability_probe",
        "model_route_id": observation.model_route_id,
        "capability": observation.key,
        "value": observation.value,
        "status": observation.status,
        "source": observation.source,
        "confidence": observation.confidence,
        "observed_at": observation.observed_at,
        "expires_at": observation.expires_at,
    });
    let content_sha256 = canonical_sha256(&extracted_json, "capability probe evidence")?;
    Ok(DiscoveryEvidenceRecord {
        id,
        session_id: snapshot.session.id.clone(),
        kind: DiscoveryEvidenceKind::JsonDocument,
        source_url: source_url.clone(),
        content_sha256,
        extracted_json,
        fetched_at: observation.observed_at,
    })
}

const fn probe_slug(probe: CapabilityProbeKind) -> &'static str {
    match probe {
        CapabilityProbeKind::Streaming => "streaming",
        CapabilityProbeKind::Reasoning => "reasoning",
        CapabilityProbeKind::StructuredOutput => "structured-output",
        CapabilityProbeKind::ToolCalling => "tool-calling",
        CapabilityProbeKind::PromptCaching => "prompt-caching",
    }
}

pub(super) fn list_models_for_draft(
    runtime: &Handle,
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    credential: Option<&str>,
    cancelled: watch::Receiver<bool>,
) -> CoreResult<()> {
    if snapshot.session.state != DiscoveryState::ListingModels {
        return Err(CoreError::invalid(
            "model listing state changed unexpectedly",
        ));
    }
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("model listing has no template"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("model listing has no connection"))?;
    let listing = AdapterRegistry::new().build_model_listing(template, connection)?;
    let listed =
        runtime.block_on(listing.list_models(ModelListRequest::new(credential, cancelled)))?;
    ensure_listing_does_not_reflect_credential(&listed, credential)?;
    apply_listed_models_to_draft(draft, &listed.models, Utc::now())
}

pub(super) fn apply_listed_models_to_draft(
    draft: &mut DiscoveryWorkingDraft,
    listed_models: &[lorepia_providers::ListedModel],
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("model listing has no template"))?
        .clone();
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("model listing has no connection"))?
        .clone();
    let (routes, _, _) = reconcile_input_routes(
        &connection.id,
        template.api_family,
        &[],
        listed_models,
        observed_at,
    )?;
    // `reconcile_input_routes` retains only the same closed, bounded,
    // credential-scanned provider metadata accepted by durable model sync.
    // Persisting that normalized projection lets the first reviewed discovery
    // graph enforce model-specific parameter controls immediately; no raw
    // provider response bytes enter the review or storage graph.
    let observations = provider_api_capability_observations(&routes, listed_models, observed_at)?;
    let presets = if template_accepts_empty_preset(&template)? {
        routes
            .iter()
            .map(|route| initial_generation_preset(&route.id, &template, observed_at))
            .collect()
    } else {
        Vec::new()
    };
    let mut connected = connection.clone();
    connected.status = ConnectionStatus::Connected;
    connected.updated_at = observed_at;
    draft.connection = Some(connected);
    draft.routes = routes;
    draft.observations = observations;
    draft.presets = presets;
    Ok(())
}

fn ensure_listing_does_not_reflect_credential(
    listed: &lorepia_providers::ModelListResult,
    credential: Option<&str>,
) -> CoreResult<()> {
    let Some(secret) = credential.filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    if listed.models.iter().any(|model| {
        model.model_id.contains(secret)
            || model
                .display_name
                .as_deref()
                .is_some_and(|value| value.contains(secret))
            || model
                .supported_generation_methods
                .iter()
                .any(|value| value.contains(secret))
            || serde_json::to_string(&model.capabilities).is_ok_and(|value| value.contains(secret))
    }) {
        return Err(CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            "provider model response reflected credential material",
            false,
        ));
    }
    Ok(())
}

pub(super) fn model_candidates(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<Vec<StoredDiscoveryCandidate>> {
    draft
        .routes
        .iter()
        .map(|route| {
            Ok(StoredDiscoveryCandidate {
                candidate: DiscoveryCandidate {
                    id: DiscoveryCandidateId::parse(deterministic_id(
                        &snapshot.session.id,
                        0,
                        &format!("model-route:{}", route.id.as_str()),
                    ))
                    .map_err(|error| {
                        CoreError::internal(format!("candidate id failed: {error}"))
                    })?,
                    session_id: snapshot.session.id.clone(),
                    summary: DiscoveryCandidateSummary::ModelRoute {
                        model_id: route.model_id.clone(),
                    },
                    evidence_ids: Vec::new(),
                    created_at: snapshot.created_at,
                },
                proposed_revision: snapshot.session.revision,
            })
        })
        .collect()
}

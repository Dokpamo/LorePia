use super::*;

use super::{
    known_provider::active_discovery_templates,
    probes::{ProbeExecution, list_models_for_draft, model_candidates, probe_draft},
};

impl ProviderDiscoveryOrchestrator<'_> {
    #[allow(clippy::too_many_lines)]
    pub(super) fn execute_nonpersistent_effect(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation: DiscoveryOperationKind,
        draft: &mut DiscoveryWorkingDraft,
        credential: Option<&str>,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<EffectCompletion> {
        match operation {
            DiscoveryOperationKind::ResolveKnownProvider => {
                if draft.deterministic.is_none() {
                    let site_intent = matches!(&draft.source, DiscoverySourceIntent::Site);
                    let source = match &draft.source {
                        DiscoverySourceIntent::KnownProvider { template_id } => {
                            DeterministicDiscoverySource::known_provider_id(template_id.clone())
                        }
                        DiscoverySourceIntent::Site => {
                            match DeterministicDiscoverySource::known_provider_site_with_policy(
                                snapshot.session.input.site_url.as_str(),
                                discovery_url_policy(&snapshot.session.input.connection_options)?,
                            ) {
                                Ok(source) => source,
                                Err(error) => return Err(deterministic_error(error)),
                            }
                        }
                        DiscoverySourceIntent::Curl => {
                            return Err(CoreError::invalid(
                                "sanitized cURL evidence must be supplied again after interruption",
                            ));
                        }
                    };
                    let active_templates = active_discovery_templates(self.storage)?;
                    let output = self.runtime.block_on(
                        DeterministicDiscoveryExecutor::new()
                            .execute_with_templates(source, &active_templates),
                    );
                    draft.deterministic = match output {
                        Ok(output) => Some(output),
                        Err(error)
                            if site_intent
                                && error.kind()
                                    == DeterministicDiscoveryErrorKind::KnownProviderNotFound =>
                        {
                            return Ok(EffectCompletion::simple(
                                ProviderDiscoveryAction::KnownProviderCandidatesResolved {
                                    candidate_count: 0,
                                },
                            ));
                        }
                        Err(error) => return Err(deterministic_error(error)),
                    };
                }
                let (evidence, candidates) =
                    deterministic_artifacts(snapshot, draft.deterministic.as_ref().expect("set"))?;
                let deterministic = draft.deterministic.clone().expect("set");
                record_deterministic_assistant_claims(snapshot, &deterministic, draft)?;
                draft.evidence_ids = evidence.iter().map(|record| record.id.clone()).collect();
                let candidate_count = u32::try_from(candidates.len())
                    .map_err(|_| CoreError::invalid("too many discovery candidates"))?;
                Ok(EffectCompletion {
                    action: ProviderDiscoveryAction::KnownProviderCandidatesResolved {
                        candidate_count,
                    },
                    evidence,
                    candidates,
                    review: DiscoveryJsonUpdate::Preserve,
                    outcome: DurableOperationOutcome::Succeeded,
                })
            }
            DiscoveryOperationKind::FetchDocuments => {
                let mut source = DeterministicDiscoverySource::site_with_policy(
                    snapshot.session.input.site_url.as_str(),
                    discovery_url_policy(&snapshot.session.input.connection_options)?,
                    DiscoveryFetchBudget::default(),
                )
                .map_err(deterministic_error)?;
                if let Some(docs_url) = &snapshot.session.input.docs_url {
                    source
                        .allow_document_url(docs_url.as_str())
                        .map_err(deterministic_error)?;
                }
                let output = self
                    .runtime
                    .block_on(DeterministicDiscoveryExecutor::new().execute(source))
                    .map_err(deterministic_error)?;
                draft.deterministic = Some(output);
                let (evidence, _) =
                    deterministic_artifacts(snapshot, draft.deterministic.as_ref().expect("set"))?;
                let deterministic = draft.deterministic.clone().expect("set");
                record_deterministic_assistant_claims(snapshot, &deterministic, draft)?;
                draft.evidence_ids = evidence.iter().map(|record| record.id.clone()).collect();
                let evidence_count = u32::try_from(evidence.len())
                    .map_err(|_| CoreError::invalid("too much discovery evidence"))?;
                Ok(EffectCompletion {
                    action: ProviderDiscoveryAction::DocumentsFetched { evidence_count },
                    evidence,
                    candidates: Vec::new(),
                    review: DiscoveryJsonUpdate::Preserve,
                    outcome: DurableOperationOutcome::Succeeded,
                })
            }
            DiscoveryOperationKind::ExtractEvidence => {
                let deterministic = draft.deterministic.as_ref();
                let has_deterministic_draft = deterministic.is_some_and(|output| {
                    !output.manifest_candidates.is_empty()
                        && (snapshot.session.input.preferred_assistant.is_none()
                            || output.manifest_candidates.iter().any(|candidate| {
                                candidate.confidence
                                    == DiscoveryCandidateConfidence::ExactCompiledProvider
                            }))
                });
                if has_deterministic_draft {
                    draft.assistant = None;
                    draft.assistant_approval_binding = None;
                    draft.assistant_more_evidence_questions.clear();
                    return Ok(EffectCompletion::simple(
                        ProviderDiscoveryAction::EvidenceExtracted {
                            resolution: DiscoveryEvidenceResolution::DeterministicDraftAvailable,
                        },
                    ));
                }
                if draft.assistant.is_some()
                    && restored_assistant(draft)?.state() == AssistantState::Ready
                {
                    let approval = draft.assistant_approval_binding.as_ref().ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "resumable setup assistant lost its approval binding",
                            false,
                        )
                    })?;
                    return Ok(EffectCompletion::simple(
                        ProviderDiscoveryAction::AssistantResumedWithEvidence {
                            approval_id: approval.approval_id.clone(),
                            approval_grant_sha256: approval.grant_sha256.clone(),
                        },
                    ));
                }
                let resolution = if snapshot.session.input.preferred_assistant.is_some()
                    && !draft.evidence_ids.is_empty()
                {
                    initialize_assistant(self.storage, snapshot, draft)?;
                    DiscoveryEvidenceResolution::AssistantRecommended
                } else {
                    DiscoveryEvidenceResolution::MoreEvidenceRequired
                };
                Ok(EffectCompletion::simple(
                    ProviderDiscoveryAction::EvidenceExtracted { resolution },
                ))
            }
            DiscoveryOperationKind::BuildDeterministicManifestDraft => {
                build_deterministic_graph(self.storage, snapshot, draft, Utc::now())?;
                let template = draft
                    .template
                    .as_ref()
                    .ok_or_else(|| CoreError::internal("manifest build produced no template"))?;
                let manifest_sha256 = validate_manifest(&template.default_manifest)?
                    .sha256()
                    .to_owned();
                Ok(EffectCompletion::simple(
                    ProviderDiscoveryAction::ManifestDraftBuilt { manifest_sha256 },
                ))
            }
            DiscoveryOperationKind::ValidateManifest => {
                let template = draft
                    .template
                    .as_ref()
                    .ok_or_else(|| CoreError::internal("manifest validation has no template"))?;
                validate_connection_fields(&template.connection_fields)?;
                let validated = validate_manifest(&template.default_manifest)?;
                let connection = draft
                    .connection
                    .as_ref()
                    .ok_or_else(|| CoreError::internal("manifest validation has no connection"))?;
                let credential_required = template.default_manifest.auth != AuthBinding::None;
                if credential_required && connection.credential_ref.is_none() {
                    return Err(CoreError::invalid(
                        "authenticated provider discovery requires an opaque credential reference",
                    ));
                }
                Ok(EffectCompletion::simple(
                    ProviderDiscoveryAction::ManifestValidated {
                        manifest_sha256: validated.sha256().to_owned(),
                        credential_origin_approval_required: credential_required,
                    },
                ))
            }
            DiscoveryOperationKind::ListModels => {
                revalidate_discovery_catalog_authority(self.storage, draft, Utc::now())?;
                list_models_for_draft(self.runtime, snapshot, draft, credential, cancelled)?;
                let model_count = u32::try_from(draft.routes.len())
                    .map_err(|_| CoreError::invalid("too many listed models"))?;
                draft.probe_route_ids = draft.routes.iter().map(|route| route.id.clone()).collect();
                let probe_candidate_count = model_count;
                let review = if probe_candidate_count == 0 {
                    DiscoveryJsonUpdate::Replace(build_review(draft)?)
                } else {
                    DiscoveryJsonUpdate::Preserve
                };
                Ok(EffectCompletion {
                    action: ProviderDiscoveryAction::ModelsListed {
                        model_count,
                        probe_candidate_count,
                    },
                    evidence: Vec::new(),
                    candidates: model_candidates(snapshot, draft)?,
                    review,
                    outcome: DurableOperationOutcome::Succeeded,
                })
            }
            DiscoveryOperationKind::ProbeCapabilities => {
                revalidate_discovery_catalog_authority(self.storage, draft, Utc::now())?;
                let budget = approved_probe_budget(self.storage, snapshot, draft)?;
                let outcome =
                    probe_draft(self.runtime, snapshot, draft, credential, budget, cancelled)?;
                match outcome {
                    ProbeExecution::Completed { evidence } => Ok(EffectCompletion {
                        action: ProviderDiscoveryAction::ProbesCompleted,
                        evidence,
                        candidates: Vec::new(),
                        review: DiscoveryJsonUpdate::Replace(build_review(draft)?),
                        outcome: DurableOperationOutcome::Succeeded,
                    }),
                    ProbeExecution::Unknown => Ok(EffectCompletion {
                        action: ProviderDiscoveryAction::Interrupt {
                            operation,
                            outcome: DiscoveryInterruptionOutcome::ExternalOutcomeUnknown,
                        },
                        evidence: Vec::new(),
                        candidates: Vec::new(),
                        review: DiscoveryJsonUpdate::Preserve,
                        outcome: DurableOperationOutcome::OutcomeUnknown,
                    }),
                }
            }
            DiscoveryOperationKind::BuildAssistantManifestDraft
            | DiscoveryOperationKind::AtomicCommit
            | DiscoveryOperationKind::Compensation => Err(CoreError::invalid(
                "persistent or host-driven effect cannot run automatically",
            )),
        }
    }
}

pub(super) fn deterministic_artifacts(
    snapshot: &DiscoverySessionSnapshot,
    output: &DeterministicDiscoveryOutput,
) -> CoreResult<(Vec<DiscoveryEvidenceRecord>, Vec<StoredDiscoveryCandidate>)> {
    let evidence = output
        .evidence
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let id = EvidenceId::from(deterministic_id(
                &snapshot.session.id,
                0,
                &format!("evidence:{index}:{}", item.content_sha256),
            ));
            let source_url = HttpUrl::parse(item.source_origin.as_str())
                .map_err(|error| CoreError::invalid(format!("invalid evidence origin: {error}")))?;
            Ok(DiscoveryEvidenceRecord {
                id,
                session_id: snapshot.session.id.clone(),
                kind: storage_evidence_kind(&item.kind),
                source_url,
                content_sha256: item.content_sha256.clone(),
                extracted_json: item.extracted_json.clone(),
                fetched_at: snapshot.created_at,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let candidates = output
        .manifest_candidates
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let evidence_ids = item
                .evidence_indices
                .iter()
                .filter_map(|index| evidence.get(*index).map(|record| record.id.clone()))
                .collect();
            let candidate = DiscoveryCandidate {
                id: DiscoveryCandidateId::parse(deterministic_id(
                    &snapshot.session.id,
                    0,
                    &format!(
                        "template-candidate:{index}:{}:{}",
                        item.template.id.as_str(),
                        item.template.manifest_version
                    ),
                ))
                .map_err(|error| CoreError::internal(format!("candidate id failed: {error}")))?,
                session_id: snapshot.session.id.clone(),
                summary: DiscoveryCandidateSummary::ProviderTemplate {
                    template_id: item.template.id.clone(),
                    template_version: item.template.manifest_version,
                },
                evidence_ids,
                created_at: snapshot.created_at,
            };
            Ok(StoredDiscoveryCandidate {
                candidate,
                proposed_revision: snapshot.session.revision,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    Ok((evidence, candidates))
}

fn storage_evidence_kind(kind: &str) -> DiscoveryEvidenceKind {
    match kind {
        "html_document" => DiscoveryEvidenceKind::HtmlDocument,
        "json_document" | "sanitized_curl_request" | "built_in_template" => {
            DiscoveryEvidenceKind::JsonDocument
        }
        "yaml_document" => DiscoveryEvidenceKind::YamlDocument,
        "xml_document" => DiscoveryEvidenceKind::XmlDocument,
        "json_schema" => DiscoveryEvidenceKind::JsonSchema,
        "open_api" => DiscoveryEvidenceKind::OpenApi,
        _ => DiscoveryEvidenceKind::PlainTextDocument,
    }
}

pub(super) fn select_candidate(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    candidate_id: &DiscoveryCandidateId,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    let entry = storage
        .read_discovery_candidates(&snapshot.session.id, MAX_DISCOVERY_ROWS)?
        .into_iter()
        .find(|entry| entry.candidate.id == *candidate_id)
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider discovery candidate was not found",
                false,
            )
        })?;
    let DiscoveryCandidateSummary::ProviderTemplate {
        template_id,
        template_version,
    } = entry.candidate.summary
    else {
        return Err(CoreError::invalid(
            "selected discovery candidate is not a provider template",
        ));
    };
    let template = draft
        .deterministic
        .as_ref()
        .and_then(|output| {
            output
                .manifest_candidates
                .iter()
                .find(|item| {
                    item.template.id == template_id
                        && item.template.manifest_version == template_version
                })
                .map(|item| item.template.clone())
        })
        .or_else(|| {
            storage
                .get_provider_template(&template_id, template_version)
                .ok()
        })
        .ok_or_else(|| CoreError::internal("selected provider template cannot be hydrated"))?;
    draft.selected_candidate_id = Some(candidate_id.clone());
    let catalog_authority = current_discovery_catalog_authority(storage, &template, observed_at)?;
    install_graph_seed(snapshot, draft, template, observed_at)?;
    draft.catalog_authority = catalog_authority;
    Ok(())
}

fn build_deterministic_graph(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    if draft.template.is_some() && draft.connection.is_some() {
        return Ok(());
    }
    let output = draft
        .deterministic
        .as_ref()
        .ok_or_else(|| CoreError::invalid("no deterministic provider result is available"))?;
    let template = output
        .selected_template
        .clone()
        .or_else(|| {
            (output.manifest_candidates.len() == 1)
                .then(|| output.manifest_candidates[0].template.clone())
        })
        .ok_or_else(|| CoreError::invalid("provider template selection is still ambiguous"))?;
    let catalog_authority = current_discovery_catalog_authority(storage, &template, observed_at)?;
    install_graph_seed(snapshot, draft, template, observed_at)?;
    draft.catalog_authority = catalog_authority;
    Ok(())
}

pub(super) fn install_graph_seed(
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    template: ProviderTemplate,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    install_graph_seed_internal(snapshot, draft, template, observed_at, false)
}

pub(super) fn install_graph_seed_with_embedded_base(
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    template: ProviderTemplate,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    install_graph_seed_internal(snapshot, draft, template, observed_at, true)
}

fn install_graph_seed_internal(
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    template: ProviderTemplate,
    observed_at: DateTime<Utc>,
    api_base_path_is_embedded: bool,
) -> CoreResult<()> {
    require_active_discovery_network_authority(
        &snapshot.session.input.connection_options,
        observed_at,
    )?;
    validate_connection_fields(&template.connection_fields)?;
    let hint = draft.deterministic.as_ref().and_then(|output| {
        output
            .connection_hints
            .iter()
            .find(|hint| hint.api_family == template.api_family)
    });
    let api_origin = hint
        .map(|hint| hint.api_origin.clone())
        .or_else(|| template.default_manifest.default_api_origin.clone())
        .or_else(|| origin_from_http_url(&snapshot.session.input.site_url).ok())
        .ok_or_else(|| CoreError::invalid("provider API origin could not be determined"))?;
    let options = &snapshot.session.input.connection_options;
    let template_owns_api_base_path =
        api_base_path_is_embedded || template.source == TemplateSource::UserDiscovered;
    if template_owns_api_base_path
        && let Some(explicit_base_path) = &options.api_base_path
        && !manifest_endpoints_include_base(&template.default_manifest, explicit_base_path)
    {
        return Err(CoreError::invalid(
            "explicit API base path conflicts with the self-contained discovered template",
        ));
    }
    let api_base_path = if template_owns_api_base_path {
        None
    } else {
        options
            .api_base_path
            .clone()
            .or_else(|| hint.and_then(|hint| hint.api_base_path.clone()))
    };
    let values = resolved_discovery_connection_values(
        &template,
        &options.values,
        &api_origin,
        api_base_path.as_ref(),
    )?;
    validate_discovery_connection_values(
        &template,
        &values,
        snapshot.session.input.credential_ref.as_ref(),
    )?;
    let local_network_approval = normalized_local_network_approval(options, &api_origin)?;
    let created_at = if options.network_mode == ProviderNetworkMode::ApprovedLocalNetwork {
        options.local_network_approved_at.ok_or_else(|| {
            CoreError::invalid(
                "legacy LAN discovery has no approval issue time; restart provider discovery",
            )
        })?
    } else {
        observed_at
    };
    draft.connection = Some(ProviderConnection {
        id: snapshot.session.input.connection_id.clone(),
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        display_name: snapshot.session.input.display_name.clone(),
        api_origin,
        config: ConnectionConfig {
            api_base_path,
            network_mode: options.network_mode,
            local_network_approval,
            values,
        },
        credential_ref: snapshot.session.input.credential_ref.clone(),
        credential_scope: None,
        timeout_seconds: options.timeout_seconds,
        status: ConnectionStatus::Untested,
        created_at,
        updated_at: observed_at,
    });
    draft.template = Some(template);
    draft.catalog_authority = None;
    Ok(())
}

fn manifest_endpoints_include_base(
    manifest: &ProviderManifest,
    api_base_path: &lorepia_domain::EndpointPath,
) -> bool {
    let includes_base = |path: &lorepia_domain::EndpointPath| {
        let base = api_base_path.as_str().trim_end_matches('/');
        base.is_empty()
            || path.as_str() == base
            || path
                .as_str()
                .strip_prefix(base)
                .is_some_and(|remainder| remainder.starts_with('/'))
    };
    includes_base(&manifest.endpoints.generate.path)
        && manifest
            .endpoints
            .models
            .as_ref()
            .is_none_or(|endpoint| includes_base(&endpoint.path))
}

fn resolved_discovery_connection_values(
    template: &ProviderTemplate,
    supplied: &[lorepia_domain::ConnectionConfigEntry],
    api_origin: &CanonicalOrigin,
    api_base_path: Option<&lorepia_domain::EndpointPath>,
) -> CoreResult<Vec<lorepia_domain::ConnectionConfigEntry>> {
    let mut values = supplied.to_vec();
    let base_url_is_declared = template.connection_fields.iter().any(|field| {
        field.key.eq_ignore_ascii_case("api_base_url")
            && field.value_type == ConnectionFieldType::Text
    });
    let base_url_is_supplied = values
        .iter()
        .any(|entry| entry.key.eq_ignore_ascii_case("api_base_url"));
    if base_url_is_declared && !base_url_is_supplied {
        let mut value = api_origin.as_str().trim_end_matches('/').to_owned();
        if let Some(path) = api_base_path
            && path.as_str() != "/"
        {
            value.push('/');
            value.push_str(path.as_str().trim_start_matches('/'));
        }
        HttpUrl::parse(&value).map_err(|error| {
            CoreError::invalid(format!("derived API base URL is invalid: {error}"))
        })?;
        values.push(lorepia_domain::ConnectionConfigEntry {
            key: "api_base_url".to_owned(),
            value: ConnectionConfigValue::Text(value),
        });
    }
    Ok(values)
}

fn validate_discovery_connection_values(
    template: &ProviderTemplate,
    values: &[lorepia_domain::ConnectionConfigEntry],
    credential_ref: Option<&CredentialRef>,
) -> CoreResult<()> {
    let mut supplied = std::collections::BTreeMap::new();
    for entry in values {
        let normalized = entry.key.to_ascii_lowercase();
        if supplied.insert(normalized, &entry.value).is_some() {
            return Err(CoreError::invalid(
                "provider connection values contain duplicate keys",
            ));
        }
    }

    let mut declared = std::collections::BTreeSet::new();
    for field in &template.connection_fields {
        let normalized = field.key.to_ascii_lowercase();
        declared.insert(normalized.clone());
        let supplied_value = supplied.get(&normalized).copied();
        match field.value_type {
            ConnectionFieldType::Credential => {
                if supplied_value.is_some() {
                    return Err(CoreError::invalid(
                        "credential fields must use the native credential reference",
                    ));
                }
                if field.required && credential_ref.is_none() {
                    return Err(CoreError::invalid(
                        "provider connection is missing its required credential reference",
                    ));
                }
            }
            ConnectionFieldType::Text => {
                if supplied_value
                    .is_some_and(|value| !matches!(value, ConnectionConfigValue::Text(_)))
                {
                    return Err(CoreError::invalid(
                        "provider connection text field has the wrong value type",
                    ));
                }
                if field.required && supplied_value.is_none() {
                    return Err(CoreError::invalid(
                        "provider connection is missing a required text value",
                    ));
                }
            }
            ConnectionFieldType::Integer => {
                if supplied_value
                    .is_some_and(|value| !matches!(value, ConnectionConfigValue::Integer(_)))
                {
                    return Err(CoreError::invalid(
                        "provider connection integer field has the wrong value type",
                    ));
                }
                if field.required && supplied_value.is_none() {
                    return Err(CoreError::invalid(
                        "provider connection is missing a required integer value",
                    ));
                }
            }
            ConnectionFieldType::Boolean => {
                if supplied_value
                    .is_some_and(|value| !matches!(value, ConnectionConfigValue::Boolean(_)))
                {
                    return Err(CoreError::invalid(
                        "provider connection boolean field has the wrong value type",
                    ));
                }
                if field.required && supplied_value.is_none() {
                    return Err(CoreError::invalid(
                        "provider connection is missing a required boolean value",
                    ));
                }
            }
        }
    }
    if supplied.keys().any(|key| !declared.contains(key)) {
        return Err(CoreError::invalid(
            "provider connection contains a value not declared by its template",
        ));
    }
    Ok(())
}

fn normalized_local_network_approval(
    options: &ProviderDiscoveryConnectionOptions,
    api_origin: &CanonicalOrigin,
) -> CoreResult<Option<ProviderLocalNetworkApproval>> {
    match (
        options.network_mode,
        options.local_network_approval.as_ref(),
    ) {
        (ProviderNetworkMode::Public | ProviderNetworkMode::LocalLoopback, None) => Ok(None),
        (ProviderNetworkMode::ApprovedLocalNetwork, Some(approval)) => {
            if &approval.origin != api_origin {
                return Err(CoreError::invalid(
                    "local-network approval origin must exactly match the discovered API origin",
                ));
            }
            let approved =
                ApprovedLocalNetworkOrigin::new(approval.origin.as_str(), &approval.addresses)
                    .map_err(|error| {
                        CoreError::invalid(format!(
                            "provider local-network approval is invalid: {error}"
                        ))
                    })?;
            Ok(Some(ProviderLocalNetworkApproval {
                origin: api_origin.clone(),
                addresses: approved.addresses().to_vec(),
            }))
        }
        (ProviderNetworkMode::ApprovedLocalNetwork, None) => Err(CoreError::invalid(
            "approved local-network mode requires an exact origin and address approval",
        )),
        (ProviderNetworkMode::Public | ProviderNetworkMode::LocalLoopback, Some(_)) => {
            Err(CoreError::invalid(
                "local-network approval is valid only in approved local-network mode",
            ))
        }
    }
}

pub(super) fn deterministic_error(
    error: crate::provider_discovery_deterministic::DeterministicDiscoveryError,
) -> CoreError {
    let (code, message) = match error.kind() {
        DeterministicDiscoveryErrorKind::InvalidSource
        | DeterministicDiscoveryErrorKind::InvalidDocumentUrl
        | DeterministicDiscoveryErrorKind::InvalidFetchBudget
        | DeterministicDiscoveryErrorKind::CurlParseRejected => (
            CoreErrorCode::InvalidInput,
            "provider discovery source was rejected",
        ),
        DeterministicDiscoveryErrorKind::KnownProviderNotFound => {
            (CoreErrorCode::NotFound, "known provider was not found")
        }
        DeterministicDiscoveryErrorKind::ProviderContractUnavailable
        | DeterministicDiscoveryErrorKind::EvidenceSerializationFailed
        | DeterministicDiscoveryErrorKind::UnsafeEvidence => (
            CoreErrorCode::UnsupportedContent,
            "provider discovery evidence could not be used",
        ),
    };
    CoreError::new(code, message, false)
}

fn current_discovery_catalog_authority(
    storage: &Storage,
    template: &ProviderTemplate,
    now: DateTime<Utc>,
) -> CoreResult<Option<DiscoveryCatalogAuthorityBinding>> {
    if template.source != TemplateSource::SignedCatalog {
        return Ok(None);
    }
    operational_provider_catalog_projection_for_storage(storage, now)?
        .discovery_authority_binding(template, now)
}

pub(super) fn revalidate_discovery_catalog_authority(
    storage: &Storage,
    draft: &DiscoveryWorkingDraft,
    now: DateTime<Utc>,
) -> CoreResult<()> {
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("provider discovery has no template authority"))?;
    if template.source != TemplateSource::SignedCatalog {
        return if draft.catalog_authority.is_none() {
            Ok(())
        } else {
            Err(CoreError::invalid(
                "non-catalog provider discovery carries signed catalog authority",
            ))
        };
    }
    let current = current_discovery_catalog_authority(storage, template, now)?;
    if current != draft.catalog_authority {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "signed catalog authority changed or expired; restart provider discovery",
            true,
        ));
    }
    Ok(())
}

pub(super) fn revalidate_prepared_discovery_catalog_authority(
    storage: &Storage,
    draft: &DiscoveryWorkingDraft,
    phase: DiscoveryCommitPhase,
) -> CoreResult<()> {
    if phase == DiscoveryCommitPhase::Prepared {
        revalidate_discovery_catalog_authority(storage, draft, Utc::now())?;
    }
    Ok(())
}

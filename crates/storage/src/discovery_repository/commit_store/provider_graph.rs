//! Provider-graph validation, publication, and durable authority audit checks.

use super::super::{
    BTreeSet, CapabilityObservation, Confidence, Connection, ConnectionConfigValue, CoreError,
    CoreErrorCode, CoreResult, DateTime, DiscoveredProviderGraph, DiscoveryCommitAttemptRecord,
    DiscoveryOperationId, DiscoverySessionId, GenerationPreset, ModelMetadataSource, ModelRoute,
    ObservationSource, OptionalExtension, ProviderConnection, ProviderNetworkMode,
    ProviderTemplate, SanitizedDiscoveryInput, StoredDiscoveredProviderGraphRows, SupportStatus,
    TemplateSource, Transaction, Utc, Value, append_audit, canonical_json_result,
    canonical_typed_json_result, contract_error, corrupted, database_error,
    encode_commit_plan_json, load_discovered_provider_graph_rows, looks_like_secret, params,
    parse_timestamp, sha256_hex, validate_credential_approval,
    validate_discovery_local_network_approval_binding, validate_opaque_credential_reference,
    validate_persistable_discovery_url, validate_provider_api_route_metadata,
    validate_redacted_value, validate_review_approval, validate_sanitized_input, validate_sha256,
    write_discovered_provider_graph_rows,
};

pub(in crate::discovery_repository) fn require_started_session_operation(
    transaction: &Transaction<'_>,
    session_id: &DiscoverySessionId,
    expected_kind: &str,
) -> CoreResult<DiscoveryOperationId> {
    let row = transaction
        .query_row(
            "SELECT session.active_operation_id, operation.operation_kind,
                    operation.side_effect_class, operation.status
             FROM provider_discovery_sessions AS session
             LEFT JOIN provider_discovery_operations AS operation
               ON operation.id = session.active_operation_id
              AND operation.session_id = session.id
             WHERE session.id = ?1",
            [session_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider discovery session was not found",
                false,
            )
        })?;
    let (Some(operation_id), Some(kind), Some(side_effect_class), Some(status)) = row else {
        return Err(corrupted(
            "discovery session has no durable active operation",
        ));
    };
    if kind != expected_kind || side_effect_class != "persistent" || status != "started" {
        return Err(CoreError::invalid(
            "persistent discovery work requires its exact durable operation to be started",
        ));
    }
    DiscoveryOperationId::parse(operation_id).map_err(contract_error)
}

fn ensure_provider_graph_ids_vacant(
    transaction: &Transaction<'_>,
    graph: &DiscoveredProviderGraph,
) -> CoreResult<()> {
    let connection_exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_connections WHERE id = ?1)",
            [graph.connection.id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if connection_exists {
        return Err(CoreError::invalid(
            "discovery commit connection identifier already belongs to another graph",
        ));
    }
    for route in &graph.routes {
        if transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
                [route.id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?
        {
            return Err(CoreError::invalid(
                "discovery commit model route identifier already exists",
            ));
        }
    }
    for observation in &graph.observations {
        if transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM model_capability_observations WHERE id = ?1
                 )",
                [observation.id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?
        {
            return Err(CoreError::invalid(
                "discovery commit capability observation identifier already exists",
            ));
        }
    }
    for preset in &graph.presets {
        if transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM generation_presets WHERE id = ?1)",
                [preset.id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?
        {
            return Err(CoreError::invalid(
                "discovery commit generation preset identifier already exists",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(in crate::discovery_repository) fn apply_provider_graph_in_transaction(
    transaction: &Transaction<'_>,
    graph: &DiscoveredProviderGraph,
    expected_session_revision: u64,
    applied_at: DateTime<Utc>,
    authority_observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    validate_provider_graph(graph)?;
    let plan_json = encode_commit_plan_json(&graph.plan)?;
    if sha256_hex(plan_json.as_bytes()) != graph.plan_sha256 {
        return Err(CoreError::invalid(
            "provider graph plan hash does not match its canonical plan",
        ));
    }
    let session = transaction
        .query_row(
            "SELECT state, revision, commit_attempt_id, commit_plan_sha256,
                    sanitized_input_json, created_at
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [graph.plan.session_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider discovery session was not found",
                false,
            )
        })?;
    if session.0 != "committing"
        || session.1 != expected_session_revision
        || session.2.as_deref() != Some(graph.plan.attempt_id.as_str())
        || session.3.as_deref() != Some(graph.plan_sha256.as_str())
        || graph.plan.expected_revision >= expected_session_revision
    {
        return Err(CoreError::invalid(
            "provider graph commit does not match the active discovery revision",
        ));
    }
    let input = serde_json::from_str::<SanitizedDiscoveryInput>(&session.4)
        .map_err(|_| corrupted("committing discovery input is invalid"))?;
    input
        .validate()
        .map_err(|_| corrupted("committing discovery input violates its contract"))?;
    validate_sanitized_input(&input)
        .map_err(|_| corrupted("committing discovery input contains forbidden data"))?;
    let session_created_at =
        parse_timestamp(&session.5, "committing discovery session creation time")?;
    validate_discovery_local_network_approval_binding(
        &input,
        session_created_at,
        authority_observed_at,
    )?;
    if graph.connection.id != input.connection_id
        || graph.connection.display_name != input.display_name
        || graph.connection.credential_ref != input.credential_ref
    {
        return Err(CoreError::invalid(
            "provider graph connection differs from the user-selected identity",
        ));
    }
    if graph.connection.config.network_mode != input.connection_options.network_mode
        || graph.connection.config.local_network_approval
            != input.connection_options.local_network_approval
    {
        return Err(CoreError::invalid(
            "provider graph network authority differs from its discovery session",
        ));
    }
    if input.connection_options.network_mode == ProviderNetworkMode::ApprovedLocalNetwork {
        let approval = input
            .connection_options
            .local_network_approval
            .as_ref()
            .ok_or_else(|| corrupted("committing LAN discovery approval is missing"))?;
        if graph.connection.created_at != session_created_at
            || graph.connection.api_origin != approval.origin
        {
            return Err(CoreError::invalid(
                "provider graph laundered its immutable LAN approval authority",
            ));
        }
    }
    require_started_session_operation(transaction, &graph.plan.session_id, "atomic_commit")?;
    let attempt = transaction
        .query_row(
            "SELECT plan_sha256, plan_json, phase
             FROM provider_discovery_commit_attempts
             WHERE id = ?1 AND session_id = ?2",
            params![
                graph.plan.attempt_id.as_str(),
                graph.plan.session_id.as_str()
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("active discovery commit attempt is missing"))?;
    if attempt.0 != graph.plan_sha256 || attempt.1 != plan_json {
        return Err(CoreError::invalid(
            "provider graph differs from its immutable commit attempt",
        ));
    }
    if !matches!(attempt.2.as_str(), "prepared" | "database_applied") {
        return Err(CoreError::invalid(
            "provider graph can only be applied from the prepared phase",
        ));
    }
    validate_review_approval(transaction, &graph.plan)?;
    validate_credential_approval(transaction, graph)?;
    validate_graph_evidence_references(transaction, graph)?;
    let requested_ownership_hash = provider_graph_ownership_hash(
        &graph.template,
        &graph.connection,
        &graph.routes,
        &graph.observations,
        &graph.presets,
    )?;
    if requested_ownership_hash != graph.plan.graph_sha256 {
        return Err(CoreError::invalid(
            "provider graph differs from the graph digest approved in the immutable commit plan",
        ));
    }
    if attempt.2 == "database_applied" {
        let stored_graph = load_discovered_provider_graph_rows(
            transaction,
            &graph.plan.template_id,
            graph.plan.template_version,
            &graph.plan.connection_id,
        )?
        .ok_or_else(|| corrupted("database-applied discovery graph is missing"))?;
        if stored_provider_graph_ownership_hash(&stored_graph)? != requested_ownership_hash
            || graph_ownership_audit_hash(transaction, &graph.plan.session_id)?
                != requested_ownership_hash
        {
            return Err(CoreError::invalid(
                "database-applied discovery graph differs from its immutable ownership record",
            ));
        }
        return Ok(());
    }
    validate_catalog_authority_in_transaction(transaction, graph, authority_observed_at)?;
    ensure_provider_graph_ids_vacant(transaction, graph)?;
    let template_existed = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM provider_templates WHERE id = ?1 AND version = ?2
             )",
            params![graph.plan.template_id.as_str(), graph.plan.template_version,],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    write_discovered_provider_graph_rows(
        transaction,
        &graph.template,
        &graph.connection,
        &graph.routes,
        &graph.observations,
        &graph.presets,
    )?;
    let stored_graph = load_discovered_provider_graph_rows(
        transaction,
        &graph.plan.template_id,
        graph.plan.template_version,
        &graph.plan.connection_id,
    )?
    .ok_or_else(|| corrupted("newly applied discovery graph is missing"))?;
    if stored_provider_graph_ownership_hash(&stored_graph)? != requested_ownership_hash {
        return Err(corrupted(
            "newly applied discovery graph does not match its requested rows",
        ));
    }
    append_audit(
        transaction,
        graph.plan.session_id.as_str(),
        expected_session_revision,
        "transition_applied",
        None,
        Some(&requested_ownership_hash),
        "discovery.audit.provider_graph_applied",
        applied_at,
    )?;
    append_audit(
        transaction,
        graph.plan.session_id.as_str(),
        expected_session_revision,
        "transition_applied",
        None,
        Some(if template_existed {
            "reused"
        } else {
            "created"
        }),
        "discovery.audit.provider_template_ownership",
        applied_at,
    )?;
    let changed = transaction
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET phase = 'database_applied', updated_at = ?2
             WHERE id = ?1 AND phase = 'prepared'",
            params![graph.plan.attempt_id.as_str(), applied_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "discovery commit phase changed concurrently",
        ));
    }
    Ok(())
}

fn validate_catalog_authority_in_transaction(
    transaction: &Transaction<'_>,
    graph: &DiscoveredProviderGraph,
    authority_observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    match (&graph.template.source, &graph.plan.catalog_authority) {
        (TemplateSource::SignedCatalog, Some(authority)) => {
            authority
                .validate_template(&graph.template)
                .map_err(contract_error)?;
            if authority_observed_at >= authority.expires_at {
                return Err(CoreError::new(
                    CoreErrorCode::InvalidInput,
                    "signed catalog authority expired before provider graph publication",
                    true,
                ));
            }
            let stored_state_version = transaction
                .query_row(
                    "SELECT state_version FROM provider_catalog_state WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(database_error)?;
            let current_state_version = u64::try_from(stored_state_version)
                .map_err(|_| corrupted("provider catalog state version is negative"))?;
            if current_state_version != authority.catalog_state_version {
                return Err(CoreError::new(
                    CoreErrorCode::InvalidInput,
                    "signed catalog authority changed before provider graph publication",
                    true,
                ));
            }
            Ok(())
        }
        (TemplateSource::SignedCatalog, None) => Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "legacy signed discovery plan has no catalog authority; restart provider discovery",
            true,
        )),
        (_, Some(_)) => Err(CoreError::invalid(
            "non-catalog provider graph cannot carry signed catalog authority",
        )),
        (_, None) => Ok(()),
    }
}

fn validate_graph_evidence_references(
    transaction: &Connection,
    graph: &DiscoveredProviderGraph,
) -> CoreResult<()> {
    for evidence_id in graph
        .observations
        .iter()
        .filter_map(|observation| observation.evidence_ref.as_ref())
    {
        let belongs = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM provider_discovery_evidence
                     WHERE id = ?1 AND session_id = ?2
                 )",
                params![evidence_id.as_str(), graph.plan.session_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if !belongs {
            return Err(CoreError::invalid(
                "capability observation evidence must belong to the committing discovery session",
            ));
        }
    }
    Ok(())
}

pub(in crate::discovery_repository) fn provider_graph_ownership_hash(
    template: &ProviderTemplate,
    connection: &ProviderConnection,
    routes: &[ModelRoute],
    observations: &[CapabilityObservation],
    presets: &[GenerationPreset],
) -> CoreResult<String> {
    let mut routes = routes.to_vec();
    routes.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let mut observations = observations.to_vec();
    observations.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let mut presets = presets.to_vec();
    presets.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let canonical = canonical_typed_json_result(
        serde_json::to_value((template, connection, routes, observations, presets)),
        "discovered provider graph ownership",
    )?;
    Ok(sha256_hex(canonical.as_bytes()))
}

pub(in crate::discovery_repository) fn stored_provider_graph_ownership_hash(
    graph: &StoredDiscoveredProviderGraphRows,
) -> CoreResult<String> {
    provider_graph_ownership_hash(
        &graph.template,
        &graph.connection,
        &graph.routes,
        &graph.observations,
        &graph.presets,
    )
}

pub(in crate::discovery_repository) fn graph_ownership_audit_hash(
    transaction: &Connection,
    session_id: &DiscoverySessionId,
) -> CoreResult<String> {
    let hashes = {
        let mut statement = transaction
            .prepare(
                "SELECT subject_id
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND summary_key = 'discovery.audit.provider_graph_applied'
                 ORDER BY audit_sequence",
            )
            .map_err(database_error)?;
        statement
            .query_map([session_id.as_str()], |row| row.get::<_, Option<String>>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    if hashes.len() != 1 {
        return Err(corrupted(
            "discovery commit must have exactly one provider graph ownership record",
        ));
    }
    let hash = hashes
        .into_iter()
        .next()
        .flatten()
        .ok_or_else(|| corrupted("provider graph ownership record has no digest"))?;
    validate_sha256("provider graph ownership digest", &hash)
        .map_err(|_| corrupted("provider graph ownership digest is invalid"))?;
    Ok(hash)
}

pub(in crate::discovery_repository) fn graph_template_was_created(
    transaction: &Connection,
    session_id: &DiscoverySessionId,
) -> CoreResult<bool> {
    let records = {
        let mut statement = transaction
            .prepare(
                "SELECT subject_id
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND summary_key = 'discovery.audit.provider_template_ownership'
                 ORDER BY audit_sequence",
            )
            .map_err(database_error)?;
        statement
            .query_map([session_id.as_str()], |row| row.get::<_, Option<String>>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    match records.as_slice() {
        [Some(value)] if value == "created" => Ok(true),
        [Some(value)] if value == "reused" => Ok(false),
        _ => Err(corrupted(
            "discovery commit has an invalid provider template ownership record",
        )),
    }
}

#[allow(clippy::too_many_lines)]
pub(in crate::discovery_repository) fn validate_provider_graph(
    graph: &DiscoveredProviderGraph,
) -> CoreResult<()> {
    graph.plan.validate().map_err(contract_error)?;
    validate_sha256("provider graph plan hash", &graph.plan_sha256)?;
    if let Some(reference) = &graph.plan.credential_ref {
        validate_opaque_credential_reference(reference.as_str())?;
    }
    validate_graph_component(serde_json::to_value(&graph.template), "provider template")?;
    validate_graph_component(
        serde_json::to_value(&graph.connection),
        "provider connection",
    )?;
    for route in &graph.routes {
        validate_graph_component(serde_json::to_value(route), "model route")?;
    }
    for preset in &graph.presets {
        validate_graph_component(serde_json::to_value(preset), "generation preset")?;
    }
    validate_persistable_discovery_url(
        graph.connection.api_origin.as_str(),
        "provider connection origin",
    )?;
    if graph.template.id != graph.plan.template_id
        || graph.template.manifest_version != graph.plan.template_version
        || graph.connection.id != graph.plan.connection_id
        || graph.connection.template_id != graph.plan.template_id
        || graph.connection.template_version != graph.plan.template_version
        || graph.connection.credential_ref != graph.plan.credential_ref
    {
        return Err(CoreError::invalid(
            "provider graph identities do not match the discovery commit plan",
        ));
    }
    let manifest_json = canonical_json_result(
        serde_json::to_value(&graph.template.default_manifest),
        "provider manifest",
    )?;
    if sha256_hex(manifest_json.as_bytes()) != graph.plan.manifest_sha256 {
        return Err(CoreError::invalid(
            "provider graph manifest does not match the validated manifest hash",
        ));
    }
    for route in &graph.routes {
        validate_discovery_route_metadata(route)?;
    }
    for observation in graph
        .observations
        .iter()
        .filter(|observation| observation.source == ObservationSource::ProviderApi)
    {
        let route = graph
            .routes
            .iter()
            .find(|route| route.id == observation.model_route_id)
            .ok_or_else(|| {
                CoreError::invalid(
                    "provider API capability observation references a route outside the graph",
                )
            })?;
        if route.metadata_source != ModelMetadataSource::ProviderApi
            || route.metadata_observed_at != Some(observation.observed_at)
            || observation.confidence != Confidence::High
            || !matches!(
                observation.status,
                SupportStatus::Verified | SupportStatus::Unsupported
            )
            || observation.evidence_ref.is_some()
            || observation
                .expires_at
                .is_none_or(|expires_at| expires_at <= observation.observed_at)
        {
            return Err(CoreError::invalid(
                "provider API capability observation provenance differs from its route metadata",
            ));
        }
    }
    for entry in &graph.connection.config.values {
        if let ConnectionConfigValue::Text(value) = &entry.value
            && looks_like_secret(value)
        {
            return Err(CoreError::invalid(
                "discovered provider connection configuration contains credential-like material",
            ));
        }
    }
    for route in &graph.routes {
        for entry in &route.route_config.values {
            if let ConnectionConfigValue::Text(value) = &entry.value
                && looks_like_secret(value)
            {
                return Err(CoreError::invalid(
                    "discovered model route configuration contains credential-like material",
                ));
            }
        }
    }
    for observation in &graph.observations {
        let value = serde_json::to_value(&observation.value)
            .map_err(|_| CoreError::internal("cannot inspect discovered capability value"))?;
        validate_redacted_value(&value)?;
    }
    let planned = graph.plan.model_route_ids.iter().collect::<BTreeSet<_>>();
    let actual = graph
        .routes
        .iter()
        .map(|route| &route.id)
        .collect::<BTreeSet<_>>();
    if planned.len() != graph.plan.model_route_ids.len()
        || actual.len() != graph.routes.len()
        || planned != actual
        || graph
            .routes
            .iter()
            .any(|route| route.connection_id != graph.connection.id)
        || graph
            .observations
            .iter()
            .any(|observation| !actual.contains(&observation.model_route_id))
        || graph
            .presets
            .iter()
            .any(|preset| !actual.contains(&preset.model_route_id))
    {
        return Err(CoreError::invalid(
            "provider graph routes and dependants do not match the commit plan",
        ));
    }
    Ok(())
}

fn validate_discovery_route_metadata(route: &ModelRoute) -> CoreResult<()> {
    if route.last_reconciled_sync_job_id.is_some() || route.metadata_sync_job_id.is_some() {
        return Err(CoreError::invalid(
            "initial discovery routes cannot claim model synchronization provenance",
        ));
    }
    match (
        route.raw_metadata.as_ref(),
        route.metadata_source,
        route.metadata_observed_at,
    ) {
        (Some(metadata), ModelMetadataSource::ProviderApi, Some(observed_at)) => {
            if route.miss_count != 0
                || route.first_seen_at != observed_at
                || route.last_seen_at != Some(observed_at)
            {
                return Err(CoreError::invalid(
                    "discovered provider API route metadata has inconsistent observation times",
                ));
            }
            validate_provider_api_route_metadata(Some(metadata))
        }
        (None, ModelMetadataSource::Legacy | ModelMetadataSource::UserOverride, None) => {
            if route.miss_count != 0 {
                return Err(CoreError::invalid(
                    "initial discovery routes cannot carry model synchronization miss counts",
                ));
            }
            Ok(())
        }
        _ => Err(CoreError::invalid(
            "discovered route metadata must be absent or a normalized provider API projection",
        )),
    }
}

pub(in crate::discovery_repository) fn validate_graph_component(
    component: Result<Value, serde_json::Error>,
    label: &str,
) -> CoreResult<()> {
    let value = component.map_err(|_| CoreError::internal(format!("cannot inspect {label}")))?;
    validate_redacted_value(&value)
        .map_err(|_| CoreError::invalid(format!("{label} contains forbidden data")))
}

pub(in crate::discovery_repository) fn ensure_discovery_attempt_graph_absent(
    transaction: &Transaction<'_>,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<()> {
    if load_discovered_provider_graph_rows(
        transaction,
        &attempt.plan.template_id,
        attempt.plan.template_version,
        &attempt.plan.connection_id,
    )?
    .is_some()
    {
        return Err(CoreError::invalid(
            "commit graph must be absent before this ledger transition",
        ));
    }
    for route_id in &attempt.plan.model_route_ids {
        let exists = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
                [route_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if exists {
            return Err(corrupted(
                "commit graph is absent but a planned route remains",
            ));
        }
    }
    Ok(())
}

pub(in crate::discovery_repository) fn verify_discovery_attempt_graph(
    transaction: &Transaction<'_>,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<()> {
    let graph = load_discovered_provider_graph_rows(
        transaction,
        &attempt.plan.template_id,
        attempt.plan.template_version,
        &attempt.plan.connection_id,
    )?
    .ok_or_else(|| CoreError::invalid("confirmed commit graph is missing"))?;
    let ownership = stored_provider_graph_ownership_hash(&graph)?;
    if ownership != attempt.plan.graph_sha256
        || graph_ownership_audit_hash(transaction, &attempt.session_id)? != ownership
    {
        return Err(CoreError::invalid(
            "confirmed commit graph differs from its approved ownership digest",
        ));
    }
    Ok(())
}

type DiscoveryAuthorityGraphAuditRow = (
    u64,
    String,
    Option<String>,
    Option<String>,
    u64,
    String,
    String,
);

pub(in crate::discovery_repository) fn validate_discovery_authority_graph_audits(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    authority_revision_bound: u64,
    authority_time_bound: DateTime<Utc>,
    applied_with_bound: bool,
    operation_start_audit_sequence: u64,
    terminal_audit_sequence: u64,
) -> CoreResult<()> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT audit_sequence, audit_kind, action_id, subject_id, session_revision,
                        summary_key, created_at
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND summary_key IN (
                       'discovery.audit.provider_graph_applied',
                       'discovery.audit.provider_template_ownership'
                   )
                 ORDER BY summary_key",
            )
            .map_err(database_error)?;
        statement
            .query_map([attempt.session_id.as_str()], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<DiscoveryAuthorityGraphAuditRow>, _>>()
            .map_err(database_error)?
    };
    if rows.len() != 2 {
        return Err(corrupted(
            "discovery credential graph ownership audits are incomplete",
        ));
    }
    let graph = rows
        .iter()
        .find(|row| row.5 == "discovery.audit.provider_graph_applied")
        .ok_or_else(|| corrupted("discovery credential graph ownership audit is missing"))?;
    let template = rows
        .iter()
        .find(|row| row.5 == "discovery.audit.provider_template_ownership")
        .ok_or_else(|| corrupted("discovery credential template ownership audit is missing"))?;
    let graph_at = parse_timestamp(&graph.6, "provider graph authority audit created_at")?;
    let template_at = parse_timestamp(&template.6, "provider template authority audit created_at")?;
    let bounded = graph.4 == authority_revision_bound
        && graph_at <= authority_time_bound
        && (!applied_with_bound || graph_at == authority_time_bound);
    if graph.1 != "transition_applied"
        || template.1 != "transition_applied"
        || graph.2.is_some()
        || template.2.is_some()
        || graph.3.as_deref() != Some(attempt.plan.graph_sha256.as_str())
        || !matches!(template.3.as_deref(), Some("created" | "reused"))
        || graph.4 != template.4
        || graph_at != template_at
        || operation_start_audit_sequence >= graph.0
        || graph.0 >= template.0
        || template.0 >= terminal_audit_sequence
        || !bounded
    {
        return Err(corrupted(
            "discovery credential graph ownership audits are detached from the terminal history",
        ));
    }
    Ok(())
}

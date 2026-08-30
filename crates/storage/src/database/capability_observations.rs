use super::{
    CapabilityKey, CapabilityObservation, CapabilityValue, Confidence, CoreError, CoreResult,
    DateTime, EvidenceId, MAX_CAPABILITY_ENUM_VALUES, MAX_CAPABILITY_VALUE_BYTES,
    MAX_CAPABILITY_VALUE_CHARS, ModelRouteId, ObservationId, ObservationSource, OptionalExtension,
    PROVIDER_API_CAPABILITY_FRESHNESS, Storage, SupportStatus, Utc, is_sensitive_configuration_key,
    not_found, params, parse_stored_datetime, row_exists, storage_corrupted, storage_db_error,
    stored_catalog_error, validate_nonempty, validate_provider_catalog_foreign_keys,
};

impl Storage {
    /// Stores one source-attributed capability observation.
    ///
    /// Observation identity, route, key, and source are immutable. Reusing an
    /// ID is idempotent for an identical value and may otherwise only advance
    /// its observation timestamp. This lets provider refreshes keep a stable
    /// per-source ID without allowing provenance to be rewritten.
    pub fn upsert_capability_observation(
        &self,
        observation: &CapabilityObservation,
    ) -> CoreResult<()> {
        self.upsert_capability_observations(std::slice::from_ref(observation))
    }

    /// Atomically stores a bounded set of capability observations.
    pub fn upsert_capability_observations(
        &self,
        observations: &[CapabilityObservation],
    ) -> CoreResult<()> {
        if observations.len() > 1_024 {
            return Err(CoreError::invalid(
                "at most 1024 capability observations may be stored at once",
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let mut ids = std::collections::BTreeSet::new();
        for observation in observations {
            if !ids.insert(observation.id.as_str()) {
                return Err(CoreError::invalid(
                    "capability observation identifiers must be unique within one write",
                ));
            }
            upsert_capability_observation_row(&transaction, observation)?;
        }
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn get_capability_observation(
        &self,
        id: &ObservationId,
    ) -> CoreResult<CapabilityObservation> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id, model_route_id, capability_key, value_json, support_status,
                        source_kind, confidence, evidence_ref, observed_at, expires_at
                 FROM model_capability_observations
                 WHERE id = ?1",
                [id.as_str()],
                capability_observation_columns,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("capability observation"))?;
        decode_capability_observation_row(row)
    }

    pub fn list_capability_observations(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        self.list_capability_observations_filtered(model_route_id, None)
    }

    /// Returns every candidate needed to compute the effective value for one
    /// route/key pair, including expired observations so callers can expose
    /// stale state instead of silently treating it as current.
    pub fn list_capability_observations_for_key(
        &self,
        model_route_id: &ModelRouteId,
        key: CapabilityKey,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        self.list_capability_observations_filtered(model_route_id, Some(key))
    }

    fn list_capability_observations_filtered(
        &self,
        model_route_id: &ModelRouteId,
        key: Option<CapabilityKey>,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        // Distinguish an empty observation list from a route that does not
        // exist. Otherwise native clients could display a missing route as
        // merely having unknown capabilities.
        self.get_model_route(model_route_id)?;
        let connection = self.connection()?;
        let rows = if let Some(key) = key {
            let mut statement = connection
                .prepare(
                    "SELECT id, model_route_id, capability_key, value_json,
                                support_status, source_kind, confidence, evidence_ref,
                                observed_at, expires_at
                         FROM model_capability_observations
                         WHERE model_route_id = ?1 AND capability_key = ?2
                         ORDER BY observed_at DESC, id",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(
                    params![model_route_id.as_str(), capability_key_to_str(key)],
                    capability_observation_columns,
                )
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        } else {
            let mut statement = connection
                .prepare(
                    "SELECT id, model_route_id, capability_key, value_json,
                                support_status, source_kind, confidence, evidence_ref,
                                observed_at, expires_at
                         FROM model_capability_observations
                         WHERE model_route_id = ?1
                         ORDER BY capability_key, observed_at DESC, id",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map([model_route_id.as_str()], capability_observation_columns)
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        rows.into_iter()
            .map(decode_capability_observation_row)
            .collect()
    }

    pub fn delete_capability_observation(&self, id: &ObservationId) -> CoreResult<()> {
        let deleted = self
            .connection()?
            .execute(
                "DELETE FROM model_capability_observations WHERE id = ?1",
                [id.as_str()],
            )
            .map_err(storage_db_error)?;
        if deleted == 0 {
            Err(not_found("capability observation"))
        } else {
            Ok(())
        }
    }
}
pub(super) type CapabilityObservationRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
);

fn validate_capability_observation(
    transaction: &rusqlite::Transaction<'_>,
    observation: &CapabilityObservation,
) -> CoreResult<()> {
    validate_bounded_identifier("capability observation id", observation.id.as_str(), 256)?;
    validate_bounded_identifier("model route id", observation.model_route_id.as_str(), 256)?;
    if !row_exists(
        transaction,
        "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
        observation.model_route_id.as_str(),
    )? {
        return Err(not_found("model route"));
    }
    if observation
        .expires_at
        .is_some_and(|expires_at| expires_at <= observation.observed_at)
    {
        return Err(CoreError::invalid(
            "capability observation expires_at must follow observed_at",
        ));
    }
    if let Some(evidence_ref) = observation.evidence_ref.as_ref() {
        validate_bounded_identifier("capability evidence reference", evidence_ref.as_str(), 512)?;
    }
    validate_capability_value(observation.key, &observation.value)?;
    if observation.status == SupportStatus::Unsupported
        && observation.value != CapabilityValue::Boolean(false)
    {
        return Err(CoreError::invalid(
            "an unsupported capability observation must carry boolean false",
        ));
    }
    Ok(())
}

pub(crate) fn validate_provider_api_snapshot_observation(
    observation: &CapabilityObservation,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    validate_bounded_identifier("capability observation id", observation.id.as_str(), 256)?;
    validate_bounded_identifier("model route id", observation.model_route_id.as_str(), 256)?;
    let expected_expires_at = observed_at
        .checked_add_signed(PROVIDER_API_CAPABILITY_FRESHNESS)
        .ok_or_else(|| {
            CoreError::invalid("provider API snapshot observation freshness cannot be represented")
        })?;
    if observation.source != ObservationSource::ProviderApi
        || observation.confidence != Confidence::High
        || observation.observed_at != observed_at
        || observation.expires_at != Some(expected_expires_at)
        || observation.evidence_ref.is_some()
    {
        return Err(CoreError::invalid(
            "provider API snapshot observation provenance or freshness is inconsistent",
        ));
    }
    validate_capability_value(observation.key, &observation.value)?;
    let shape_is_valid = matches!(
        (observation.key, observation.status, &observation.value),
        (
            CapabilityKey::ContextWindow | CapabilityKey::MaxOutputTokens,
            SupportStatus::Verified,
            CapabilityValue::Integer(_),
        ) | (
            CapabilityKey::Reasoning,
            SupportStatus::Verified,
            CapabilityValue::Structured(_),
        ) | (
            CapabilityKey::Reasoning
                | CapabilityKey::ToolCalling
                | CapabilityKey::ParallelToolCalling
                | CapabilityKey::StructuredOutput
                | CapabilityKey::JsonMode
                | CapabilityKey::Logprobs
                | CapabilityKey::Seed,
            SupportStatus::Verified,
            CapabilityValue::Boolean(true),
        ) | (
            CapabilityKey::Reasoning
                | CapabilityKey::ToolCalling
                | CapabilityKey::ParallelToolCalling
                | CapabilityKey::StructuredOutput
                | CapabilityKey::JsonMode
                | CapabilityKey::Logprobs
                | CapabilityKey::Seed,
            SupportStatus::Unsupported,
            CapabilityValue::Boolean(false),
        )
    );
    if !shape_is_valid {
        return Err(CoreError::invalid(
            "provider API snapshot observation key, status, or value is inconsistent",
        ));
    }
    Ok(())
}

pub(super) fn validate_provider_api_snapshot_observations_for_routes(
    observations: &[CapabilityObservation],
    listed_route_ids: &std::collections::BTreeSet<&str>,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    let mut observation_ids = std::collections::BTreeSet::new();
    for observation in observations {
        if !listed_route_ids.contains(observation.model_route_id.as_str())
            || !observation_ids.insert(observation.id.as_str())
        {
            return Err(CoreError::invalid(
                "model refresh capability observations must be unique and belong to a listed route",
            ));
        }
        validate_provider_api_snapshot_observation(observation, observed_at)?;
    }
    Ok(())
}

fn validate_bounded_identifier(label: &str, value: &str, max_bytes: usize) -> CoreResult<()> {
    validate_nonempty(label, value)?;
    if value.trim() != value || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(CoreError::invalid(format!(
            "{label} is oversized, contains control characters, or is not trimmed"
        )));
    }
    Ok(())
}

fn validate_capability_value(key: CapabilityKey, value: &CapabilityValue) -> CoreResult<()> {
    match (key, value) {
        (
            CapabilityKey::ContextWindow | CapabilityKey::MaxOutputTokens,
            CapabilityValue::Integer(value),
        ) if *value > 0 => {}
        (
            CapabilityKey::ContextWindow | CapabilityKey::MaxOutputTokens,
            CapabilityValue::Structured(value),
        ) => validate_capability_structured_value(value)?,
        (CapabilityKey::ContextWindow | CapabilityKey::MaxOutputTokens, _) => {
            return Err(CoreError::invalid(
                "token-limit capabilities require a positive integer or structured value",
            ));
        }
        (_, CapabilityValue::Integer(_)) => {
            return Err(CoreError::invalid(
                "integer capability values are reserved for numeric token limits",
            ));
        }
        (_, CapabilityValue::EnumValues(values)) => {
            if values.is_empty() || values.len() > MAX_CAPABILITY_ENUM_VALUES {
                return Err(CoreError::invalid(
                    "capability enum values must contain from 1 to 128 entries",
                ));
            }
            let mut unique = std::collections::BTreeSet::new();
            for value in values {
                validate_bounded_identifier("capability enum value", value, 256)?;
                if !unique.insert(value.as_str()) {
                    return Err(CoreError::invalid("capability enum values must be unique"));
                }
            }
        }
        (_, CapabilityValue::Structured(value)) => validate_capability_structured_value(value)?,
        (_, CapabilityValue::Boolean(_)) => {}
    }
    Ok(())
}

fn validate_capability_structured_value(value: &serde_json::Value) -> CoreResult<()> {
    if !value.is_object() {
        return Err(CoreError::invalid(
            "structured capability values must be JSON objects",
        ));
    }
    let encoded = serde_json::to_string(value).map_err(|error| {
        CoreError::invalid(format!("structured capability value is invalid: {error}"))
    })?;
    if encoded.len() > MAX_CAPABILITY_VALUE_BYTES
        || encoded.chars().count() > MAX_CAPABILITY_VALUE_CHARS
    {
        return Err(CoreError::invalid(
            "structured capability value exceeds the storage limit",
        ));
    }
    let mut pending = vec![(value, 0_usize)];
    let mut visited = 0_usize;
    while let Some((node, depth)) = pending.pop() {
        visited = visited.saturating_add(1);
        if visited > 2_048 || depth > 16 {
            return Err(CoreError::invalid(
                "structured capability value exceeds nesting or node limits",
            ));
        }
        match node {
            serde_json::Value::Object(object) => {
                for (key, child) in object {
                    if is_sensitive_configuration_key(key) {
                        return Err(CoreError::invalid(
                            "raw credentials and secret-like fields must never be stored in capability metadata",
                        ));
                    }
                    pending.push((child, depth.saturating_add(1)));
                }
            }
            serde_json::Value::Array(array) => {
                pending.extend(array.iter().map(|child| (child, depth.saturating_add(1))));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn upsert_capability_observation_row(
    transaction: &rusqlite::Transaction<'_>,
    observation: &CapabilityObservation,
) -> CoreResult<()> {
    validate_capability_observation(transaction, observation)?;
    let existing = transaction
        .query_row(
            "SELECT id, model_route_id, capability_key, value_json, support_status,
                    source_kind, confidence, evidence_ref, observed_at, expires_at
             FROM model_capability_observations
             WHERE id = ?1",
            [observation.id.as_str()],
            capability_observation_columns,
        )
        .optional()
        .map_err(storage_db_error)?
        .map(decode_capability_observation_row)
        .transpose()?;
    if let Some(existing) = existing.as_ref() {
        if existing.model_route_id != observation.model_route_id
            || existing.key != observation.key
            || existing.source != observation.source
        {
            return Err(CoreError::invalid(
                "an existing capability observation cannot change route, key, or source",
            ));
        }
        if observation.observed_at < existing.observed_at {
            return Err(CoreError::invalid(
                "capability observation updates must not move observed_at backwards",
            ));
        }
        if observation.observed_at == existing.observed_at {
            if existing == observation {
                return Ok(());
            }
            return Err(CoreError::invalid(
                "a capability observation cannot change without advancing observed_at",
            ));
        }
    }

    let value_json = serde_json::to_string(&observation.value).map_err(|error| {
        CoreError::internal(format!(
            "cannot encode capability observation value: {error}"
        ))
    })?;
    transaction
        .execute(
            "INSERT INTO model_capability_observations
             (id, model_route_id, capability_key, value_json, support_status,
              source_kind, confidence, evidence_ref, observed_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
               value_json = excluded.value_json,
               support_status = excluded.support_status,
               confidence = excluded.confidence,
               evidence_ref = excluded.evidence_ref,
               observed_at = excluded.observed_at,
               expires_at = excluded.expires_at",
            params![
                observation.id.as_str(),
                observation.model_route_id.as_str(),
                capability_key_to_str(observation.key),
                value_json,
                support_status_to_str(observation.status),
                observation_source_to_str(observation.source),
                confidence_to_str(observation.confidence),
                observation.evidence_ref.as_ref().map(EvidenceId::as_str),
                observation.observed_at.to_rfc3339(),
                observation.expires_at.map(|value| value.to_rfc3339()),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(super) fn capability_observation_columns(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CapabilityObservationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

pub(super) fn decode_capability_observation_row(
    row: CapabilityObservationRow,
) -> CoreResult<CapabilityObservation> {
    let (
        id,
        model_route_id,
        key,
        value_json,
        status,
        source,
        confidence,
        evidence_ref,
        observed_at,
        expires_at,
    ) = row;
    let id = ObservationId::from(id);
    let model_route_id = ModelRouteId::from(model_route_id);
    let key = str_to_capability_key(&key)?;
    let value = serde_json::from_str::<CapabilityValue>(&value_json).map_err(|error| {
        storage_corrupted(format!(
            "stored capability observation value is invalid: {error}"
        ))
    })?;
    let status = str_to_support_status(&status)?;
    let source = str_to_observation_source(&source)?;
    let confidence = str_to_confidence(&confidence)?;
    let observed_at = parse_stored_datetime(&observed_at, "capability observed_at")?;
    let expires_at = expires_at
        .map(|value| parse_stored_datetime(&value, "capability expires_at"))
        .transpose()?;
    let observation = CapabilityObservation {
        id,
        model_route_id,
        key,
        value,
        status,
        source,
        confidence,
        observed_at,
        expires_at,
        evidence_ref: evidence_ref.map(EvidenceId::from),
    };
    if observation
        .expires_at
        .is_some_and(|expires_at| expires_at <= observation.observed_at)
    {
        return Err(storage_corrupted(
            "stored capability observation timestamps are inconsistent",
        ));
    }
    if observation.status == SupportStatus::Unsupported
        && observation.value != CapabilityValue::Boolean(false)
    {
        return Err(storage_corrupted(
            "stored unsupported capability observation has a non-false value",
        ));
    }
    validate_capability_value(observation.key, &observation.value).map_err(stored_catalog_error)?;
    Ok(observation)
}

const fn capability_key_to_str(key: CapabilityKey) -> &'static str {
    match key {
        CapabilityKey::Streaming => "streaming",
        CapabilityKey::Reasoning => "reasoning",
        CapabilityKey::PromptCaching => "prompt_caching",
        CapabilityKey::ToolCalling => "tool_calling",
        CapabilityKey::ParallelToolCalling => "parallel_tool_calling",
        CapabilityKey::StructuredOutput => "structured_output",
        CapabilityKey::JsonMode => "json_mode",
        CapabilityKey::ImageInput => "image_input",
        CapabilityKey::AudioInput => "audio_input",
        CapabilityKey::AudioOutput => "audio_output",
        CapabilityKey::Logprobs => "logprobs",
        CapabilityKey::Seed => "seed",
        CapabilityKey::Batch => "batch",
        CapabilityKey::Background => "background",
        CapabilityKey::ContextWindow => "context_window",
        CapabilityKey::MaxOutputTokens => "max_output_tokens",
    }
}

fn str_to_capability_key(value: &str) -> CoreResult<CapabilityKey> {
    match value {
        "streaming" => Ok(CapabilityKey::Streaming),
        "reasoning" => Ok(CapabilityKey::Reasoning),
        "prompt_caching" => Ok(CapabilityKey::PromptCaching),
        "tool_calling" => Ok(CapabilityKey::ToolCalling),
        "parallel_tool_calling" => Ok(CapabilityKey::ParallelToolCalling),
        "structured_output" => Ok(CapabilityKey::StructuredOutput),
        "json_mode" => Ok(CapabilityKey::JsonMode),
        "image_input" => Ok(CapabilityKey::ImageInput),
        "audio_input" => Ok(CapabilityKey::AudioInput),
        "audio_output" => Ok(CapabilityKey::AudioOutput),
        "logprobs" => Ok(CapabilityKey::Logprobs),
        "seed" => Ok(CapabilityKey::Seed),
        "batch" => Ok(CapabilityKey::Batch),
        "background" => Ok(CapabilityKey::Background),
        "context_window" => Ok(CapabilityKey::ContextWindow),
        "max_output_tokens" => Ok(CapabilityKey::MaxOutputTokens),
        _ => Err(storage_corrupted(format!(
            "stored capability key is invalid: {value}"
        ))),
    }
}

const fn support_status_to_str(status: SupportStatus) -> &'static str {
    match status {
        SupportStatus::Verified => "verified",
        SupportStatus::Documented => "documented",
        SupportStatus::Inferred => "inferred",
        SupportStatus::Unsupported => "unsupported",
        SupportStatus::Unknown => "unknown",
        SupportStatus::Conditional => "conditional",
    }
}

fn str_to_support_status(value: &str) -> CoreResult<SupportStatus> {
    match value {
        "verified" => Ok(SupportStatus::Verified),
        "documented" => Ok(SupportStatus::Documented),
        "inferred" => Ok(SupportStatus::Inferred),
        "unsupported" => Ok(SupportStatus::Unsupported),
        "unknown" => Ok(SupportStatus::Unknown),
        "conditional" => Ok(SupportStatus::Conditional),
        _ => Err(storage_corrupted(format!(
            "stored capability support status is invalid: {value}"
        ))),
    }
}

const fn observation_source_to_str(source: ObservationSource) -> &'static str {
    match source {
        ObservationSource::ProviderApi => "provider_api",
        ObservationSource::OfficialDocumentation => "official_documentation",
        ObservationSource::SignedLorepiaCatalog => "signed_lorepia_catalog",
        ObservationSource::CapabilityProbe => "capability_probe",
        ObservationSource::UserOverride => "user_override",
        ObservationSource::LlmInference => "llm_inference",
    }
}

fn str_to_observation_source(value: &str) -> CoreResult<ObservationSource> {
    match value {
        "provider_api" => Ok(ObservationSource::ProviderApi),
        "official_documentation" => Ok(ObservationSource::OfficialDocumentation),
        "signed_lorepia_catalog" => Ok(ObservationSource::SignedLorepiaCatalog),
        "capability_probe" => Ok(ObservationSource::CapabilityProbe),
        "user_override" => Ok(ObservationSource::UserOverride),
        "llm_inference" => Ok(ObservationSource::LlmInference),
        _ => Err(storage_corrupted(format!(
            "stored capability observation source is invalid: {value}"
        ))),
    }
}

const fn confidence_to_str(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Low => "low",
        Confidence::Medium => "medium",
        Confidence::High => "high",
    }
}

fn str_to_confidence(value: &str) -> CoreResult<Confidence> {
    match value {
        "low" => Ok(Confidence::Low),
        "medium" => Ok(Confidence::Medium),
        "high" => Ok(Confidence::High),
        _ => Err(storage_corrupted(format!(
            "stored capability observation confidence is invalid: {value}"
        ))),
    }
}

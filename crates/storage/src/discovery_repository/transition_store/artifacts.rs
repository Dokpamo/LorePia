//! Evidence and candidate inserts owned by one discovery transition.

use super::{
    CoreError, CoreResult, DiscoveryEvidenceRecord, OptionalExtension, StoredDiscoveryCandidate,
    Transaction, append_audit, candidate_kind, contract_error, database_error, encode_json_result,
    encode_redacted_json, params, require_session, validate_candidate_evidence_references,
    validate_discovery_evidence,
};

pub(super) fn insert_evidence_in_transaction(
    transaction: &Transaction<'_>,
    evidence: &DiscoveryEvidenceRecord,
) -> CoreResult<()> {
    validate_discovery_evidence(evidence)?;
    require_session(transaction, evidence.session_id.as_str())?;
    let extracted_json = encode_redacted_json(&evidence.extracted_json, "discovery evidence")?;
    let existing = transaction
        .query_row(
            "SELECT session_id, kind, source_url, content_sha256, extracted_json, fetched_at
             FROM provider_discovery_evidence WHERE id = ?1",
            [evidence.id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let fetched_at = evidence.fetched_at.to_rfc3339();
    if let Some(existing) = existing {
        if existing
            == (
                evidence.session_id.as_str().to_owned(),
                evidence.kind.as_str().to_owned(),
                evidence.source_url.as_str().to_owned(),
                evidence.content_sha256.clone(),
                extracted_json,
                fetched_at,
            )
        {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "discovery evidence identifiers are immutable",
        ));
    }
    transaction
        .execute(
            "INSERT INTO provider_discovery_evidence (
                 id, session_id, kind, source_url, content_sha256,
                 extracted_json, redaction_version, fetched_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
            params![
                evidence.id.as_str(),
                evidence.session_id.as_str(),
                evidence.kind.as_str(),
                evidence.source_url.as_str(),
                evidence.content_sha256,
                extracted_json,
                fetched_at,
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) fn insert_candidate_in_transaction(
    transaction: &Transaction<'_>,
    candidate: &StoredDiscoveryCandidate,
    expected_revision: u64,
) -> CoreResult<()> {
    candidate.candidate.validate().map_err(contract_error)?;
    if candidate.proposed_revision != expected_revision {
        return Err(CoreError::invalid(
            "transition candidate revision does not match the source revision",
        ));
    }
    validate_candidate_evidence_references(transaction, candidate)?;
    let summary_json = encode_json_result(
        serde_json::to_value(&candidate.candidate.summary),
        "discovery candidate summary",
    )?;
    let evidence_ids_json = encode_json_result(
        serde_json::to_value(&candidate.candidate.evidence_ids),
        "candidate evidence references",
    )?;
    let kind = candidate_kind(&candidate.candidate);
    let created_at = candidate.candidate.created_at.to_rfc3339();
    let existing = transaction
        .query_row(
            "SELECT session_id, candidate_kind, summary_json, evidence_ids_json,
                    proposed_revision, created_at
             FROM provider_discovery_candidates WHERE id = ?1",
            [candidate.candidate.id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    if let Some(existing) = existing {
        if existing
            == (
                candidate.candidate.session_id.as_str().to_owned(),
                kind.to_owned(),
                summary_json,
                evidence_ids_json,
                candidate.proposed_revision,
                created_at,
            )
        {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "discovery candidate identifiers are immutable",
        ));
    }
    transaction
        .execute(
            "INSERT INTO provider_discovery_candidates (
                 id, session_id, candidate_kind, summary_json, evidence_ids_json,
                 proposed_revision, redaction_version, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
            params![
                candidate.candidate.id.as_str(),
                candidate.candidate.session_id.as_str(),
                kind,
                summary_json,
                evidence_ids_json,
                candidate.proposed_revision,
                created_at,
            ],
        )
        .map_err(database_error)?;
    append_audit(
        transaction,
        candidate.candidate.session_id.as_str(),
        candidate.proposed_revision,
        "candidate_recorded",
        None,
        Some(candidate.candidate.id.as_str()),
        "discovery.audit.candidate_recorded",
        candidate.candidate.created_at,
    )
}

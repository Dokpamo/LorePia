//! Completed package approval, commit evidence, and CAS authority snapshot queries.

use super::{
    BTreeMap, CompletedPackageAssetAuthority, CompletedPackageAssetSourceAuthority,
    CompletedPackageAuthority, CompletedPackageAuthoritySnapshot, CompletedPackageCasFile,
    CompletedPackageComponentAuthority, CompletedPackageDocumentAuthority, Connection, CoreResult,
    OptionalExtension, PackageImportStatus, Storage, Value, VersionedJson, component_kind_str,
    decode_json, encode_json, load_selected_commit_components, not_found, params, parse_datetime,
    read_approval_payload, read_import_state, read_package_source_by_id, sha256_hex,
    storage_corrupted, storage_db_error, u64_from_i64, validate_completed_authority_audit,
    validate_identifier, validate_sha256,
};

pub(super) type CompletedAuthorityCommitEvidence =
    BTreeMap<(String, u32), (CompletedPackageDocumentAuthority, Value)>;

impl Storage {
    #[allow(clippy::too_many_lines)] // Every immutable approval and commit seam is revalidated.
    pub(super) fn get_completed_package_authority_by_approval_id_in_connection(
        connection: &Connection,
        approval_id: &str,
    ) -> CoreResult<CompletedPackageAuthoritySnapshot> {
        validate_identifier("package approval", approval_id)?;
        let approval_row = connection
            .query_row(
                "SELECT import_id, inspection_sha256, selection_sha256,
                        capability_review_sha256, approved_at
                 FROM package_import_approvals
                 WHERE id = ?1",
                [approval_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("package approval"))?;
        let current = read_import_state(connection, &approval_row.0)?;
        let completed_at = connection
            .query_row(
                "SELECT completed_at FROM package_imports WHERE id = ?1",
                [approval_row.0.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                storage_corrupted("completed package authority has no completion timestamp")
            })?;
        parse_datetime("package import completed_at", &completed_at)?;
        if current.record.status != PackageImportStatus::Completed
            || current.inspection_sha256 != approval_row.1
            || current.selection_sha256.as_deref() != Some(approval_row.2.as_str())
            || current.capability_review_sha256 != approval_row.3
            || current.approved_selection_sha256.as_deref() != Some(approval_row.2.as_str())
            || current.approved_at
                != Some(parse_datetime(
                    "package approval approved_at",
                    &approval_row.4,
                )?)
        {
            return Err(storage_corrupted(
                "package approval is not the exact authority for a completed import",
            ));
        }
        let source = read_package_source_by_id(connection, &current.package_source_id)?;
        validate_sha256("package source", &source.source_sha256)
            .map_err(|_| storage_corrupted("completed package source digest is invalid"))?;
        let source_cas = connection
            .query_row(
                "SELECT relative_path, size_bytes
                 FROM content_sources
                 WHERE sha256 = ?1",
                [source.source_sha256.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(storage_db_error)?;
        if u64_from_i64("package source size", source_cas.1)? != source.source_size_bytes {
            return Err(storage_corrupted(
                "completed package source CAS metadata differs from its package record",
            ));
        }
        let mut cas_files = vec![CompletedPackageCasFile {
            namespace: "sources",
            sha256: source.source_sha256.clone(),
            size_bytes: source.source_size_bytes,
            relative_path: source_cas.0,
        }];
        let approval = read_approval_payload(connection, &approval_row.0)?;
        if approval.plan.approval_id != approval_id
            || approval.plan.review_sha256.as_str() != approval_row.1
            || approval.plan.plan_sha256.as_str() != approval_row.2
            || approval.plan.source_sha256.as_str() != source.source_sha256
            || approval.plan.package_id != source.package_id
            || approval.plan.package_id != current.record.package_id
        {
            return Err(storage_corrupted(
                "completed package approval payload differs from its durable identity",
            ));
        }
        let mut committed_assets = Vec::with_capacity(approval.plan.assets.len());
        for asset in &approval.plan.assets {
            let stored = connection
                .query_row(
                    "SELECT cas.relative_path, cas.media_type, cas.size_bytes,
                            descriptor.payload_json
                     FROM assets AS cas
                     JOIN asset_descriptors AS descriptor
                       ON descriptor.asset_hash = cas.sha256
                     WHERE cas.sha256 = ?1 AND descriptor.id = ?2",
                    params![asset.sha256.as_str(), asset.id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    storage_corrupted("completed package approval asset CAS row is missing")
                })?;
            if stored.1 != asset.media_type
                || u64_from_i64("package asset size", stored.2)? != asset.size_bytes
            {
                return Err(storage_corrupted(
                    "completed package approval asset metadata differs from its descriptor",
                ));
            }
            cas_files.push(CompletedPackageCasFile {
                namespace: "assets",
                sha256: asset.sha256.as_str().to_owned(),
                size_bytes: asset.size_bytes,
                relative_path: stored.0,
            });
            let expected_descriptor = encode_json("completed package asset descriptor", asset)?;
            if stored.3 != expected_descriptor {
                return Err(storage_corrupted(
                    "completed package asset descriptor differs from approval",
                ));
            }
            let source_components = approval
                .plan
                .components
                .iter()
                .filter(|component| component.component.asset_ids.contains(&asset.id))
                .map(|component| CompletedPackageAssetSourceAuthority {
                    component_id: component.component.id.clone(),
                    component_sha256: component.component.sha256.as_str().to_owned(),
                })
                .collect();
            committed_assets.push(CompletedPackageAssetAuthority {
                asset_id: asset.id.clone(),
                descriptor: asset.clone(),
                descriptor_sha256: sha256_hex(expected_descriptor.as_bytes()),
                cas_sha256: asset.sha256.as_str().to_owned(),
                source_components,
            });
        }

        let selected_rows = load_selected_commit_components(connection, &approval_row.0)?;
        if selected_rows.len() != approval.plan.components.len() {
            return Err(storage_corrupted(
                "completed package approval component count differs from selection",
            ));
        }
        for planned in &approval.plan.components {
            let row = selected_rows.get(&planned.component.id).ok_or_else(|| {
                storage_corrupted("completed package approval component is missing from selection")
            })?;
            if !row.selected
                || !matches!(row.disposition.as_str(), "create" | "update" | "conflict")
                || row.component_kind != component_kind_str(planned.component.kind)
                || sha256_hex(row.review_json.as_bytes()) != row.review_sha256
            {
                return Err(storage_corrupted(
                    "completed package component review metadata is invalid",
                ));
            }
            let descriptor: lorepia_orchestration::PackageComponentDescriptor =
                decode_json("package component review", &row.review_json)?;
            if descriptor != planned.component {
                return Err(storage_corrupted(
                    "completed package component differs from its approved descriptor",
                ));
            }
        }

        let mut statement = connection
            .prepare(
                "SELECT component.source_component_key,
                        committed_document.document_ordinal,
                        committed_document.target_object_id,
                        committed_document.target_revision_id,
                        committed_document.result_json,
                        committed_document.result_sha256
                 FROM package_import_component_commits AS committed_document
                 JOIN package_import_components AS component
                   ON component.import_id = committed_document.import_id
                  AND component.ordinal = committed_document.component_ordinal
                 WHERE committed_document.import_id = ?1
                 ORDER BY component.ordinal, committed_document.document_ordinal",
            )
            .map_err(storage_db_error)?;
        let committed_rows = statement
            .query_map([approval_row.0.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        let mut committed_documents = CompletedAuthorityCommitEvidence::new();
        for row in committed_rows {
            validate_sha256("package component commit result", &row.5)?;
            if sha256_hex(row.4.as_bytes()) != row.5 {
                return Err(storage_corrupted(
                    "completed package component result hash does not match",
                ));
            }
            let result: VersionedJson = decode_json("package component commit result", &row.4)?;
            if result.schema_version != 1
                || result
                    .value
                    .get("source_component_key")
                    .and_then(Value::as_str)
                    != Some(row.0.as_str())
                || result
                    .value
                    .get("component_document_ordinal")
                    .and_then(Value::as_u64)
                    != Some(u64::from(row.1))
                || result.value.get("target_object_id").and_then(Value::as_str)
                    != Some(row.2.as_str())
                || result
                    .value
                    .get("target_revision_id")
                    .and_then(Value::as_str)
                    != Some(row.3.as_str())
            {
                return Err(storage_corrupted(
                    "completed package component result differs from its typed columns",
                ));
            }
            let source_component_sha256 = result
                .value
                .get("source_component_sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    storage_corrupted("completed package component source hash is missing")
                })?
                .to_owned();
            let document_sha256 = result
                .value
                .get("document_sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| storage_corrupted("completed package document hash is missing"))?
                .to_owned();
            validate_sha256("package component", &source_component_sha256)
                .map_err(|_| storage_corrupted("completed package component hash is invalid"))?;
            validate_sha256("package document", &document_sha256)
                .map_err(|_| storage_corrupted("completed package document hash is invalid"))?;
            let authority = CompletedPackageDocumentAuthority {
                document_ordinal: row.1,
                target_object_id: row.2,
                target_revision_id: row.3,
                source_component_sha256,
                document_sha256,
                result_sha256: row.5,
            };
            if committed_documents
                .insert((row.0, row.1), (authority, result.value))
                .is_some()
            {
                return Err(storage_corrupted(
                    "completed package component result identity is duplicated",
                ));
            }
        }
        if committed_documents.len() != approval.document_bindings.len() {
            return Err(storage_corrupted(
                "completed package commit evidence count differs from approval",
            ));
        }
        for binding in &approval.document_bindings {
            let (document, _) = committed_documents
                .get(&(
                    binding.source_component_key.clone(),
                    binding.component_document_ordinal,
                ))
                .ok_or_else(|| {
                    storage_corrupted("completed package approval binding has no commit evidence")
                })?;
            if document.target_object_id != binding.target_object_id
                || document.source_component_sha256 != binding.source_component_sha256
                || document.document_sha256 != binding.document_sha256
            {
                return Err(storage_corrupted(
                    "completed package commit evidence differs from approval binding",
                ));
            }
        }
        validate_completed_authority_audit(connection, &current, &approval, &committed_documents)?;

        let enabled_components = approval
            .plan
            .components
            .iter()
            .filter(|component| component.enabled)
            .map(|component| {
                let mut documents = committed_documents
                    .iter()
                    .filter(|((component_id, _), _)| component_id == &component.component.id)
                    .map(|(_, (document, _))| document.clone())
                    .collect::<Vec<_>>();
                documents.sort_by_key(|document| document.document_ordinal);
                CompletedPackageComponentAuthority {
                    component_id: component.component.id.clone(),
                    kind: component.component.kind,
                    sha256: component.component.sha256.as_str().to_owned(),
                    committed_documents: documents,
                }
            })
            .collect();
        Ok(CompletedPackageAuthoritySnapshot {
            authority: CompletedPackageAuthority {
                approval_id: approval_id.to_owned(),
                import_id: approval_row.0,
                package_id: approval.plan.package_id,
                status: current.record.status,
                import_revision: current.record.revision,
                source_sha256: source.source_sha256,
                inspection_sha256: approval_row.1,
                selection_sha256: approval_row.2,
                capability_review_sha256: approval_row.3,
                approval_sha256: approval.plan.approval_sha256.as_str().to_owned(),
                required_capabilities: approval.plan.required_capabilities,
                approved_capabilities: approval.approved_capabilities,
                enabled_components,
                committed_assets,
            },
            cas_files,
        })
    }
}

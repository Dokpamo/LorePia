//! Public and transaction-local imported module authority queries.

use super::{
    ActiveContentModuleRevision, CoreError, CoreResult, MAX_COMPLETED_MODULE_AUTHORITIES,
    ModuleImportApprovalEvidence, Storage, Transaction, VerifiedCompletedPackageAuthorities,
    build_module_import_approval_evidence_in_connection, params, storage_corrupted,
    storage_db_error, validate_completed_module_authority_target,
};

impl Storage {
    /// Builds the exact imported-module authority consumed by the pure module
    /// resolver. The caller supplies only a stored immutable module revision
    /// and an approval id; all package and component evidence is reloaded.
    pub fn get_module_import_approval_evidence(
        &self,
        approval_id: &str,
        stored: &ActiveContentModuleRevision,
    ) -> CoreResult<ModuleImportApprovalEvidence> {
        let verified = self.verify_completed_package_authority_with(
            approval_id,
            |connection, approval_id| {
                Self::get_completed_package_authority_by_approval_id_in_connection(
                    connection,
                    approval_id,
                )
            },
            || {},
        )?;
        let connection = self.connection()?;
        let authority = Self::revalidate_completed_package_authority_in_connection(
            &connection,
            approval_id,
            &verified,
        )?;
        build_module_import_approval_evidence_in_connection(&connection, stored, &authority)
    }

    /// Lists every completed package authority that committed this exact
    /// imported module revision.
    ///
    /// The deterministic list exists for restart and lost-response recovery.
    /// Callers must present multiple candidates for an explicit choice; this
    /// method never selects an approval merely because it is newest.
    pub fn list_completed_package_import_authorities_for_module_revision(
        &self,
        stored: &ActiveContentModuleRevision,
    ) -> CoreResult<Vec<ModuleImportApprovalEvidence>> {
        validate_completed_module_authority_target(stored)?;
        let candidate_limit = i64::try_from(MAX_COMPLETED_MODULE_AUTHORITIES + 1)
            .map_err(|_| CoreError::internal("completed module authority limit overflow"))?;
        let approval_ids = {
            let connection = self.connection()?;
            let mut statement = connection
                .prepare(
                    "SELECT DISTINCT approval.id, approval.approved_at
                     FROM package_import_approvals AS approval
                     JOIN package_imports AS import
                       ON import.id = approval.import_id
                     JOIN package_sources AS source
                       ON source.id = import.package_source_id
                     JOIN package_import_component_commits AS committed_document
                       ON committed_document.import_id = import.id
                     JOIN package_import_components AS component
                       ON component.import_id = committed_document.import_id
                      AND component.ordinal =
                          committed_document.component_ordinal
                     WHERE import.state = 'completed'
                       AND source.source_hash = ?1
                       AND component.component_kind = 'content_module'
                       AND committed_document.target_object_id = ?2
                       AND committed_document.target_revision_id = ?3
                     ORDER BY approval.approved_at, approval.id
                     LIMIT ?4",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(
                    params![
                        stored.module_revision.source_hash.as_str(),
                        stored.object.value.id.as_str(),
                        stored.module_revision.id.as_str(),
                        candidate_limit,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        if approval_ids.len() > MAX_COMPLETED_MODULE_AUTHORITIES {
            return Err(storage_corrupted(
                "completed module authority candidates exceed the bounded recovery limit",
            ));
        }
        let verified =
            self.verify_completed_package_authorities(approval_ids.iter().map(String::as_str))?;
        let connection = self.connection()?;
        approval_ids
            .into_iter()
            .map(|approval_id| {
                let verified = verified.get(&approval_id).ok_or_else(|| {
                    storage_corrupted("completed module authority was not CAS-verified")
                })?;
                let authority = Self::revalidate_completed_package_authority_in_connection(
                    &connection,
                    &approval_id,
                    verified,
                )?;
                build_module_import_approval_evidence_in_connection(&connection, stored, &authority)
            })
            .collect()
    }

    /// Transaction-local variant used while package-backed module activation
    /// is re-reviewed under the same database snapshot as its bindings.
    pub(crate) fn get_module_import_approval_evidence_in_transaction(
        transaction: &Transaction<'_>,
        approval_id: &str,
        stored: &ActiveContentModuleRevision,
        verified_authorities: &VerifiedCompletedPackageAuthorities,
    ) -> CoreResult<ModuleImportApprovalEvidence> {
        // No CAS path is opened here. The transaction performs only an exact
        // metadata/revision revalidation of the proof created before it began.
        let verified = verified_authorities.get(approval_id).ok_or_else(|| {
            CoreError::invalid(
                "module package approval changed after CAS authority preverification",
            )
        })?;
        let authority = Self::revalidate_completed_package_authority_in_connection(
            transaction,
            approval_id,
            verified,
        )?;
        build_module_import_approval_evidence_in_connection(transaction, stored, &authority)
    }
}

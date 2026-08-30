use super::{
    CanonicalOrigin, ConnectionConfig, ConnectionStatus, CoreError, CoreErrorCode, CoreResult,
    CredentialRef, CredentialScope, DateTime, GenerationRecord, IpAddr, OptionalExtension,
    ProviderConnection, ProviderConnectionId, ProviderTemplate, ProviderTemplateId, Storage,
    TemplateSource, TransactionBehavior, Utc, clear_provider_selections_for_connection, not_found,
    params, parse_stored_datetime, row_exists, save_provider_template_row, storage_corrupted,
    storage_db_error, stored_catalog_error, validate_connection_config,
    validate_provider_catalog_foreign_keys, validate_provider_connection,
    validate_provider_network_contract,
};

impl Storage {
    pub fn list_provider_connections(&self) -> CoreResult<Vec<ProviderConnection>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, template_id, template_version, display_name, api_origin,
                        config_json, credential_ref, credential_scope_json, timeout_seconds,
                        status, created_at, updated_at
                 FROM provider_connections
                 WHERE archived_at IS NULL
                 ORDER BY display_name COLLATE NOCASE, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], provider_connection_columns)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter()
            .map(decode_provider_connection_row)
            .collect()
    }

    pub fn get_provider_connection(
        &self,
        id: &ProviderConnectionId,
    ) -> CoreResult<ProviderConnection> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id, template_id, template_version, display_name, api_origin,
                        config_json, credential_ref, credential_scope_json, timeout_seconds,
                        status, created_at, updated_at
                 FROM provider_connections
                 WHERE id = ?1 AND archived_at IS NULL",
                [id.as_str()],
                provider_connection_columns,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("provider connection"))?;
        decode_provider_connection_row(row)
    }

    pub fn save_provider_connection(
        &self,
        connection_value: &ProviderConnection,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        validate_provider_connection(&transaction, connection_value)?;
        upsert_provider_connection_row(&transaction, connection_value)?;
        transaction.commit().map_err(storage_db_error)
    }

    /// Inserts a newly reviewed connection without permitting an existing
    /// identity to be overwritten.
    pub fn insert_provider_connection(
        &self,
        connection_value: &ProviderConnection,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        validate_provider_connection(&transaction, connection_value)?;
        ensure_provider_connection_id_vacant(&transaction, &connection_value.id)?;
        upsert_provider_connection_row(&transaction, connection_value)?;
        transaction.commit().map_err(storage_db_error)
    }

    /// Save a connection only while the reviewed provider catalog state is
    /// still active.
    ///
    /// Signed templates are copied into the immutable template table solely
    /// to satisfy the connection foreign key. This transaction-level CAS
    /// prevents a concurrent catalog rollback from creating a new connection
    /// against a template which is no longer active.
    pub fn save_provider_connection_for_catalog_state(
        &self,
        connection_value: &ProviderConnection,
        catalog_template: &ProviderTemplate,
        expected_catalog_state_version: u64,
    ) -> CoreResult<()> {
        self.persist_provider_connection_for_catalog_state(
            connection_value,
            catalog_template,
            expected_catalog_state_version,
            false,
        )
    }

    /// Inserts a newly reviewed signed-catalog connection while atomically
    /// rejecting both a stale catalog review and an occupied connection ID.
    pub fn insert_provider_connection_for_catalog_state(
        &self,
        connection_value: &ProviderConnection,
        catalog_template: &ProviderTemplate,
        expected_catalog_state_version: u64,
    ) -> CoreResult<()> {
        self.persist_provider_connection_for_catalog_state(
            connection_value,
            catalog_template,
            expected_catalog_state_version,
            true,
        )
    }

    fn persist_provider_connection_for_catalog_state(
        &self,
        connection_value: &ProviderConnection,
        catalog_template: &ProviderTemplate,
        expected_catalog_state_version: u64,
        insert_only: bool,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current_state_version = transaction
            .query_row(
                "SELECT state_version
                 FROM provider_catalog_state
                 WHERE singleton = 1",
                [],
                |row| row.get::<_, u64>(0),
            )
            .map_err(storage_db_error)?;
        if current_state_version != expected_catalog_state_version {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "provider catalog changed; review the connection again",
                true,
            ));
        }
        if connection_value.template_id != catalog_template.id
            || connection_value.template_version != catalog_template.manifest_version
            || catalog_template.source != TemplateSource::SignedCatalog
        {
            return Err(CoreError::invalid(
                "catalog connection does not match its signed provider template",
            ));
        }
        save_provider_template_row(&transaction, catalog_template)?;
        validate_provider_connection(&transaction, connection_value)?;
        if insert_only {
            ensure_provider_connection_id_vacant(&transaction, &connection_value.id)?;
        }
        upsert_provider_connection_row(&transaction, connection_value)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn delete_provider_connection(&self, id: &ProviderConnectionId) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let credential_bound = transaction
            .query_row(
                "SELECT credential_ref IS NOT NULL
                 FROM provider_connections
                 WHERE id = ?1 AND archived_at IS NULL",
                [id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("provider connection"))?;
        if credential_bound {
            return Err(CoreError::invalid(
                "credential-bound provider connections require durable native removal before archive",
            ));
        }
        archive_provider_connection_row(&transaction, id.as_str(), Utc::now())?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }
}
type ProviderConnectionRow = (
    String,
    String,
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    String,
    String,
    String,
);

pub(crate) fn upsert_provider_connection_row(
    transaction: &rusqlite::Transaction<'_>,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    validate_provider_connection(transaction, connection)?;
    if connection.updated_at < connection.created_at {
        return Err(CoreError::invalid(
            "provider connection updated_at must not precede created_at",
        ));
    }
    let archived_connection_exists = row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1 FROM provider_connections
           WHERE id = ?1 AND archived_at IS NOT NULL
         )",
        connection.id.as_str(),
    )?;
    if archived_connection_exists {
        return Err(CoreError::invalid(
            "an archived provider connection identifier cannot be reused",
        ));
    }
    let existing_connection = transaction
        .query_row(
            "SELECT id, template_id, template_version, display_name, api_origin,
                    config_json, credential_ref, credential_scope_json, timeout_seconds,
                    status, created_at, updated_at
             FROM provider_connections
             WHERE id = ?1 AND archived_at IS NULL",
            [connection.id.as_str()],
            provider_connection_columns,
        )
        .optional()
        .map_err(storage_db_error)?
        .map(decode_provider_connection_row)
        .transpose()?;
    if existing_connection.as_ref().is_some_and(|existing| {
        existing.template_id != connection.template_id
            || existing.template_version != connection.template_version
    }) {
        return Err(CoreError::invalid(
            "an existing provider connection cannot change its template identity",
        ));
    }
    if let Some(existing) = existing_connection.as_ref()
        && (existing.api_origin != connection.api_origin
            || existing.config != connection.config
            || existing.credential_ref != connection.credential_ref
            || existing.credential_scope != connection.credential_scope)
    {
        return Err(CoreError::invalid(
            "an existing provider connection cannot change its endpoint configuration, \
             network approval, or credential binding; create a new connection instead",
        ));
    }
    let config_json = serde_json::to_string(&connection.config).map_err(|error| {
        CoreError::internal(format!("cannot encode provider connection config: {error}"))
    })?;
    let credential_scope_json = connection
        .credential_scope
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| CoreError::internal(format!("cannot encode credential scope: {error}")))?;
    transaction
        .execute(
            "INSERT INTO provider_connections
             (id, template_id, template_version, display_name, api_origin, config_json,
              credential_ref, credential_scope_json, timeout_seconds, status,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               api_origin = excluded.api_origin,
               config_json = excluded.config_json,
               credential_ref = excluded.credential_ref,
               credential_scope_json = excluded.credential_scope_json,
               timeout_seconds = excluded.timeout_seconds,
               status = excluded.status,
               updated_at = excluded.updated_at",
            params![
                connection.id.as_str(),
                connection.template_id.as_str(),
                connection.template_version,
                connection.display_name,
                connection.api_origin.as_str(),
                config_json,
                connection
                    .credential_ref
                    .as_ref()
                    .map(CredentialRef::as_str),
                credential_scope_json,
                connection.timeout_seconds,
                connection_status_to_str(connection.status),
                connection.created_at.to_rfc3339(),
                connection.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    sync_provider_local_network_approval_row(transaction, connection)?;
    Ok(())
}

fn ensure_provider_connection_id_vacant(
    transaction: &rusqlite::Transaction<'_>,
    id: &ProviderConnectionId,
) -> CoreResult<()> {
    if row_exists(
        transaction,
        "SELECT EXISTS(SELECT 1 FROM provider_connections WHERE id = ?1)",
        id.as_str(),
    )? {
        return Err(CoreError::invalid(
            "provider connection identifier already exists; choose a new identifier",
        ));
    }
    Ok(())
}

fn sync_provider_local_network_approval_row(
    transaction: &rusqlite::Transaction<'_>,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    let existing = transaction
        .query_row(
            "SELECT origin, addresses_json
             FROM provider_connection_local_network_approvals
             WHERE connection_id = ?1",
            [connection.id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?;
    match (connection.config.local_network_approval.as_ref(), existing) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(storage_corrupted(
            "stored local-network approval has no matching typed connection grant",
        )),
        (Some(approval), Some((origin, addresses_json))) => {
            let addresses =
                serde_json::from_str::<Vec<IpAddr>>(&addresses_json).map_err(|error| {
                    storage_corrupted(format!(
                        "stored local-network approval addresses are invalid: {error}"
                    ))
                })?;
            if origin != approval.origin.as_str() || addresses != approval.addresses {
                return Err(storage_corrupted(
                    "stored local-network approval does not match its typed connection grant",
                ));
            }
            Ok(())
        }
        (Some(approval), None) => {
            let addresses_json = serde_json::to_string(&approval.addresses).map_err(|error| {
                CoreError::internal(format!(
                    "cannot encode provider local-network approval: {error}"
                ))
            })?;
            transaction
                .execute(
                    "INSERT INTO provider_connection_local_network_approvals
                     (connection_id, origin, addresses_json, approved_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        connection.id.as_str(),
                        approval.origin.as_str(),
                        addresses_json,
                        connection.created_at.to_rfc3339(),
                    ],
                )
                .map_err(storage_db_error)?;
            Ok(())
        }
    }
}

pub(crate) fn archive_provider_connection_row(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
    archived_at: DateTime<Utc>,
) -> CoreResult<()> {
    let active = row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1 FROM provider_connections
           WHERE id = ?1 AND archived_at IS NULL
         )",
        connection_id,
    )?;
    if !active {
        return Err(not_found("provider connection"));
    }
    ensure_provider_connection_has_no_unfinished_work(transaction, connection_id)?;
    clear_provider_selections_for_connection(transaction, connection_id)?;
    let changed = transaction
        .execute(
            "UPDATE provider_connections
             SET archived_at = ?2
             WHERE id = ?1 AND archived_at IS NULL",
            params![connection_id, archived_at.to_rfc3339()],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "provider connection changed while it was being archived",
        ));
    }
    Ok(())
}

pub(crate) fn ensure_provider_connection_has_no_unfinished_work(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<()> {
    ensure_provider_connection_has_no_unfinished_generation_attempt(transaction, connection_id)?;
    let unfinished_generation = row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1
           FROM generations AS generation
           JOIN provider_models AS route ON route.id = generation.model_route_id
           WHERE route.connection_id = ?1
             AND generation.status = 'running'
         )",
        connection_id,
    )?;
    if unfinished_generation {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider connection cannot be archived while generation is unfinished",
            true,
        ));
    }
    let unfinished_model_sync = row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1
           FROM model_sync_jobs
           WHERE connection_id = ?1
             AND state NOT IN ('completed', 'failed', 'cancelled')
         )",
        connection_id,
    )?;
    if unfinished_model_sync {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider connection cannot be archived while model synchronization is unfinished",
            true,
        ));
    }

    let unfinished_discovery = row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1
           FROM provider_discovery_sessions
           WHERE (
               json_extract(sanitized_input_json, '$.connection_id') = ?1
               OR committed_connection_id = ?1
             )
             AND state NOT IN ('ready', 'failed', 'cancelled')
         )",
        connection_id,
    )?;
    if unfinished_discovery {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider connection cannot be archived while provider discovery is unfinished",
            true,
        ));
    }
    Ok(())
}

fn ensure_provider_connection_has_no_unfinished_generation_attempt(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<()> {
    let unfinished_generation_attempt = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1
               FROM generation_attempt_intents AS attempt
               LEFT JOIN provider_models AS route
                 ON route.id = json_extract(
                   attempt.prompt_selection_authority_json,
                   '$.provider_target_authority.target.model_route_id'
                 )
               WHERE attempt.status NOT IN ('failed_before_dispatch', 'completed')
                 AND (
                   (
                     json_extract(
                       attempt.prompt_selection_authority_json,
                       '$.provider_target_authority.kind'
                     ) = 'provider_profile'
                     AND json_extract(
                       attempt.prompt_selection_authority_json,
                       '$.provider_target_authority.provider_profile_id'
                     ) = ?1
                   )
                   OR (
                     json_extract(
                       attempt.prompt_selection_authority_json,
                       '$.provider_target_authority.kind'
                     ) = 'generation_target'
                     AND route.connection_id = ?1
                   )
                 )
             )",
            [connection_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if unfinished_generation_attempt {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider connection cannot be archived while a generation attempt is unfinished",
            true,
        ));
    }
    Ok(())
}

pub(super) fn ensure_generation_provider_credential_settled(
    transaction: &rusqlite::Transaction<'_>,
    generation: &GenerationRecord,
    credential_authority: Option<&crate::ProviderCredentialAccessAuthority>,
    require_exact_credential_authority: bool,
) -> CoreResult<()> {
    let Some(model_route_id) = generation.model_route_id.as_ref() else {
        return Ok(());
    };
    if require_exact_credential_authority {
        let connection_id = transaction
            .query_row(
                "SELECT connection_id FROM provider_models WHERE id = ?1",
                [model_route_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| CoreError::invalid("generation provider model route is unavailable"))?;
        return crate::generation_attempt::validate_provider_credential_access_authority_in_transaction(
            transaction,
            &ProviderConnectionId::from(connection_id),
            credential_authority,
        );
    }
    let unresolved = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1
               FROM provider_models AS route
               JOIN provider_credential_operations AS operation
                 ON operation.connection_id = route.connection_id
               WHERE route.id = ?1
                 AND operation.status IN (
                   'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                 )
             )",
            [model_route_id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if unresolved {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "generation cannot start while provider credential recovery is unresolved",
            true,
        ));
    }
    Ok(())
}

pub(crate) fn provider_connection_columns(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ProviderConnectionRow> {
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
        row.get(10)?,
        row.get(11)?,
    ))
}

pub(crate) fn decode_provider_connection_row(
    row: ProviderConnectionRow,
) -> CoreResult<ProviderConnection> {
    let (
        id,
        template_id,
        template_version,
        display_name,
        api_origin,
        config_json,
        credential_ref,
        credential_scope_json,
        timeout_seconds,
        status,
        created_at,
        updated_at,
    ) = row;
    let template_version = u32::try_from(template_version)
        .map_err(|_| storage_corrupted("stored provider template version is invalid"))?;
    let timeout_seconds = u32::try_from(timeout_seconds)
        .map_err(|_| storage_corrupted("stored provider timeout is invalid"))?;
    if !(1..=600).contains(&timeout_seconds) {
        return Err(storage_corrupted(
            "stored provider timeout is outside the supported range",
        ));
    }
    let api_origin = CanonicalOrigin::parse(&api_origin).map_err(|error| {
        storage_corrupted(format!("stored provider API origin is invalid: {error}"))
    })?;
    let config = serde_json::from_str::<ConnectionConfig>(&config_json).map_err(|error| {
        storage_corrupted(format!(
            "stored provider connection config is invalid: {error}"
        ))
    })?;
    validate_connection_config(&config).map_err(stored_catalog_error)?;
    validate_provider_network_contract(&api_origin, &config).map_err(stored_catalog_error)?;
    let credential_scope = credential_scope_json
        .map(|json| {
            serde_json::from_str::<CredentialScope>(&json).map_err(|error| {
                storage_corrupted(format!("stored credential scope is invalid: {error}"))
            })
        })
        .transpose()?;
    if credential_ref.is_some() != credential_scope.is_some() {
        return Err(storage_corrupted(
            "stored credential reference and scope are inconsistent",
        ));
    }
    if let Some(scope) = credential_scope.as_ref()
        && (scope.allowed_origins.is_empty()
            || !scope
                .allowed_origins
                .iter()
                .any(|origin| origin == &api_origin))
    {
        return Err(storage_corrupted(
            "stored credential scope does not include the provider API origin",
        ));
    }
    let created_at = parse_stored_datetime(&created_at, "provider connection created_at")?;
    let updated_at = parse_stored_datetime(&updated_at, "provider connection updated_at")?;
    if updated_at < created_at {
        return Err(storage_corrupted(
            "stored provider connection timestamps are inconsistent",
        ));
    }
    Ok(ProviderConnection {
        id: ProviderConnectionId::from(id),
        template_id: ProviderTemplateId::from(template_id),
        template_version,
        display_name,
        api_origin,
        config,
        credential_ref: credential_ref.map(CredentialRef),
        credential_scope,
        timeout_seconds,
        status: str_to_connection_status(&status)?,
        created_at,
        updated_at,
    })
}

pub(super) const fn connection_status_to_str(status: ConnectionStatus) -> &'static str {
    match status {
        ConnectionStatus::Untested => "untested",
        ConnectionStatus::Connected => "connected",
        ConnectionStatus::AuthFailed => "auth_failed",
        ConnectionStatus::Unavailable => "unavailable",
    }
}

fn str_to_connection_status(value: &str) -> CoreResult<ConnectionStatus> {
    match value {
        "untested" => Ok(ConnectionStatus::Untested),
        "connected" => Ok(ConnectionStatus::Connected),
        "auth_failed" => Ok(ConnectionStatus::AuthFailed),
        "unavailable" => Ok(ConnectionStatus::Unavailable),
        _ => Err(storage_corrupted(format!(
            "stored provider connection status is invalid: {value}"
        ))),
    }
}

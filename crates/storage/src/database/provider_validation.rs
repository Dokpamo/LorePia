use super::{
    AuthBinding, CanonicalOrigin, Connection, ConnectionConfig, ConnectionConfigValue,
    ConnectionFieldType, CoreError, CoreErrorCode, CoreResult, IpAddr, Ipv4Addr, Ipv6Addr,
    LEGACY_BASE_URL_CONFIG_KEY, LEGACY_PROVIDER_TEMPLATE_ID, LEGACY_PROVIDER_TEMPLATE_VERSION,
    ModelRouteConfig, OptionalExtension, ProviderConnection, ProviderLocalNetworkApproval,
    ProviderNetworkMode, ProviderTemplate, Url, canonical_origin_for_legacy_base_url,
    is_loopback_host, legacy_api_base_path, legacy_network_mode, not_found, params,
    storage_corrupted, storage_db_error, stored_catalog_error,
};

pub(super) fn validate_nonempty(label: &str, value: &str) -> CoreResult<()> {
    if value.trim().is_empty() {
        Err(CoreError::invalid(format!("{label} must not be empty")))
    } else {
        Ok(())
    }
}

pub(super) fn validate_connection_config(config: &ConnectionConfig) -> CoreResult<()> {
    let mut keys = std::collections::BTreeSet::new();
    for entry in &config.values {
        validate_nonempty("connection configuration key", &entry.key)?;
        if !keys.insert(entry.key.as_str()) {
            return Err(CoreError::invalid(
                "connection configuration keys must be unique",
            ));
        }
        if is_sensitive_configuration_key(&entry.key) {
            return Err(CoreError::invalid(
                "credentials must be referenced by credential_ref and never stored in configuration",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_provider_network_contract(
    api_origin: &CanonicalOrigin,
    config: &ConnectionConfig,
) -> CoreResult<()> {
    let origin = Url::parse(api_origin.as_str())
        .map_err(|error| CoreError::invalid(format!("provider API origin is invalid: {error}")))?;
    let host = origin
        .host_str()
        .ok_or_else(|| CoreError::invalid("provider API origin requires a host"))?;
    let loopback = is_loopback_host(host);
    match (config.network_mode, config.local_network_approval.as_ref()) {
        (ProviderNetworkMode::Public, None) => {
            if origin.scheme() != "https" {
                return Err(CoreError::invalid(
                    "public provider connections require an https API origin",
                ));
            }
            if loopback {
                return Err(CoreError::invalid(
                    "public provider connections cannot use a loopback API origin",
                ));
            }
            if host
                .trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .is_ok_and(is_rfc1918_or_ula)
            {
                return Err(CoreError::invalid(
                    "private IP origins require approved local-network mode",
                ));
            }
        }
        (ProviderNetworkMode::LocalLoopback, None) => {
            if !loopback {
                return Err(CoreError::invalid(
                    "loopback provider mode requires a loopback API origin",
                ));
            }
        }
        (ProviderNetworkMode::ApprovedLocalNetwork, Some(approval)) => {
            validate_provider_local_network_approval(api_origin, approval)?;
        }
        (ProviderNetworkMode::ApprovedLocalNetwork, None) => {
            return Err(CoreError::invalid(
                "approved local-network mode requires an exact origin and address approval",
            ));
        }
        (ProviderNetworkMode::Public | ProviderNetworkMode::LocalLoopback, Some(_)) => {
            return Err(CoreError::invalid(
                "local-network approval is only valid in approved local-network mode",
            ));
        }
    }
    Ok(())
}

fn validate_provider_local_network_approval(
    api_origin: &CanonicalOrigin,
    approval: &ProviderLocalNetworkApproval,
) -> CoreResult<()> {
    if &approval.origin != api_origin {
        return Err(CoreError::invalid(
            "local-network approval origin must exactly match the provider API origin",
        ));
    }
    if approval.addresses.is_empty() || approval.addresses.len() > 16 {
        return Err(CoreError::invalid(
            "local-network approval requires from 1 to 16 exact IP addresses",
        ));
    }
    let mut normalized = approval
        .addresses
        .iter()
        .copied()
        .map(normalize_approved_ip)
        .collect::<Vec<_>>();
    if normalized
        .iter()
        .any(|address| !is_rfc1918_or_ula(*address))
    {
        return Err(CoreError::invalid(
            "local-network approval accepts only RFC1918 IPv4 or ULA IPv6 addresses",
        ));
    }
    normalized.sort_unstable();
    normalized.dedup();
    if normalized != approval.addresses {
        return Err(CoreError::invalid(
            "local-network approval addresses must be normalized, sorted, and unique",
        ));
    }
    let origin = Url::parse(api_origin.as_str())
        .map_err(|error| CoreError::invalid(format!("provider API origin is invalid: {error}")))?;
    if let Some(literal) = origin
        .host_str()
        .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
        .map(normalize_approved_ip)
        && approval.addresses.as_slice() != [literal]
    {
        return Err(CoreError::invalid(
            "an IP-literal local-network origin must approve only that exact address",
        ));
    }
    Ok(())
}

pub(super) fn validate_provider_local_network_approval_integrity(
    connection: &Connection,
) -> CoreResult<()> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT id, api_origin, config_json
                 FROM provider_connections
                 ORDER BY id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    for (id, api_origin, config_json) in rows {
        let api_origin = CanonicalOrigin::parse(&api_origin).map_err(|error| {
            storage_corrupted(format!("stored provider API origin is invalid: {error}"))
        })?;
        let config = serde_json::from_str::<ConnectionConfig>(&config_json).map_err(|error| {
            storage_corrupted(format!(
                "stored provider connection config is invalid: {error}"
            ))
        })?;
        validate_provider_network_contract(&api_origin, &config).map_err(stored_catalog_error)?;
        let mirror = connection
            .query_row(
                "SELECT origin, addresses_json
                 FROM provider_connection_local_network_approvals
                 WHERE connection_id = ?1",
                [&id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(storage_db_error)?;
        match (config.local_network_approval.as_ref(), mirror) {
            (None, None) => {}
            (None, Some(_)) | (Some(_), None) => {
                return Err(storage_corrupted(
                    "stored provider local-network approval mirror is incomplete",
                ));
            }
            (Some(approval), Some((origin, addresses_json))) => {
                let addresses =
                    serde_json::from_str::<Vec<IpAddr>>(&addresses_json).map_err(|error| {
                        storage_corrupted(format!(
                            "stored local-network approval addresses are invalid: {error}"
                        ))
                    })?;
                if origin != approval.origin.as_str() || addresses != approval.addresses {
                    return Err(storage_corrupted(
                        "stored provider local-network approval mirror does not match config",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn normalize_approved_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(address), IpAddr::V4),
        IpAddr::V4(address) => IpAddr::V4(address),
    }
}

const fn is_rfc1918_or_ula(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_rfc1918(address),
        IpAddr::V6(address) => is_ula(address),
    }
}

const fn is_rfc1918(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 10
        || (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31)
        || (octets[0] == 192 && octets[1] == 168)
}

const fn is_ula(address: Ipv6Addr) -> bool {
    address.octets()[0] & 0xfe == 0xfc
}

pub(super) fn validate_route_config(config: &ModelRouteConfig) -> CoreResult<()> {
    let mut keys = std::collections::BTreeSet::new();
    for entry in &config.values {
        validate_nonempty("model route configuration key", &entry.key)?;
        if !keys.insert(entry.key.as_str()) {
            return Err(CoreError::invalid(
                "model route configuration keys must be unique",
            ));
        }
        if is_sensitive_configuration_key(&entry.key) {
            return Err(CoreError::invalid(
                "credentials must never be stored in model route configuration",
            ));
        }
    }
    Ok(())
}

pub(super) fn is_sensitive_configuration_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized == "authorization"
        || normalized == "apikey"
        || normalized.ends_with("apikey")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.ends_with("token")
        || normalized.contains("credential")
        || normalized == "cookie"
}

pub(super) fn validate_provider_connection(
    transaction: &rusqlite::Transaction<'_>,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    validate_nonempty("provider connection id", connection.id.as_str())?;
    validate_nonempty("provider connection display name", &connection.display_name)?;
    if connection.template_version == 0 {
        return Err(CoreError::invalid(
            "provider connection template version must be positive",
        ));
    }
    if !(1..=600).contains(&connection.timeout_seconds) {
        return Err(CoreError::invalid(
            "provider timeout must be from 1 to 600 seconds",
        ));
    }
    validate_provider_network_contract(&connection.api_origin, &connection.config)?;
    let template_json = transaction
        .query_row(
            "SELECT manifest_json FROM provider_templates
             WHERE id = ?1 AND version = ?2",
            params![connection.template_id.as_str(), connection.template_version],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("provider template"))?;
    let template = serde_json::from_str::<ProviderTemplate>(&template_json).map_err(|error| {
        storage_corrupted(format!("stored provider template is invalid: {error}"))
    })?;
    validate_connection_config(&connection.config)?;
    match (&connection.credential_ref, &connection.credential_scope) {
        (None, None) => {}
        (Some(reference), Some(scope)) => {
            validate_nonempty("credential reference", reference.as_str())?;
            if scope.allowed_origins.is_empty() {
                return Err(CoreError::invalid(
                    "credential scope requires at least one allowed origin",
                ));
            }
            if !scope
                .allowed_origins
                .iter()
                .any(|origin| origin == &connection.api_origin)
            {
                return Err(CoreError::invalid(
                    "credential scope must include the provider API origin",
                ));
            }
            let unique_origins = scope
                .allowed_origins
                .iter()
                .map(CanonicalOrigin::as_str)
                .collect::<std::collections::BTreeSet<_>>();
            if unique_origins.len() != scope.allowed_origins.len() {
                return Err(CoreError::invalid(
                    "credential scope origins must be unique",
                ));
            }
        }
        _ => {
            return Err(CoreError::invalid(
                "credential_ref and credential_scope must be set or cleared together",
            ));
        }
    }
    validate_connection_against_template(connection, &template)?;
    if connection.template_id.as_str() == LEGACY_PROVIDER_TEMPLATE_ID
        && connection.template_version == LEGACY_PROVIDER_TEMPLATE_VERSION
    {
        validate_legacy_provider_connection(transaction, connection)?;
    }
    Ok(())
}

fn validate_connection_against_template(
    connection: &ProviderConnection,
    template: &ProviderTemplate,
) -> CoreResult<()> {
    if connection.config.network_mode == ProviderNetworkMode::ApprovedLocalNetwork
        && !matches!(template.default_manifest.auth, AuthBinding::None)
        && Url::parse(connection.api_origin.as_str()).is_ok_and(|origin| origin.scheme() != "https")
    {
        return Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "credential-bearing local-network providers require an https API origin",
            false,
        ));
    }
    for entry in &connection.config.values {
        let field = template
            .connection_fields
            .iter()
            .find(|field| field.key == entry.key)
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "provider connection contains undeclared field {}",
                    entry.key
                ))
            })?;
        let type_matches = matches!(
            (field.value_type, &entry.value),
            (ConnectionFieldType::Text, ConnectionConfigValue::Text(_))
                | (
                    ConnectionFieldType::Integer,
                    ConnectionConfigValue::Integer(_)
                )
                | (
                    ConnectionFieldType::Boolean,
                    ConnectionConfigValue::Boolean(_)
                )
        );
        if !type_matches {
            return Err(CoreError::invalid(format!(
                "provider connection field {} has the wrong value type",
                entry.key
            )));
        }
    }
    for field in &template.connection_fields {
        let present = if field.value_type == ConnectionFieldType::Credential {
            connection.credential_ref.is_some()
        } else {
            connection
                .config
                .values
                .iter()
                .any(|entry| entry.key == field.key)
        };
        if field.required && !present {
            return Err(CoreError::invalid(format!(
                "provider connection is missing required field {}",
                field.key
            )));
        }
    }
    if let Some(scope) = connection.credential_scope.as_ref()
        && scope.auth_binding != template.default_manifest.auth
    {
        return Err(CoreError::invalid(
            "credential scope authentication does not match the provider template",
        ));
    }
    if matches!(template.default_manifest.auth, AuthBinding::None)
        && connection.credential_ref.is_some()
    {
        return Err(CoreError::invalid(
            "a no-auth provider template cannot persist a credential reference",
        ));
    }
    Ok(())
}

fn validate_legacy_provider_connection(
    transaction: &rusqlite::Transaction<'_>,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    let base_urls = connection
        .config
        .values
        .iter()
        .filter(|entry| entry.key == LEGACY_BASE_URL_CONFIG_KEY)
        .collect::<Vec<_>>();
    if base_urls.len() != 1 {
        return Err(CoreError::invalid(
            "legacy provider connection requires exactly one api_base_url value",
        ));
    }
    let ConnectionConfigValue::Text(base_url) = &base_urls[0].value else {
        return Err(CoreError::invalid(
            "legacy provider api_base_url must be text",
        ));
    };
    let origin = canonical_origin_for_legacy_base_url(base_url)?;
    if origin != connection.api_origin {
        return Err(CoreError::invalid(
            "legacy provider api_base_url origin does not match api_origin",
        ));
    }
    if legacy_api_base_path(base_url)? != connection.config.api_base_path {
        return Err(CoreError::invalid(
            "legacy provider api_base_url path does not match api_base_path",
        ));
    }
    if legacy_network_mode(base_url)? != connection.config.network_mode {
        return Err(CoreError::invalid(
            "legacy provider api_base_url does not match its network mode",
        ));
    }
    if connection
        .credential_ref
        .as_ref()
        .is_some_and(|reference| reference.as_str() != connection.id.as_str())
    {
        return Err(CoreError::invalid(
            "legacy provider credential_ref, when set, must equal the connection id",
        ));
    }
    if let Some((display_name, legacy_base_url, timeout_seconds)) = transaction
        .query_row(
            "SELECT display_name, base_url, timeout_seconds
             FROM provider_profiles WHERE id = ?1",
            [connection.id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        && (display_name != connection.display_name
            || legacy_base_url != *base_url
            || timeout_seconds != connection.timeout_seconds)
    {
        return Err(CoreError::invalid(
            "legacy provider connection fields must match its provider profile",
        ));
    }
    Ok(())
}

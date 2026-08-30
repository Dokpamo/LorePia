use lorepia_domain::{CanonicalOrigin, CoreResult, HeaderName};
use rusqlite::{Connection, OptionalExtension, functions::FunctionFlags};
use sha2::{Digest, Sha256};

use super::{storage_corrupted, storage_db_error};

pub(super) fn configure_connection(connection: &Connection) -> CoreResult<()> {
    register_integrity_functions(connection)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(storage_db_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(storage_db_error)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(storage_db_error)?;
    Ok(())
}

pub(crate) fn register_integrity_functions(connection: &Connection) -> CoreResult<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    connection
        .create_scalar_function("lorepia_sha256_hex", 1, flags, |context| {
            let value = context.get::<String>(0)?;
            Ok(format!("{:x}", Sha256::digest(value.as_bytes())))
        })
        .map_err(storage_db_error)?;
    connection
        .create_scalar_function(
            "lorepia_discovery_commit_plan_sha256",
            1,
            flags,
            |context| {
                let value = context.get::<String>(0)?;
                Ok(crate::discovery_repository::contract_codec::canonical_discovery_commit_plan_sha256(
                    &value,
                ))
            },
        )
        .map_err(storage_db_error)?;
    connection
        .create_scalar_function("lorepia_canonical_origin", 1, flags, |context| {
            let value = context.get::<String>(0)?;
            Ok(CanonicalOrigin::parse(&value)
                .ok()
                .map(|origin| origin.to_string()))
        })
        .map_err(storage_db_error)?;
    connection
        .create_scalar_function("lorepia_header_name", 1, flags, |context| {
            let value = context.get::<String>(0)?;
            Ok(HeaderName::parse(&value)
                .ok()
                .map(|name| name.as_str().to_owned()))
        })
        .map_err(storage_db_error)?;
    connection
        .create_scalar_function(
            "lorepia_native_no_effect_evidence_sha256",
            8,
            flags,
            |context| {
                let evidence = serde_json::json!({
                    "schema_version": context.get::<i64>(0)?,
                    "attestation_kind": context.get::<String>(1)?,
                    "recovery_owner": context.get::<String>(2)?,
                    "operation_id": context.get::<String>(3)?,
                    "session_id": context.get::<String>(4)?,
                    "commit_attempt_id": context.get::<String>(5)?,
                    "commit_plan_sha256": context.get::<String>(6)?,
                    "connection_id": context.get::<String>(7)?,
                });
                Ok(format!(
                    "{:x}",
                    Sha256::digest(evidence.to_string().as_bytes())
                ))
            },
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(crate) fn validate_database_integrity(connection: &Connection) -> CoreResult<()> {
    let foreign_key_violation = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()
        .map_err(storage_db_error)?
        .is_some();
    if foreign_key_violation {
        return Err(storage_corrupted(
            "database cutover foreign-key validation failed",
        ));
    }
    let integrity = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .map_err(storage_db_error)?;
    if integrity != "ok" {
        return Err(storage_corrupted(format!(
            "database cutover integrity validation failed: {integrity}"
        )));
    }
    Ok(())
}

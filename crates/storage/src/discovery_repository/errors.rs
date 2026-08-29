//! Error translation for the typed discovery repository boundary.

use lorepia_domain::{CoreError, CoreErrorCode, discovery::DiscoveryContractError};

use crate::discovery::DiscoveryStorageError;

pub(super) fn contract_error(error: DiscoveryContractError) -> CoreError {
    CoreError::invalid(format!("invalid provider discovery contract: {error}"))
}

pub(super) fn database_error(error: rusqlite::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("SQLite discovery operation failed: {error}"),
        true,
    )
}

pub(super) fn discovery_error(error: DiscoveryStorageError) -> CoreError {
    match error {
        DiscoveryStorageError::Database(error) => database_error(error),
        DiscoveryStorageError::SessionNotFound(_) => CoreError::new(
            CoreErrorCode::NotFound,
            "provider discovery session was not found",
            false,
        ),
        DiscoveryStorageError::RevisionConflict { expected, actual } => CoreError::invalid(
            format!("discovery revision conflict: expected {expected}, current {actual}"),
        ),
        DiscoveryStorageError::IdempotencyConflict { .. } => {
            CoreError::invalid("discovery action identifier was reused with a different request")
        }
        DiscoveryStorageError::InvalidTransition(reason) => {
            CoreError::invalid(format!("invalid durable discovery transition: {reason}"))
        }
    }
}

pub(super) fn corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

use std::fmt;

use lorepia_core::{CoreError, CoreErrorCode};
use serde::{Deserialize, Serialize};

pub type ShellResult<T> = Result<T, ShellError>;

/// Stable, localizable error categories exposed across the shell boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellErrorCode {
    InvalidInput,
    UnsupportedContent,
    UnsafeArchive,
    NotFound,
    PermissionDenied,
    StorageUnavailable,
    StorageCorrupted,
    ProviderAuthFailed,
    ProviderRateLimited,
    ProviderUnavailable,
    NetworkUnavailable,
    Cancelled,
    Internal,
}

impl ShellErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::UnsupportedContent => "unsupported_content",
            Self::UnsafeArchive => "unsafe_archive",
            Self::NotFound => "not_found",
            Self::PermissionDenied => "permission_denied",
            Self::StorageUnavailable => "storage_unavailable",
            Self::StorageCorrupted => "storage_corrupted",
            Self::ProviderAuthFailed => "provider_auth_failed",
            Self::ProviderRateLimited => "provider_rate_limited",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::NetworkUnavailable => "network_unavailable",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }

    const fn message_key(self) -> &'static str {
        match self {
            Self::InvalidInput => "error.invalid_input",
            Self::UnsupportedContent => "error.unsupported_content",
            Self::UnsafeArchive => "error.unsafe_archive",
            Self::NotFound => "error.not_found",
            Self::PermissionDenied => "error.permission_denied",
            Self::StorageUnavailable => "error.storage_unavailable",
            Self::StorageCorrupted => "error.storage_corrupted",
            Self::ProviderAuthFailed => "error.provider_auth_failed",
            Self::ProviderRateLimited => "error.provider_rate_limited",
            Self::ProviderUnavailable => "error.provider_unavailable",
            Self::NetworkUnavailable => "error.network_unavailable",
            Self::Cancelled => "error.cancelled",
            Self::Internal => "error.internal",
        }
    }
}

impl From<CoreErrorCode> for ShellErrorCode {
    fn from(value: CoreErrorCode) -> Self {
        match value {
            CoreErrorCode::InvalidInput => Self::InvalidInput,
            CoreErrorCode::UnsupportedContent => Self::UnsupportedContent,
            CoreErrorCode::UnsafeArchive => Self::UnsafeArchive,
            CoreErrorCode::NotFound => Self::NotFound,
            CoreErrorCode::PermissionDenied => Self::PermissionDenied,
            CoreErrorCode::StorageUnavailable => Self::StorageUnavailable,
            CoreErrorCode::StorageCorrupted => Self::StorageCorrupted,
            CoreErrorCode::ProviderAuthFailed => Self::ProviderAuthFailed,
            CoreErrorCode::ProviderRateLimited => Self::ProviderRateLimited,
            CoreErrorCode::ProviderUnavailable => Self::ProviderUnavailable,
            CoreErrorCode::NetworkUnavailable => Self::NetworkUnavailable,
            CoreErrorCode::Cancelled => Self::Cancelled,
            CoreErrorCode::Internal => Self::Internal,
        }
    }
}

/// Secret-free error envelope returned to the frontend.
///
/// Core's free-form diagnostic message stays on the Rust side. It can include
/// filesystem or provider context that is useful for local diagnostics but is
/// not an appropriate IPC contract. The operation identifier remains available
/// for correlating a user-visible error with redacted native logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellError {
    pub code: ShellErrorCode,
    pub message_key: String,
    pub recoverable: bool,
    pub operation_id: String,
}

impl From<CoreError> for ShellError {
    fn from(value: CoreError) -> Self {
        let code = ShellErrorCode::from(value.code);
        Self {
            code,
            message_key: code.message_key().to_owned(),
            recoverable: value.recoverable,
            operation_id: value.operation_id,
        }
    }
}

impl fmt::Display for ShellError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.message_key, self.operation_id)
    }
}

impl std::error::Error for ShellError {}

#[cfg(test)]
mod tests {
    use lorepia_core::{CoreError, CoreErrorCode};

    use super::{ShellError, ShellErrorCode};

    #[test]
    fn core_diagnostic_message_is_not_exposed_by_error_envelope() {
        let secret = "sk-shell-error-canary";
        let raw_path = "/Users/synthetic/private/lorepia.sqlite3";
        let core_error = CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            format!("{secret} at {raw_path}"),
            true,
        );

        let shell_error = ShellError::from(core_error);
        let json = serde_json::to_string(&shell_error).expect("serialize shell error");
        let debug = format!("{shell_error:?}");

        assert_eq!(shell_error.code, ShellErrorCode::ProviderUnavailable);
        assert!(shell_error.recoverable);
        assert!(!json.contains(secret));
        assert!(!json.contains(raw_path));
        assert!(!debug.contains(secret));
        assert!(!debug.contains(raw_path));
    }
}

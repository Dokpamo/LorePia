use lorepia_shell_api::ShellError;
use serde::Serialize;
use tauri_plugin_lorepia_platform::{PlatformError, PlatformErrorCode};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandError {
    pub code: String,
    pub message_key: String,
    pub recoverable: bool,
    pub operation_id: String,
}

impl CommandError {
    pub fn internal() -> Self {
        Self {
            code: "internal".to_owned(),
            message_key: "error.internal".to_owned(),
            recoverable: false,
            operation_id: operation_id(),
        }
    }

    pub fn invalid_input() -> Self {
        Self {
            code: "invalid_input".to_owned(),
            message_key: "error.invalid_input".to_owned(),
            recoverable: true,
            operation_id: operation_id(),
        }
    }

    pub fn core_unavailable() -> Self {
        Self {
            code: "core_unavailable".to_owned(),
            message_key: "error.core_unavailable".to_owned(),
            recoverable: true,
            operation_id: operation_id(),
        }
    }

    pub fn busy() -> Self {
        Self {
            code: "busy".to_owned(),
            message_key: "error.busy".to_owned(),
            recoverable: true,
            operation_id: operation_id(),
        }
    }

    pub fn assistant_pricing_unavailable() -> Self {
        Self {
            code: "assistant_pricing_unavailable".to_owned(),
            message_key: "provider.discovery.assistant_pricing_unavailable".to_owned(),
            recoverable: false,
            operation_id: operation_id(),
        }
    }

    pub fn generation_reattachment_unavailable() -> Self {
        Self {
            code: "generation_reattachment_unavailable".to_owned(),
            message_key: "chat.generation_reattachment_unavailable".to_owned(),
            recoverable: false,
            operation_id: operation_id(),
        }
    }
}

impl From<ShellError> for CommandError {
    fn from(value: ShellError) -> Self {
        Self {
            code: value.code.as_str().to_owned(),
            message_key: value.message_key,
            recoverable: value.recoverable,
            operation_id: value.operation_id,
        }
    }
}

impl From<PlatformError> for CommandError {
    fn from(value: PlatformError) -> Self {
        let (code, message_key, recoverable) = match value.code() {
            PlatformErrorCode::InvalidInput => ("invalid_input", "error.invalid_input", true),
            PlatformErrorCode::Busy => ("busy", "error.platform_busy", true),
            PlatformErrorCode::PermissionDenied => {
                ("permission_denied", "error.permission_denied", true)
            }
            PlatformErrorCode::StorageUnavailable => {
                ("storage_unavailable", "error.storage_unavailable", true)
            }
            PlatformErrorCode::CredentialUnavailable => (
                "credential_unavailable",
                "error.credential_unavailable",
                true,
            ),
            PlatformErrorCode::CredentialRecoveryRequired => (
                "credential_recovery_required",
                "error.credential_recovery_required",
                true,
            ),
            PlatformErrorCode::SelectionFailed => {
                ("selection_failed", "error.selection_failed", true)
            }
            PlatformErrorCode::SelectedFileTooLarge => (
                "selected_file_too_large",
                "error.selected_file_too_large",
                true,
            ),
            PlatformErrorCode::UnsupportedPlatform => {
                ("unsupported_platform", "error.unsupported_platform", false)
            }
            PlatformErrorCode::Internal => ("internal", "error.internal", false),
        };
        Self {
            code: code.to_owned(),
            message_key: message_key.to_owned(),
            recoverable,
            operation_id: operation_id(),
        }
    }
}

fn operation_id() -> String {
    format!("tauri-{}", Uuid::new_v4())
}

pub type CommandResult<T> = Result<T, CommandError>;

#[cfg(test)]
mod tests {
    use tauri_plugin_lorepia_platform::{PlatformError, PlatformErrorCode};

    use super::CommandError;

    #[test]
    fn platform_error_projection_contains_no_native_diagnostic() {
        let json = serde_json::to_string(&CommandError::from(PlatformError::new(
            PlatformErrorCode::StorageUnavailable,
        )))
        .expect("serialize");
        assert!(!json.contains("/Users/"));
        assert!(!json.contains("sk-"));
    }

    #[test]
    fn credential_recovery_failure_has_a_distinct_safe_projection() {
        let error = CommandError::from(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));

        assert_eq!(error.code, "credential_recovery_required");
        assert_eq!(error.message_key, "error.credential_recovery_required");
        assert!(error.recoverable);
    }

    #[test]
    fn assistant_pricing_failure_is_stable_and_not_recoverable() {
        let error = CommandError::assistant_pricing_unavailable();

        assert_eq!(error.code, "assistant_pricing_unavailable");
        assert_eq!(
            error.message_key,
            "provider.discovery.assistant_pricing_unavailable"
        );
        assert!(!error.recoverable);
    }

    #[test]
    fn generation_reattachment_failure_is_stable_and_not_recoverable() {
        let error = CommandError::generation_reattachment_unavailable();

        assert_eq!(error.code, "generation_reattachment_unavailable");
        assert_eq!(
            error.message_key,
            "chat.generation_reattachment_unavailable"
        );
        assert!(!error.recoverable);
    }
}

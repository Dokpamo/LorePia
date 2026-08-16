use std::fmt;

/// Platform failures intentionally carry no path or credential value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformErrorCode {
    InvalidInput,
    Busy,
    PermissionDenied,
    StorageUnavailable,
    CredentialUnavailable,
    CredentialRecoveryRequired,
    SelectionFailed,
    SelectedFileTooLarge,
    UnsupportedPlatform,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformError {
    code: PlatformErrorCode,
}

impl PlatformError {
    pub const fn new(code: PlatformErrorCode) -> Self {
        Self { code }
    }

    pub const fn code(&self) -> PlatformErrorCode {
        self.code
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            PlatformErrorCode::InvalidInput => "invalid platform input",
            PlatformErrorCode::Busy => "platform operation is already active",
            PlatformErrorCode::PermissionDenied => "platform permission denied",
            PlatformErrorCode::StorageUnavailable => "platform storage unavailable",
            PlatformErrorCode::CredentialUnavailable => "credential service unavailable",
            PlatformErrorCode::CredentialRecoveryRequired => {
                "credential recovery requires user attention"
            }
            PlatformErrorCode::SelectionFailed => "file selection failed",
            PlatformErrorCode::SelectedFileTooLarge => "selected file exceeds the staging limit",
            PlatformErrorCode::UnsupportedPlatform => "platform is unsupported",
            PlatformErrorCode::Internal => "platform operation failed",
        })
    }
}

impl std::error::Error for PlatformError {}

pub type PlatformResult<T> = Result<T, PlatformError>;

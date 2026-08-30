use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, ProviderConnection, ProviderConnectionId,
};
use zeroize::Zeroize;

/// Request-scoped credential material bound to one provider connection.
///
/// This type intentionally implements neither `Clone`, `Serialize`, nor
/// `Display`. The credential allocation is zeroized on drop.
pub struct ConnectionBoundCredential {
    pub(in crate::app) connection_id: ProviderConnectionId,
    pub(in crate::app) value: Option<String>,
    access_authority: Option<lorepia_storage::ProviderCredentialAccessAuthority>,
    dispatch_lease: Option<Box<dyn Send + Sync>>,
}

/// Secret material owned for one primary generation dispatch.
///
/// Bound credentials retain their complete carrier so its native dispatch
/// lease and zeroizing drop remain coupled to the provider future. Legacy raw
/// credentials receive the same zeroizing task-owned lifetime.
pub(in crate::app) enum GenerationCredential {
    Raw(Option<String>),
    Bound(ConnectionBoundCredential),
}

impl GenerationCredential {
    pub(in crate::app) fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Raw(value) => value.as_deref(),
            Self::Bound(credential) => credential.value.as_deref(),
        }
    }
}

impl From<Option<String>> for GenerationCredential {
    fn from(value: Option<String>) -> Self {
        Self::Raw(value)
    }
}

impl From<ConnectionBoundCredential> for GenerationCredential {
    fn from(value: ConnectionBoundCredential) -> Self {
        Self::Bound(value)
    }
}

impl Drop for GenerationCredential {
    fn drop(&mut self) {
        if let Self::Raw(Some(value)) = self {
            value.zeroize();
        }
    }
}

/// Opaque process-local reservation retained only until a durable generation
/// attempt has been admitted.
///
/// Native hosts use this for legacy raw credentials which predate durable
/// credential authority epochs. The reservation has no serialized or debug
/// representation and is dropped before prompt-time auxiliary work begins.
pub struct GenerationCredentialAdmissionLease(Box<dyn Send + Sync>);

impl GenerationCredentialAdmissionLease {
    pub fn new(value: impl Send + Sync + 'static) -> Self {
        Self(Box::new(value))
    }

    pub(in crate::app) fn release(self) {
        drop(self.0);
    }
}

impl ConnectionBoundCredential {
    pub fn new(connection_id: ProviderConnectionId, value: Option<String>) -> Self {
        Self {
            connection_id,
            value,
            access_authority: None,
            dispatch_lease: None,
        }
    }

    /// Binds credential material to the exact durable ownership authority
    /// observed by the native vault read which released it.
    pub fn new_with_access_authority(
        connection_id: ProviderConnectionId,
        value: Option<String>,
        access_authority: lorepia_storage::ProviderCredentialAccessAuthority,
    ) -> Self {
        Self {
            connection_id,
            value,
            access_authority: Some(access_authority),
            dispatch_lease: None,
        }
    }

    /// Retains one process-local native credential lease for the full provider
    /// dispatch lifetime. The lease has no serialized or debug representation.
    pub fn new_with_dispatch_lease(
        connection_id: ProviderConnectionId,
        value: Option<String>,
        dispatch_lease: impl Send + Sync + 'static,
    ) -> Self {
        Self {
            connection_id,
            value,
            access_authority: None,
            dispatch_lease: Some(Box::new(dispatch_lease)),
        }
    }

    /// Attaches a native provider-operation lease without changing the
    /// credential's durable access authority. The carrier releases the lease
    /// only after zeroizing its credential value.
    #[must_use]
    pub fn with_dispatch_lease(mut self, dispatch_lease: impl Send + Sync + 'static) -> Self {
        self.dispatch_lease = Some(Box::new(dispatch_lease));
        self
    }

    pub(crate) fn access_authority(
        &self,
    ) -> Option<&lorepia_storage::ProviderCredentialAccessAuthority> {
        self.access_authority.as_ref()
    }

    pub(crate) fn value_for_connection<'a>(
        &'a self,
        connection: &ProviderConnection,
    ) -> CoreResult<Option<&'a str>> {
        validate_connection_credential_binding(connection, self)?;
        Ok(self.value.as_deref())
    }
}

impl std::fmt::Debug for ConnectionBoundCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ConnectionBoundCredential([REDACTED])")
    }
}

impl Drop for ConnectionBoundCredential {
    fn drop(&mut self) {
        if let Some(value) = &mut self.value {
            value.zeroize();
        }
        drop(self.dispatch_lease.take());
    }
}

pub(in crate::app) fn validate_connection_credential_binding(
    connection: &ProviderConnection,
    credential: &ConnectionBoundCredential,
) -> CoreResult<()> {
    let credential_reference = connection.credential_ref.as_ref();
    if credential.connection_id != connection.id
        || credential_reference
            .is_some_and(|reference| reference.as_str() != credential.connection_id.as_str())
    {
        return Err(CoreError::invalid(
            "credential does not belong to the selected provider connection",
        ));
    }
    let has_credential = credential
        .value
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    match (credential_reference.is_some(), has_credential) {
        (true, false) => {
            return Err(CoreError::new(
                CoreErrorCode::ProviderAuthFailed,
                "provider credential is required",
                false,
            ));
        }
        (false, true) => {
            return Err(CoreError::invalid(
                "this provider connection does not permit a credential",
            ));
        }
        _ => {}
    }
    Ok(())
}

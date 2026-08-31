use super::*;

pub(in crate::provider_commands) type DiscoveryVaultFuture<'a, T> =
    Pin<Box<dyn Future<Output = PlatformResult<T>> + Send + 'a>>;
pub(in crate::provider_commands) type ConnectionSlotGuardFuture<'a> =
    Pin<Box<dyn Future<Output = CommandResult<()>> + Send + 'a>>;
pub(in crate::provider_commands) type ExistingConnectionCredentialReadFuture<'a> = Pin<
    Box<
        dyn Future<
                Output = CommandResult<
                    crate::credential_operations::ProviderConnectionCredentialRead,
                >,
            > + Send
            + 'a,
    >,
>;

pub(in crate::provider_commands) struct CapturedDiscoveryCredential {
    pub(in crate::provider_commands) value: NativeCredential,
    pub(in crate::provider_commands) status: NativeCaptureStatus,
}

pub(in crate::provider_commands) enum PreparedDiscoveryCredentialStore {
    Platform(PreparedBoundCredentialStore),
    #[cfg(test)]
    Fake {
        reference: String,
        value: NativeCredential,
        authority: CredentialAuthority,
    },
}

/// Rust-only one-use approval carried across the prompt-without-lock boundary.
/// The platform receipt cannot be cloned or serialized; the fake variant is
/// compiled only into this module's tests.
pub(in crate::provider_commands) enum DiscoveryCompensationConfirmation {
    Platform(NativeCredentialEffectConfirmation),
    #[cfg(test)]
    Fake {
        effect: NativeCredentialEffect,
        target_id: String,
        origin: String,
        revision: String,
    },
}

impl DiscoveryCompensationConfirmation {
    pub(super) fn consume_exact(
        self,
        context: &NativeCredentialEffectContext,
    ) -> PlatformResult<()> {
        match self {
            Self::Platform(confirmation) => confirmation.consume_exact(
                context.effect(),
                context.target_id(),
                context.origin(),
                context.revision(),
            ),
            #[cfg(test)]
            Self::Fake {
                effect,
                target_id,
                origin,
                revision,
            } => {
                if effect == context.effect()
                    && target_id == context.target_id()
                    && origin == context.origin()
                    && revision == context.revision()
                {
                    Ok(())
                } else {
                    Err(tauri_plugin_lorepia_platform::PlatformError::new(
                        PlatformErrorCode::InvalidInput,
                    ))
                }
            }
        }
    }
}

impl PreparedDiscoveryCredentialStore {
    fn into_platform(self) -> PreparedBoundCredentialStore {
        match self {
            Self::Platform(prepared) => prepared,
            #[cfg(test)]
            Self::Fake { .. } => {
                unreachable!("platform vault received a fake prepared credential store")
            }
        }
    }

    #[cfg(test)]
    pub(in crate::provider_commands) fn into_fake(
        self,
    ) -> (String, NativeCredential, CredentialAuthority) {
        match self {
            Self::Fake {
                reference,
                value,
                authority,
            } => (reference, value, authority),
            Self::Platform(_) => {
                unreachable!("fake vault received a platform prepared credential store")
            }
        }
    }
}

pub(in crate::provider_commands) trait DiscoveryCredentialVault:
    Send + Sync
{
    fn status_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, CredentialStatus>;

    fn capture_bound(&self) -> DiscoveryVaultFuture<'_, CapturedDiscoveryCredential>;

    fn prepare_bound_store(
        &self,
        reference: &str,
        value: NativeCredential,
        authority: &CredentialAuthority,
    ) -> PlatformResult<PreparedDiscoveryCredentialStore>;

    fn store_prepared(
        &self,
        prepared: PreparedDiscoveryCredentialStore,
    ) -> DiscoveryVaultFuture<'_, ()>;

    fn observe_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, BoundCredentialObservation>;

    fn delete_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, ()>;

    fn confirm_compensation(
        &self,
        context: NativeCredentialEffectContext,
    ) -> DiscoveryVaultFuture<'_, DiscoveryCompensationConfirmation>;
}

pub(super) struct PlatformDiscoveryCredentialVault<'a> {
    pub(super) app: &'a AppHandle,
}

impl DiscoveryCredentialVault for PlatformDiscoveryCredentialVault<'_> {
    fn status_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, CredentialStatus> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .bound_credential_status(reference, &authority)
                .await
        })
    }

    fn capture_bound(&self) -> DiscoveryVaultFuture<'_, CapturedDiscoveryCredential> {
        Box::pin(async move {
            let captured = self
                .app
                .lorepia_platform()
                .capture_credential_text_from_clipboard()
                .await?;
            Ok(CapturedDiscoveryCredential {
                status: captured.status(),
                value: NativeCredential::new(captured.into_secret_string()),
            })
        })
    }

    fn prepare_bound_store(
        &self,
        reference: &str,
        value: NativeCredential,
        authority: &CredentialAuthority,
    ) -> PlatformResult<PreparedDiscoveryCredentialStore> {
        self.app
            .lorepia_platform()
            .prepare_bound_credential_store(reference, value, authority)
            .map(PreparedDiscoveryCredentialStore::Platform)
    }

    fn store_prepared(
        &self,
        prepared: PreparedDiscoveryCredentialStore,
    ) -> DiscoveryVaultFuture<'_, ()> {
        let prepared = prepared.into_platform();
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .store_prepared_bound_credential(prepared)
                .await
        })
    }

    fn observe_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, BoundCredentialObservation> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .observe_bound_credential(reference, &authority)
                .await
        })
    }

    fn delete_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, ()> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .delete_bound_credential(reference, &authority)
                .await
        })
    }

    fn confirm_compensation(
        &self,
        context: NativeCredentialEffectContext,
    ) -> DiscoveryVaultFuture<'_, DiscoveryCompensationConfirmation> {
        Box::pin(async move {
            let confirmation = self
                .app
                .lorepia_platform()
                .confirm_credential_effect(context)
                .await?;
            Ok(DiscoveryCompensationConfirmation::Platform(confirmation))
        })
    }
}

pub(in crate::provider_commands) trait NewConnectionSlotGuard:
    Send + Sync
{
    fn ensure_missing<'a>(&'a self, connection_id: &'a str) -> ConnectionSlotGuardFuture<'a>;
}

pub(in crate::provider_commands) struct PlatformNewConnectionSlotGuard<'a> {
    pub(in crate::provider_commands) app: &'a AppHandle,
}

impl NewConnectionSlotGuard for PlatformNewConnectionSlotGuard<'_> {
    fn ensure_missing<'a>(&'a self, connection_id: &'a str) -> ConnectionSlotGuardFuture<'a> {
        Box::pin(async move {
            crate::credential_operations::ensure_new_connection_slot_missing(
                self.app,
                connection_id,
            )
            .await
        })
    }
}

pub(in crate::provider_commands) trait ExistingConnectionCredentialReader:
    Send + Sync
{
    fn read<'a>(
        &'a self,
        shell: &'a shell::ShellApi,
        connection_id: &'a str,
    ) -> ExistingConnectionCredentialReadFuture<'a>;
}

pub(in crate::provider_commands) struct PlatformExistingConnectionCredentialReader<'a> {
    pub(in crate::provider_commands) app: &'a AppHandle,
}

impl ExistingConnectionCredentialReader for PlatformExistingConnectionCredentialReader<'_> {
    fn read<'a>(
        &'a self,
        shell: &'a shell::ShellApi,
        connection_id: &'a str,
    ) -> ExistingConnectionCredentialReadFuture<'a> {
        Box::pin(async move {
            crate::credential_operations::read_provider_connection_credential(
                self.app,
                shell,
                connection_id,
            )
            .await
        })
    }
}
pub(in crate::provider_commands) async fn credential_for_connection_with_reader<
    R: ExistingConnectionCredentialReader + ?Sized,
>(
    reader: &R,
    shell: &shell::ShellApi,
    connection_id: &str,
) -> CommandResult<(
    Option<shell::SecretCredential>,
    Option<shell::ProviderCredentialAccessAuthorityContext>,
)> {
    let connection = find_connection(shell, connection_id)?;
    if !connection.credential_binding_required {
        return Ok((None, None));
    }
    let read = reader.read(shell, connection_id).await?;
    Ok((
        native_credential_to_shell(read.credential),
        Some(read.access_authority),
    ))
}

pub(in crate::provider_commands) async fn credential_authority_for_existing_connection_with_reader<
    R: ExistingConnectionCredentialReader + ?Sized,
>(
    reader: &R,
    shell: &shell::ShellApi,
    connection_id: &str,
) -> CommandResult<Option<shell::ProviderCredentialAccessAuthorityContext>> {
    if !shell
        .list_provider_connections()?
        .iter()
        .any(|connection| connection.id == connection_id)
    {
        return Ok(None);
    }
    let (credential, access_authority) =
        credential_for_connection_with_reader(reader, shell, connection_id).await?;
    drop(credential);
    Ok(access_authority)
}

const fn bound_observation_status(observation: BoundCredentialObservation) -> CredentialStatus {
    match observation {
        BoundCredentialObservation::Missing => CredentialStatus::Missing,
        BoundCredentialObservation::Match => CredentialStatus::Available,
        BoundCredentialObservation::Legacy
        | BoundCredentialObservation::Mismatch
        | BoundCredentialObservation::Unreadable => CredentialStatus::Unreadable,
    }
}

/// Status and recovery classification must remain fail-closed without turning
/// an unreadable platform item into a bootstrap outage. Mutation and provider
/// dispatch paths continue to propagate the original platform error.
pub(crate) fn status_only_bound_observation(
    observation: PlatformResult<BoundCredentialObservation>,
) -> CredentialStatus {
    observation.map_or(CredentialStatus::Unreadable, bound_observation_status)
}

fn native_credential_to_shell(value: Option<NativeCredential>) -> Option<shell::SecretCredential> {
    value.map(|value| shell::SecretCredential::new(value.into_secret_string()))
}
pub(in crate::provider_commands) fn find_connection(
    shell: &shell::ShellApi,
    connection_id: &str,
) -> CommandResult<shell::ProviderConnectionDto> {
    shell
        .list_provider_connections()?
        .into_iter()
        .find(|connection| connection.id == connection_id)
        .ok_or_else(CommandError::invalid_input)
}

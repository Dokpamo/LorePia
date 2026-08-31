//! Native-host coordination for ordinary provider credential effects.
//!
//! The webview never sees a credential, vault reference, authority marker, or
//! journal plan. Every native mutation is preceded by Core's durable cutpoint
//! and recovery only observes state; it never repeats a store/delete effect.

mod authority;
mod cleanup;
mod execute;
mod prepare;
mod recovery;
mod types;

pub(crate) use execute::{
    archive_provider_connection, capture_legacy_provider_credential,
    capture_provider_connection_credential, delete_legacy_provider_credential,
    delete_provider_connection_credential, read_legacy_provider_credential,
    read_provider_connection_credential,
};
pub(crate) use prepare::{
    ensure_new_connection_slot_missing, provider_connection_credential_effect_context,
};
pub(crate) use recovery::recover_provider_credential_operations;
pub(crate) use types::ProviderConnectionCredentialRead;

#[cfg(test)]
use crate::error::{CommandError, CommandResult};
#[cfg(test)]
use authority::{operation_authority, operation_predecessor_authority};
#[cfg(test)]
use execute::{
    capture_legacy_provider_credential_with, capture_provider_connection_credential_with,
    capture_provider_connection_credential_with_policy, delete_legacy_provider_credential_with,
    legacy_provider_credential_status_with, read_legacy_provider_credential_with,
    read_provider_connection_credential_with, remove_provider_credential_with,
    remove_provider_credential_with_policy,
};
#[cfg(test)]
use prepare::ensure_slot_missing;
#[cfg(test)]
use recovery::{
    recover_provider_credential_operations_with, recover_provider_credential_slot_garbage_with,
};
#[cfg(test)]
use types::{
    CapturedCredential, CredentialVault, LegacyCredentialAccess, OrdinaryCredentialTargetPolicy,
    PreparedCredentialStore, VaultFuture,
};

#[cfg(test)]
mod tests {
    include!("credential_operations/tests/support.rs");
    include!("credential_operations/tests/install_and_replacement.rs");
    include!("credential_operations/tests/garbage_and_failure.rs");
    include!("credential_operations/tests/cleanup_and_restart.rs");
    include!("credential_operations/tests/snapshot_and_helpers.rs");
}

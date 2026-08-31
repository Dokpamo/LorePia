mod compensation;
mod install;
mod vault;

use compensation::platform_result_requires_credential_recovery;
#[cfg(test)]
pub(in crate::provider_commands) use compensation::{
    CompensationCredentialEffectPolicy, CredentialCompensationDeleteOutcome,
    DiscoveryCompensationDriveResult, credential_compensation_delete_outcome,
    delete_and_observe_discovery_bound_slot, discovery_compensation_confirmation_context,
    discovery_compensation_credential_authority, drive_provider_discovery_compensation_with,
    observe_discovery_compensation_slot,
};
pub(in crate::provider_commands) use compensation::{
    CompensationObserveErrorPolicy, drive_provider_discovery_compensation_explicit,
    drive_provider_discovery_compensation_observe_only,
};
#[cfg(test)]
pub(in crate::provider_commands) use install::{
    CredentialInstallRecoveryAction, credential_install_recovery_action,
};
#[cfg(test)]
pub(in crate::provider_commands) use install::{
    DiscoveryCredentialCommitCandidate, DiscoveryCredentialInstallJournal,
    capture_discovery_credential_for_empty_bound_slot_with,
    capture_precommit_discovery_credential_with, discovery_committing_credential_status_with,
    promote_discovery_credential_lease_with, settle_started_discovery_credential_recovery,
};
pub(crate) use install::{
    capture_discovery_credential_for_empty_bound_slot, capture_precommit_discovery_credential,
    discovery_credential_authority, discovery_credential_reservation_authority,
    discovery_credential_status,
};
pub(in crate::provider_commands) use install::{
    credential_for_discovery_action, promote_discovery_credential_lease,
    recover_provider_discovery_credential_installs, require_started_discovery_credential_install,
};
use vault::PlatformDiscoveryCredentialVault;
pub(crate) use vault::status_only_bound_observation;
pub(in crate::provider_commands) use vault::{
    CapturedDiscoveryCredential, DiscoveryCompensationConfirmation, DiscoveryCredentialVault,
    ExistingConnectionCredentialReader, NewConnectionSlotGuard,
    PlatformExistingConnectionCredentialReader, PlatformNewConnectionSlotGuard,
    credential_authority_for_existing_connection_with_reader,
    credential_for_connection_with_reader, find_connection,
};
#[cfg(test)]
pub(in crate::provider_commands) use vault::{
    ConnectionSlotGuardFuture, DiscoveryVaultFuture, ExistingConnectionCredentialReadFuture,
    PreparedDiscoveryCredentialStore,
};

import type { CredentialTargetDto, ProviderDiscoverySessionDto } from '../lib/ipc/contracts';

export function credentialKey(target: CredentialTargetDto): string {
    switch (target.kind) {
        case 'connection':
            return `connection:${target.connection_id}`;
        case 'legacy_profile':
            return `legacy_profile:${target.provider_profile_id}`;
        case 'discovery_session':
            return `discovery_session:${target.session_id}`;
    }
}

export function discoveryCredentialTarget(
    session: ProviderDiscoverySessionDto,
): Extract<CredentialTargetDto, { kind: 'discovery_session' }> | null {
    if (!session.credential_binding_requested) return null;
    const eligible =
        session.state === 'awaiting_credential_origin_approval' ||
        session.state === 'awaiting_probe_consent' ||
        session.state === 'awaiting_review' ||
        session.state === 'committing' ||
        (session.state === 'interrupted' &&
            (session.recovery_operation === 'list_models' ||
                session.recovery_operation === 'probe_capabilities'));
    return eligible
        ? {
              kind: 'discovery_session',
              session_id: session.id,
              expected_revision: session.revision,
          }
        : null;
}

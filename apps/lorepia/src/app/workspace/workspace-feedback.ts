import { t, type MessageKey } from '../../lib/i18n';

const labels: Readonly<Record<string, MessageKey>> = {
    'error.not_found': 'workspace.errorNotFound',
    'error.storage_unavailable': 'workspace.errorStorageUnavailable',
    'error.storage_corrupted': 'workspace.errorStorageCorrupted',
    'error.provider_auth_failed': 'workspace.errorProviderAuth',
    'error.provider_rate_limited': 'workspace.errorRateLimited',
    'error.provider_unavailable': 'workspace.errorProviderUnavailable',
    'error.network_unavailable': 'workspace.errorNetwork',
    'error.permission_denied': 'workspace.errorPermission',
    'error.cancelled': 'workspace.errorCancelled',
};

export function workspaceFeedback(value: string): string {
    const key = labels[value];
    if (key) return t(key);
    return value.startsWith('error.') ? t('error.unexpected') : value;
}

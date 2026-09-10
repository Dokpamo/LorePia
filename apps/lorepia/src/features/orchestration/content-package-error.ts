import { t } from '../../lib/i18n';
import { normalizeClientError } from '../../lib/ipc/errors';

export function contentPackageErrorLabel(error: unknown): string {
    const normalized = normalizeClientError(error);
    switch (normalized.messageKey) {
        case 'error.invalid_input':
            return t('error.invalid_input');
        case 'error.unexpected':
            return t('content_package.error.generic');
        default:
            return normalized.messageKey;
    }
}

import { describe, expect, it } from 'vitest';

import { t } from '../../lib/i18n';
import { LorepiaClientError } from '../../lib/ipc/errors';
import { contentPackageErrorLabel } from './content-package-error';

describe('contentPackageErrorLabel', () => {
    it('translates a common client error instead of exposing its catalog key', () => {
        const error = new LorepiaClientError({
            code: 'invalid_input',
            message_key: 'error.invalid_input',
            recoverable: false,
            operation_id: null,
            field_errors: [],
        });

        expect(contentPackageErrorLabel(error)).toBe(t('error.invalid_input'));
    });
});

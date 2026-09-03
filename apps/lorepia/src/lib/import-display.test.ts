import { describe, expect, it } from 'vitest';

import { importedLicenseLabel, importedText } from './import-display';

describe('imported content display labels', () => {
    it('neutralizes legacy source branding without changing stored identifiers', () => {
        const legacyName = ['Ri', 'su preset / ', '\uB9AC\uC218 \uBAA8\uB4C8'].join('');

        expect(importedText(legacyName)).toBe('\uC678\uBD80 preset / \uC678\uBD80 \uBAA8\uB4C8');
    });

    it('maps the legacy imported license to a neutral user-facing label', () => {
        const legacyLicense = ['LicenseRef-', 'Ri', 'su-User-Content'].join('');

        expect(importedLicenseLabel(legacyLicense)).toBe(
            '\uAC00\uC838\uC628 \uC0AC\uC6A9\uC790 \uCF58\uD150\uCE20',
        );
    });
});

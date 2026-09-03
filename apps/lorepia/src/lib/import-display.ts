const LEGACY_IMPORTED_LICENSE = 'LicenseRef-Risu-User-Content';
const LEGACY_SOURCE_BRAND_PATTERN = /risu|\uB9AC\uC218/gi;

export function importedText(value: string, replacement = '\uC678\uBD80'): string {
    return value.replaceAll(LEGACY_SOURCE_BRAND_PATTERN, replacement);
}

export function importedLicenseLabel(value: string): string {
    return value === LEGACY_IMPORTED_LICENSE
        ? '\uAC00\uC838\uC628 \uC0AC\uC6A9\uC790 \uCF58\uD150\uCE20'
        : importedText(value);
}

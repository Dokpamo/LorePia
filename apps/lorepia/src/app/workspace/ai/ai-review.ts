import { t, type MessageKey } from '../../../lib/i18n';
import { koWorkspaceRuntime } from '../../../lib/i18n/ko-workspace-runtime';

export interface ReviewRow {
    label: string;
    value: string;
}

function fieldLabel(key: string): string {
    const message = `workspaceReview.field.${key}`;
    if (Object.hasOwn(koWorkspaceRuntime, message)) return t(message as MessageKey);
    // Provider-defined names are preserved, including unknown future consent fields.
    return key.replace(/_/g, ' ').replace(/\b[a-z]/g, (letter) => letter.toUpperCase());
}

/** Keep every native-approved value, array position and empty container visible. */
export function reviewRows(value: unknown, label: string): ReviewRow[] {
    const rows: ReviewRow[] = [];
    function visit(item: unknown, path: string[]) {
        if (Array.isArray(item)) {
            if (item.length === 0)
                rows.push({ label: path.join(' · '), value: t('workspaceReview.empty') });
            item.forEach((entry, index) =>
                visit(entry, [...path, t('workspaceReview.item', { index: index + 1 })]),
            );
        } else if (item !== null && typeof item === 'object') {
            const entries = Object.entries(item);
            if (entries.length === 0)
                rows.push({ label: path.join(' · '), value: t('workspaceReview.empty') });
            entries.forEach(([key, entry]) => visit(entry, [...path, fieldLabel(key)]));
        } else {
            const text =
                item === null || item === undefined
                    ? t('workspaceReview.empty')
                    : typeof item === 'boolean'
                      ? t(item ? 'workspaceReview.true' : 'workspaceReview.false')
                      : item === ''
                        ? t('workspaceReview.blank')
                        : typeof item === 'string'
                          ? item
                          : typeof item === 'number'
                            ? String(item)
                            : t('workspaceReview.empty');
            rows.push({ label: path.join(' · '), value: text });
        }
    }
    visit(value, [label]);
    return rows;
}

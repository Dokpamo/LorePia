import type { ProviderCatalogDiffDto } from '../../lib/ipc/contracts';

export function changeCount(diff: ProviderCatalogDiffDto): number {
    return diff.manifest_changes.length + diff.model_changes.length;
}

export function securityChanges(
    diff: ProviderCatalogDiffDto,
): ProviderCatalogDiffDto['manifest_changes'] {
    return diff.manifest_changes.filter((change) => change.security_review != null);
}

export function reviewJson(value: unknown): string {
    return value === undefined ? '—' : JSON.stringify(value);
}

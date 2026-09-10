import type { ProviderCatalogHistoryDto, ProviderCatalogStatusDto } from '../lib/ipc/contracts';

const CATALOG_SHA = 'b'.repeat(64);
const BASELINE_SHA = 'c'.repeat(64);

export const DEMO_CATALOG_STATUS: ProviderCatalogStatusDto = {
    status_schema_version: 1,
    state_version: 4,
    active_revision: 4,
    active_snapshot_sha256: CATALOG_SHA,
    bundled_baseline_sha256: BASELINE_SHA,
    snapshot_count: 4,
    signed_update_count: 2,
    highest_accepted_revision: 4,
    latest_issued_at: '2026-08-22T00:00:00.000Z',
    active_signed_revisions: [3, 4],
};

export const DEMO_CATALOG_HISTORY: ProviderCatalogHistoryDto = {
    history_schema_version: 1,
    active_revision: 4,
    revisions: [
        {
            revision: 4,
            captured_at: '2026-08-22T00:00:00.000Z',
            snapshot_sha256: CATALOG_SHA,
            signed_revisions: [4],
            active: true,
        },
        {
            revision: 3,
            captured_at: '2026-08-10T00:00:00.000Z',
            snapshot_sha256: 'd'.repeat(64),
            signed_revisions: [3],
            active: false,
        },
    ],
    activations: [],
    next_before_revision: null,
    next_before_state_version: null,
};

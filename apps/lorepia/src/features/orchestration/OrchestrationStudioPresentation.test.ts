import { describe, expect, it } from 'vitest';

import studioSource from './OrchestrationStudio.svelte?raw';

describe('Orchestration Studio pushed-page presentation', () => {
    it('keeps package import actions in one stage-dependent fixed action bar', () => {
        expect(studioSource).toContain('class="studio-card package-detail"');
        expect(
            studioSource.match(/<DetailActionBar fixed ariaLabel=/g)?.length,
        ).toBeGreaterThanOrEqual(2);
        expect(studioSource).toContain("contentPackageState.phase === 'selection_ready'");
        expect(studioSource).toContain("contentPackageState.phase === 'approved'");
        expect(studioSource).toContain('void contentPackageController.reviewSelection()');
        expect(studioSource).toContain('void contentPackageController.approve()');
        expect(studioSource).toContain('void contentPackageController.commit()');
        expect(studioSource).toContain('void contentPackageController.discard()');
    });

    it('renders diagnostics as flat pushed pages without changing shared chat panels', () => {
        expect(studioSource.match(/class="studio-card diagnostic-flat"/g)).toHaveLength(2);
        expect(studioSource).toContain('class="studio-card plan-detail"');
        expect(studioSource).toContain('class="plan-embedded-panel"');
        expect(studioSource).toContain('.plan-embedded-panel :global(.attempt-approvals)');
        expect(studioSource).toContain('.plan-embedded-panel :global(.memory-query-retry)');
    });

    it('keeps compute, new-preview, and reviewed-send controls in the fixed plan bar', () => {
        expect(studioSource).toContain('class="studio-card plan-detail"');
        expect(studioSource).toContain('void resolveNewPlanPreviewAndRefreshRetries()');
        expect(studioSource).toContain('void resolvePlanPreviewAndRefreshRetries()');
        expect(studioSource).toContain('void sendReviewedPlan()');
        expect(studioSource).toContain('controller.reviewedPromptSendInput() === null');
    });
});

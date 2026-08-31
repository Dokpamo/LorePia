import { describe, expect, it } from 'vitest';

import studioSource from './OrchestrationStudio.svelte?raw';
import contentPackageReviewSource from './studio/ContentPackageReview.svelte?raw';
import contentSource from './studio/ContentSection.svelte?raw';
import diagnosticsSource from './studio/DiagnosticsSection.svelte?raw';
import runtimePlanSource from './studio/RuntimePlanSection.svelte?raw';
import studioStylesA from './studio/styles/studio-a.css?raw';
import studioStylesB from './studio/styles/studio-b.css?raw';

const studioPresentationSource = [
    studioSource,
    contentPackageReviewSource,
    contentSource,
    diagnosticsSource,
    runtimePlanSource,
    studioStylesA,
    studioStylesB,
].join('\n');

describe('Orchestration Studio pushed-page presentation', () => {
    it('keeps package import actions in one stage-dependent fixed action bar', () => {
        expect(studioPresentationSource).toContain('class="studio-card package-detail"');
        expect(
            studioPresentationSource.match(/<DetailActionBar fixed ariaLabel=/g)?.length,
        ).toBeGreaterThanOrEqual(2);
        expect(studioPresentationSource).toContain(
            "contentPackageState.phase === 'selection_ready'",
        );
        expect(studioPresentationSource).toContain("contentPackageState.phase === 'approved'");
        expect(studioPresentationSource).toContain(
            'void contentPackageController.reviewSelection()',
        );
        expect(studioPresentationSource).toContain('void contentPackageController.approve()');
        expect(studioPresentationSource).toContain('void contentPackageController.commit()');
        expect(studioPresentationSource).toContain('void contentPackageController.discard()');
    });

    it('renders diagnostics as flat pushed pages without changing shared chat panels', () => {
        expect(studioPresentationSource.match(/class="studio-card diagnostic-flat"/g)).toHaveLength(
            2,
        );
        expect(studioPresentationSource).toContain('class="studio-card plan-detail"');
        expect(studioPresentationSource).toContain('class="plan-embedded-panel"');
        expect(studioPresentationSource).toContain('.plan-embedded-panel .attempt-approvals');
        expect(studioPresentationSource).toContain('.plan-embedded-panel .memory-query-retry');
    });

    it('keeps compute, new-preview, and reviewed-send controls in the fixed plan bar', () => {
        expect(studioPresentationSource).toContain('class="studio-card plan-detail"');
        expect(studioPresentationSource).toContain('void resolveNewPlanPreviewAndRefreshRetries()');
        expect(studioPresentationSource).toContain('void resolvePlanPreviewAndRefreshRetries()');
        expect(studioPresentationSource).toContain('void sendReviewedPlan()');
        expect(studioPresentationSource).toContain('controller.reviewedPromptSendInput() === null');
    });
});

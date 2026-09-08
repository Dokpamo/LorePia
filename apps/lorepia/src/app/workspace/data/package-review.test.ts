import { expect, it } from 'vitest';
import { contentPackageSelectionState } from '../../../features/orchestration/tests/content-package-fixtures';
import { canApprovePackage } from './package-review';
it('requires exact update revision consent and explicit portable runtime permission before package approval', () => {
    const state = contentPackageSelectionState();
    expect(canApprovePackage(state)).toBe(false);
    state.confirmed_update_targets = [
        {
            source_component_id: 'component-safe',
            component_document_ordinal: 0,
            target_object_id: 'prompt-existing',
            expected_target_revision_id: 'prompt-revision-7',
            expected_target_state_revision: 8,
        },
    ];
    expect(canApprovePackage(state)).toBe(true);
    state.required_capabilities = ['portable_runtime'];
    expect(canApprovePackage(state)).toBe(false);
    state.approved_capabilities = ['portable_runtime'];
    expect(canApprovePackage(state)).toBe(true);
    const confirmation = state.confirmed_update_targets[0];
    if (!confirmation) throw new Error('missing fixture confirmation');
    confirmation.expected_target_state_revision = 9;
    expect(canApprovePackage(state)).toBe(false);
});

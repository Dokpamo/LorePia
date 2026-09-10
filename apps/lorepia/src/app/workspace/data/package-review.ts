import type { ContentPackageState } from '../../../features/orchestration/content-package-controller';
import type {
    ContentPackageTargetReviewDocumentDto,
    ContentPackageCapabilityDto,
    ApprovableContentPackageCapabilityDto,
} from '../../../lib/ipc/contracts';
import { isApprovableContentPackageCapability } from '../../../features/orchestration/content-package-capabilities';
export function needsApproval(
    value: ContentPackageCapabilityDto,
): value is ApprovableContentPackageCapabilityDto {
    return isApprovableContentPackageCapability(value);
}
export function updateConfirmed(
    state: ContentPackageState,
    document: ContentPackageTargetReviewDocumentDto,
): boolean {
    return state.confirmed_update_targets.some(
        (item) =>
            item.source_component_id === document.source_component_id &&
            item.component_document_ordinal === document.component_document_ordinal &&
            item.target_object_id === document.target_object_id &&
            item.expected_target_revision_id === document.expected_target_revision_id &&
            item.expected_target_state_revision === document.expected_target_state_revision,
    );
}
export function canApprovePackage(state: ContentPackageState): boolean {
    if (state.phase !== 'selection_ready' || !state.selection) return false;
    const updates = state.selection.target_review.documents.filter(
        (item) => item.disposition === 'update',
    );
    const capabilities = state.required_capabilities.filter(needsApproval);
    return (
        updates.length === state.confirmed_update_targets.length &&
        updates.every((item) => updateConfirmed(state, item)) &&
        capabilities.length === state.approved_capabilities.length &&
        capabilities.every((item) => state.approved_capabilities.includes(item))
    );
}

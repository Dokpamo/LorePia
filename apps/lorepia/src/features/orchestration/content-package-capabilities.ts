import type { ContentPackageCapabilityDto } from '../../lib/ipc/contracts';

const APPROVABLE = new Set<ContentPackageCapabilityDto>([
    'transforms',
    'declarative_interactions',
    'portable_runtime',
]);

export function isApprovableContentPackageCapability(
    capability: ContentPackageCapabilityDto,
): boolean {
    return APPROVABLE.has(capability);
}

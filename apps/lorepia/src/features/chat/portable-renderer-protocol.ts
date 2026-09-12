import { isPortableAction } from './portable-renderer-policy';

export const PORTABLE_RENDERER_CHANNEL = 'lorepia-portable-renderer-v1';
// A layout bound, not a scroll viewport. Long messages grow into the transcript.
export const MAX_PORTABLE_RENDERER_HEIGHT = 65_536;
export const MIN_PORTABLE_RENDERER_HEIGHT = 32;
export const MAX_PORTABLE_REGIONS = 128;
export interface PortableRegion {
    x: number;
    y: number;
    width: number;
    height: number;
}

export type PortableRendererMessage =
    | {
          channel: typeof PORTABLE_RENDERER_CHANNEL;
          type: 'portable_regions';
          runtimeId: string;
          regions: PortableRegion[];
      }
    | {
          channel: typeof PORTABLE_RENDERER_CHANNEL;
          type: 'portable_action';
          runtimeId: string;
          action: string;
      }
    | {
          channel: typeof PORTABLE_RENDERER_CHANNEL;
          type: 'portable_resize';
          runtimeId: string;
          height: number;
      };

export function isPortableRendererMessage(
    value: unknown,
    runtimeId: string,
): value is PortableRendererMessage {
    if (!isRecord(value) || value.channel !== PORTABLE_RENDERER_CHANNEL) return false;
    if (value.runtimeId !== runtimeId) return false;
    if (value.type === 'portable_action') return isPortableAction(value.action);
    if (value.type === 'portable_regions')
        return (
            Array.isArray(value.regions) &&
            value.regions.length <= MAX_PORTABLE_REGIONS &&
            value.regions.every(
                (region: unknown) =>
                    isRecord(region) &&
                    ['x', 'y', 'width', 'height'].every(
                        (key) =>
                            typeof region[key] === 'number' &&
                            Number.isFinite(region[key]) &&
                            region[key] >= 0 &&
                            region[key] <= MAX_PORTABLE_RENDERER_HEIGHT,
                    ),
            )
        );
    return (
        value.type === 'portable_resize' &&
        typeof value.height === 'number' &&
        Number.isFinite(value.height) &&
        Number.isInteger(value.height) &&
        value.height >= MIN_PORTABLE_RENDERER_HEIGHT &&
        value.height <= MAX_PORTABLE_RENDERER_HEIGHT
    );
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null;
}

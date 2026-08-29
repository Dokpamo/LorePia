import { isPortableAction } from './portable-renderer-policy';

export const PORTABLE_RENDERER_CHANNEL = 'lorepia-portable-renderer-v1';
export const MAX_PORTABLE_RENDERER_HEIGHT = 720;
export const MIN_PORTABLE_RENDERER_HEIGHT = 32;

export type PortableRendererMessage =
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

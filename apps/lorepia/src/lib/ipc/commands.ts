/**
 * Command/event names exposed to frontend callers.
 *
 * `LOREPIA_COMMANDS` is generated from the reviewed repository manifest.
 * Events remain explicit here because they are not Tauri commands.
 */
export { LOREPIA_COMMANDS } from './commands.generated';

export const LOREPIA_EVENTS = {
    memorySupervisorStatus: 'memory-supervisor-status',
    interactionEffect: 'interaction-effect',
} as const;

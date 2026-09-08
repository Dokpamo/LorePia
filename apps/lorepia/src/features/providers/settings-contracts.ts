/**
 * Settings destinations.
 *
 * The index and the pushed screens both name sections by this type, so adding
 * one is a single edit that the compiler then chases through the list, the
 * titles, and the screen that renders it.
 */
export const SETTINGS_SECTIONS = [
    'ai',
    'prompt',
    'memory',
    'plugins',
    'language',
    'storage',
    'appearance',
    'persona',
    'target',
    'connections',
    'templates',
    'discovery',
    'catalog',
    'advanced',
    'licenses',
] as const;

export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

export const PRIMARY_SETTINGS_SECTIONS = [
    'ai',
    'persona',
    'prompt',
    'memory',
    'appearance',
    'language',
    'plugins',
    'storage',
] as const satisfies readonly SettingsSection[];

export function settingsParent(section: SettingsSection): SettingsSection | null {
    return ['target', 'connections', 'templates', 'discovery', 'catalog', 'advanced'].includes(
        section,
    )
        ? 'ai'
        : null;
}

/**
 * A pushed screen inside a settings destination.
 *
 * The owning panel interprets the value. Keeping the route in App means the
 * fixed title and back button can pop one level before leaving the section,
 * exactly like the Persona list/editor flow.
 */
export type SettingsDetailPage = string | null;

/**
 * Settings destinations.
 *
 * The index and the pushed screens both name sections by this type, so adding
 * one is a single edit that the compiler then chases through the list, the
 * titles, and the screen that renders it.
 */
export const SETTINGS_SECTIONS = [
    'appearance',
    'persona',
    'target',
    'connections',
    'templates',
    'discovery',
    'catalog',
    'advanced',
] as const;

export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

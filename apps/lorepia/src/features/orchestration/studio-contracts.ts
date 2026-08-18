/**
 * Studio destinations.
 *
 * The prompt workshop is too large for one scroll on a phone, so it is an
 * index of four screens. Naming them here keeps the list, the titles, and the
 * screen that renders each one from drifting apart.
 */
export const STUDIO_SECTIONS = ['prompt', 'memory', 'content', 'diagnostics'] as const;

export type StudioSection = (typeof STUDIO_SECTIONS)[number];

import type { Component } from 'svelte';

/** Only explicitly mounted fixture hosts supply this presentation component. */
export const assetPresentationContext = Symbol('asset-presentation');
export type AssetPresentation = Component<{
    assetId: string;
    name: string;
    fit?: 'cover' | 'contain';
}>;

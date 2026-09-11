/** View-local scheduling only; it never changes native delivery authorization. */
export const assetLoadPriorityContext = Symbol('asset-load-priority');
export type AssetLoadPriority = () => number;

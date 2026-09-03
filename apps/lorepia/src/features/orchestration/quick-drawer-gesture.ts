export function panelSwipeCommitDistance(viewportWidth: number): number {
    return Math.min(120, Math.max(64, viewportWidth * 0.22));
}

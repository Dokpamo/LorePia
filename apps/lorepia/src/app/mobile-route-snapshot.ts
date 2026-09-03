function copyElementState(source: HTMLElement, clone: HTMLElement): void {
    const sources = [source, ...source.querySelectorAll<HTMLElement>('*')];
    const clones = [clone, ...clone.querySelectorAll<HTMLElement>('*')];
    const count = Math.min(sources.length, clones.length);
    for (let index = 0; index < count; index += 1) {
        const sourceElement = sources[index];
        const cloneElement = clones[index];
        if (!sourceElement || !cloneElement) continue;
        if (sourceElement.scrollTop !== 0) {
            cloneElement.dataset.snapshotScrollTop = String(sourceElement.scrollTop);
        }
        if (sourceElement.scrollLeft !== 0) {
            cloneElement.dataset.snapshotScrollLeft = String(sourceElement.scrollLeft);
        }
        if (sourceElement instanceof HTMLInputElement && cloneElement instanceof HTMLInputElement) {
            cloneElement.value = sourceElement.value;
            cloneElement.checked = sourceElement.checked;
        } else if (
            sourceElement instanceof HTMLTextAreaElement &&
            cloneElement instanceof HTMLTextAreaElement
        ) {
            cloneElement.value = sourceElement.value;
        } else if (
            sourceElement instanceof HTMLSelectElement &&
            cloneElement instanceof HTMLSelectElement
        ) {
            cloneElement.value = sourceElement.value;
        }
    }
}

export function snapshotClone(source: HTMLElement): HTMLElement {
    const clone = source.cloneNode(true) as HTMLElement;
    copyElementState(source, clone);
    return clone;
}

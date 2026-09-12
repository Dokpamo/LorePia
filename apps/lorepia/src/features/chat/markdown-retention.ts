import type { MarkdownBlock, MarkdownInline } from './markdown';

/** Preserve UTF16 (including lone surrogates) without retaining a source slice. */
function detached(value: string): string {
    return JSON.parse(JSON.stringify(value)) as string;
}

function detachInline(nodes: MarkdownInline[]): void {
    for (const node of nodes) {
        if (node.kind === 'text' || node.kind === 'code') node.value = detached(node.value);
        else {
            if (node.kind === 'link') node.href = detached(node.href);
            detachInline(node.children);
        }
    }
}

/** Only newly finalized blocks: prior prefix objects are never revisited. */
export function detachMarkdownBlocks(blocks: MarkdownBlock[], count: number): void {
    for (let index = 0; index < count; index++) {
        const block = blocks[index];
        if (block === undefined || block.kind === 'rule') continue;
        if (block.kind === 'code') {
            block.value = detached(block.value);
            if (block.language !== null) block.language = detached(block.language);
        } else if (block.kind === 'paragraph') detachInline(block.children);
        else {
            for (const line of block.kind === 'quote' ? block.lines : block.items)
                detachInline(line);
        }
    }
}

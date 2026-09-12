import { afterEach, expect, it } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import MarkdownText from './MarkdownText.svelte';
import {
    MAX_MARKDOWN_NODES,
    parseMarkdown,
    type MarkdownInline,
    type MarkdownBlock,
} from './markdown';

afterEach(cleanup);
const inlineText = (nodes: MarkdownInline[]): string =>
    nodes.map((n) => ('value' in n ? n.value : inlineText(n.children))).join('');
function renderedText(blocks: MarkdownBlock[]): string {
    return blocks
        .map((b) =>
            b.kind === 'code'
                ? b.value
                : b.kind === 'paragraph'
                  ? inlineText(b.children)
                  : b.kind === 'list'
                    ? b.items.map(inlineText).join('\n')
                    : b.kind === 'quote'
                      ? b.lines.map(inlineText).join('\n')
                      : '',
        )
        .join('\n');
}
function inlineCost(nodes: MarkdownInline[]): number {
    return nodes.reduce(
        (n, node) =>
            n +
            (node.kind === 'text' ? 1 : node.kind === 'code' ? 2 : 1 + inlineCost(node.children)),
        0,
    );
}
function cost(blocks: MarkdownBlock[]): number {
    return blocks.reduce(
        (n, b) =>
            n +
            (b.kind === 'code'
                ? 3
                : b.kind === 'rule'
                  ? 1
                  : b.kind === 'paragraph'
                    ? 1 + inlineCost(b.children)
                    : 1 +
                      (b.kind === 'list' ? b.items : b.lines).reduce(
                          (sum, line) => sum + 1 + inlineCost(line),
                          0,
                      )),
        0,
    );
}
it.each([
    ['paragraph', 'a\n\n'.repeat(20000), 20000],
    ['rule', '---\n'.repeat(15000), 0],
    ['list', '- a\n'.repeat(15000), 15000],
    ['quote', '> a\n'.repeat(15000), 15000],
] as const)(
    'bounds the whole %s render tree and keeps the source remainder',
    (_name, source, letters) => {
        const blocks = parseMarkdown(source);
        expect(cost(blocks)).toBeLessThanOrEqual(MAX_MARKDOWN_NODES);
        const last = blocks.at(-1);
        expect(last?.kind).toBe('paragraph');
        const remainder = last?.kind === 'paragraph' ? inlineText(last.children) : '';
        expect(remainder.length).toBeGreaterThan(0);
        expect(source.endsWith(remainder)).toBe(true);
        expect(renderedText(blocks).match(/a/g)?.length ?? 0).toBe(letters);
        const { container } = render(MarkdownText, { text: source });
        const root = container.querySelector('.markdown');
        if (!root) throw new Error('missing markdown');
        const walker = document.createTreeWalker(
            root,
            NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT,
        );
        let units = 0;
        while (walker.nextNode())
            if (
                walker.currentNode.nodeType === Node.ELEMENT_NODE ||
                walker.currentNode.textContent !== ''
            )
                units++;
        expect(units).toBeLessThanOrEqual(MAX_MARKDOWN_NODES);
        expect(root.textContent.match(/a/g)?.length ?? 0).toBe(letters);
    },
);
it('rolls back an over-budget formatted block to exact literal source', () => {
    const tail = '**a `b` [c](https://example.com)** '.repeat(5000);
    const source = 'kept\n\n' + tail;
    const blocks = parseMarkdown(source);
    expect(cost(blocks)).toBeLessThanOrEqual(MAX_MARKDOWN_NODES);
    expect(blocks.at(-1)).toEqual({ kind: 'paragraph', children: [{ kind: 'text', value: tail }] });
});
it.each([511, 512, 513, 514])(
    'keeps every one of %i fenced lines with and without a closing fence',
    (count) => {
        const lines = Array.from(
            { length: count },
            (_, i) => `LINE_${String(i + 1).padStart(4, '0')}`,
        ).join('\n');
        for (const close of ['', '\n```']) {
            expect(parseMarkdown('```ts\n' + lines + close)).toEqual([
                { kind: 'code', language: 'ts', value: lines },
            ]);
        }
    },
);

it('keeps finalized rendered paragraphs when the streaming suffix changes', async () => {
    const view = render(MarkdownText, { text: 'first\n\n**open' });
    const first = view.container.querySelector('p');
    await view.rerender({ text: 'first\n\n**open closed**' });
    expect(view.container.querySelector('p')).toBe(first);
    expect(view.container.querySelector('strong')?.textContent).toBe('open closed');
    await view.rerender({ text: 'replacement' });
    expect(view.container.textContent).toBe('replacement');
});

import { afterEach, expect, it, vi } from 'vitest';
import { parseMarkdown, type MarkdownBlock } from './markdown';
import { createIncrementalMarkdownParser } from './markdown-incremental';
import * as retention from './markdown-retention';

afterEach(() => vi.restoreAllMocks());

it('detaches every newly stable payload while preserving UTF16 and node identities', () => {
    const value = 'long payload \ud800 lone \udc00 and \ud83d\ude42';
    const blocks: MarkdownBlock[] = [
        { kind: 'paragraph', children: [{ kind: 'text', value }] },
        { kind: 'code', language: 'typescript', value },
        { kind: 'quote', lines: [[{ kind: 'strong', children: [{ kind: 'code', value }] }]] },
        {
            kind: 'list',
            ordered: false,
            items: [
                [
                    {
                        kind: 'emphasis',
                        children: [
                            { kind: 'link', href: value, children: [{ kind: 'text', value }] },
                        ],
                    },
                ],
            ],
        },
        { kind: 'rule' },
        { kind: 'paragraph', children: [{ kind: 'text', value: 'mutable suffix' }] },
    ];
    const original = JSON.stringify(blocks);
    const identities = [...blocks];
    retention.detachMarkdownBlocks(blocks, 5);
    expect(JSON.stringify(blocks)).toBe(original);
    blocks.forEach((block, index) => expect(block).toBe(identities[index]));
});

it('copies only newly finalized blocks once across appends and unchanged renders', () => {
    const detach = vi.spyOn(retention, 'detachMarkdownBlocks');
    const parse = createIncrementalMarkdownParser();
    let source = '';
    let first: MarkdownBlock | undefined;
    for (let index = 0; index < 500; index++) {
        source += `paragraph ${String(index)} with lone \ud800 and emoji \ud83d\ude42\n\n`;
        const output = parse(source);
        if (index === 0) first = output[0];
        expect(output[0]).toBe(first);
        expect(parse(source)).toBe(output);
    }
    expect(detach.mock.calls.reduce((sum, [, count]) => sum + count, 0)).toBe(500);
    expect(parse(source)).toEqual(parseMarkdown(source));
});

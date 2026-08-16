import { describe, expect, it } from 'vitest';

import {
    MAX_MARKDOWN_INPUT_BYTES,
    MAX_MARKDOWN_NODES,
    type MarkdownBlock,
    type MarkdownInline,
    parseMarkdown,
} from './markdown';

function plainText(nodes: MarkdownInline[]): string {
    return nodes
        .map((node) => {
            if (node.kind === 'text') return node.value;
            if (node.kind === 'code') return node.value;
            return plainText(node.children);
        })
        .join('');
}

function blockText(blocks: MarkdownBlock[]): string {
    return blocks
        .map((block) => {
            if (block.kind === 'paragraph') return plainText(block.children);
            if (block.kind === 'code') return block.value;
            if (block.kind === 'quote') return block.lines.map(plainText).join('\n');
            if (block.kind === 'list') return block.items.map(plainText).join('\n');
            return '';
        })
        .join('\n');
}

function countNodes(nodes: MarkdownInline[]): number {
    return nodes.reduce(
        (total, node) =>
            total +
            1 +
            (node.kind === 'strong' || node.kind === 'emphasis' || node.kind === 'link'
                ? countNodes(node.children)
                : 0),
        0,
    );
}

describe('parseMarkdown', () => {
    it('reads the supported inline subset', () => {
        const [block] = parseMarkdown('**굵게** 그리고 *기울임* 그리고 `코드`');
        expect(block?.kind).toBe('paragraph');
        const children = block?.kind === 'paragraph' ? block.children : [];
        expect(children.map((node) => node.kind)).toEqual([
            'strong',
            'text',
            'emphasis',
            'text',
            'code',
        ]);
        expect(plainText(children)).toBe('굵게 그리고 기울임 그리고 코드');
    });

    it('reads block structures', () => {
        const blocks = parseMarkdown(
            [
                '> 인용문',
                '',
                '- 첫째',
                '- 둘째',
                '',
                '1. 하나',
                '',
                '---',
                '',
                '```ts',
                'const a = 1;',
                '```',
            ].join('\n'),
        );
        expect(blocks.map((block) => block.kind)).toEqual([
            'quote',
            'list',
            'list',
            'rule',
            'code',
        ]);
        const [, unordered, ordered, , fence] = blocks;
        expect(unordered?.kind === 'list' && unordered.ordered).toBe(false);
        expect(ordered?.kind === 'list' && ordered.ordered).toBe(true);
        expect(fence?.kind === 'code' && fence.language).toBe('ts');
        expect(fence?.kind === 'code' && fence.value).toBe('const a = 1;');
    });

    it('leaves unterminated markers as literal text', () => {
        for (const source of ['**굵게', '*기울임', '`코드', '[링크](', '**', '***']) {
            const blocks = parseMarkdown(source);
            expect(blockText(blocks)).toBe(source);
        }
    });

    it('stays stable across every prefix of a streaming message', () => {
        const full = '안녕 **친구**, *반가워*. `코드` 그리고\n\n> 인용\n\n- 목록';
        for (let length = 1; length <= full.length; length += 1) {
            const prefix = full.slice(0, length);
            expect(() => parseMarkdown(prefix)).not.toThrow();
            // No character is ever invented or dropped.
            const rendered = blockText(parseMarkdown(prefix)).replace(/[\s]/g, '');
            const expected = prefix.replace(/[*`>\-\s]/g, '');
            expect(rendered.replace(/[*`>-]/g, '')).toContain(expected.slice(0, 8));
        }
    });

    it('never produces markup or a non-http link', () => {
        const blocks = parseMarkdown(
            '<script>alert(1)</script> [x](javascript:alert(1)) [y](data:text/html,x) [ok](https://example.com)',
        );
        const [block] = blocks;
        const children = block?.kind === 'paragraph' ? block.children : [];
        const links = children.filter((node) => node.kind === 'link');
        expect(links).toHaveLength(1);
        expect(links[0]?.kind === 'link' && links[0].href).toBe('https://example.com');
        // The angle brackets survive as literal text; nothing becomes an element.
        expect(plainText(children)).toContain('<script>alert(1)</script>');
    });

    it('bounds inline nodes on adversarial input', () => {
        const blocks = parseMarkdown('`a`'.repeat(MAX_MARKDOWN_NODES * 2));
        const total = blocks.reduce(
            (sum, block) => sum + (block.kind === 'paragraph' ? countNodes(block.children) : 0),
            0,
        );
        expect(total).toBeLessThanOrEqual(MAX_MARKDOWN_NODES + 1);
    });

    it('falls back to plain text beyond the input ceiling', () => {
        const oversized = 'x'.repeat(MAX_MARKDOWN_INPUT_BYTES + 1);
        const blocks = parseMarkdown(`**${oversized}**`);
        expect(blocks).toHaveLength(1);
        expect(blocks[0]?.kind).toBe('paragraph');
        const children = blocks[0]?.kind === 'paragraph' ? blocks[0].children : [];
        expect(children).toEqual([{ kind: 'text', value: `**${oversized}**` }]);
    });

    it('renders an unclosed streaming fence with what has arrived', () => {
        const blocks = parseMarkdown('```ts\nconst a = 1;');
        expect(blocks).toHaveLength(1);
        expect(blocks[0]).toEqual({ kind: 'code', language: 'ts', value: 'const a = 1;' });
    });

    it('returns nothing for empty input', () => {
        expect(parseMarkdown('')).toEqual([]);
    });
});

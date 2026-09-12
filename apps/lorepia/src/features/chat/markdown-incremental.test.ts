import { afterEach, expect, it, vi } from 'vitest';
import * as markdown from './markdown';
import { createIncrementalMarkdownParser } from './markdown-incremental';
afterEach(() => vi.restoreAllMocks());

it.each([
    'a\n\n**bold *nested* text** [label](https://example.com) `code`\n\n> quote\n> **next**\n\n- item\n- next',
    'para\n---not a rule\n\n~~~ts\ncode\n\ninside fence\n~~~\n\nnext',
    'paragraph\n```js\n**literal**\n```\nnot separated\n\n***combined***',
    '\ud83d\ude42\uD55C\uAE00\n\n[bad](javascript:alert) _open\nclose_\n\n',
])('matches full parsing at every UTF16 prefix of mutable syntax: %s', (source) => {
    const parse = createIncrementalMarkdownParser();
    for (let i = 0; i <= source.length; i++)
        expect(parse(source.slice(0, i))).toEqual(markdown.parseMarkdown(source.slice(0, i)));
});
it('resets after truncation, edits, unrelated messages and byte-limit transitions', () => {
    const parse = createIncrementalMarkdownParser();
    const edge = 'x'.repeat(markdown.MAX_MARKDOWN_INPUT_BYTES - 4);
    const sources = [
        'first\n\nsecond',
        'first\n\nsecond appended',
        'first',
        'replacement\n\n- list',
        edge,
        edge + '\ud83d',
        edge + '\ud83d\ude42',
        edge + '\ud83d\ude42x',
        edge + '\ud83d\ude42xy',
        'small',
        '',
    ];
    for (const source of sources) expect(parse(source)).toEqual(markdown.parseMarkdown(source));
});
it('preserves full-parser budget and 512-line grouping at streaming boundaries', () => {
    for (const source of [
        'a\n\n'.repeat(2500),
        '- **a**\n'.repeat(700),
        '```\n' + 'line\n'.repeat(514) + '```\n\nend',
    ]) {
        const parse = createIncrementalMarkdownParser();
        for (let length = 0; length < source.length; length += 61)
            expect(parse(source.slice(0, length))).toEqual(
                markdown.parseMarkdown(source.slice(0, length)),
            );
        expect(parse(source)).toEqual(markdown.parseMarkdown(source));
    }
});
it('reuses finalized blocks and scans only appended bytes and mutable suffixes', () => {
    const scan = vi.spyOn(markdown, 'markdownByteLength');
    const segment = vi.spyOn(markdown, 'parseMarkdownSegment');
    const parse = createIncrementalMarkdownParser();
    const first = parse('a\n\n'.repeat(3));
    let latest = first;
    for (let i = 2; i <= 500; i++) latest = parse('a\n\n'.repeat(i * 3));
    expect(latest[0]).toBe(first[0]);
    expect(segment.mock.calls.reduce((n, [source]) => n + source.length, 0)).toBe(4500);
    expect(scan.mock.calls.reduce((n, [source]) => n + source.length, 0)).toBe(4500);
    const calls = segment.mock.calls.length;
    expect(parse('a\n\n'.repeat(1500))).toBe(latest);
    expect(segment).toHaveBeenCalledTimes(calls);
});

it('differentially checks deterministic mixed malformed syntax and partial Unicode', () => {
    const tokens = [
        'a',
        '\n',
        '\n\n',
        ' ',
        '```',
        '~~~js',
        '*',
        '**',
        '_',
        '`x`',
        '[x](',
        'https://x)',
        '- a',
        '> q',
        '1. n',
        '---',
        '\ud83d',
        '\ude42',
    ];
    let seed = 17;
    for (let run = 0; run < 64; run++) {
        let source = '';
        for (let token = 0; token < 30; token++) {
            seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
            source += tokens[seed % tokens.length] ?? '';
        }
        const parse = createIncrementalMarkdownParser();
        for (let length = 0; length <= source.length; length++) {
            const prefix = source.slice(0, length);
            expect(parse(prefix)).toEqual(markdown.parseMarkdown(prefix));
        }
    }
});

it('reconsiders mutable syntax at the remaining-budget boundary instead of freezing fallback', () => {
    const stable = '---\n\n'.repeat(markdown.MAX_MARKDOWN_NODES - 3);
    const parse = createIncrementalMarkdownParser();
    for (const tail of ['', '-', '--', '---', '---\n', '---\n\n', '---\n\na']) {
        expect(parse(stable + tail)).toEqual(markdown.parseMarkdown(stable + tail));
    }
});

it('stops constructing historical blocks after exhausting the budget during a 60KB stream', () => {
    const segment = vi.spyOn(markdown, 'parseMarkdownSegment');
    const scan = vi.spyOn(markdown, 'markdownByteLength');
    const parse = createIncrementalMarkdownParser();
    const source = 'a\n\n'.repeat(20000);
    let output: markdown.MarkdownBlock[] = [];
    for (let step = 1; step <= 500; step++) output = parse(source.slice(0, step * 120));
    const created = segment.mock.results.reduce((total, result) => {
        if (result.type !== 'return') throw new Error('segment failed');
        return total + result.value.blocks.length;
    }, 0);
    expect(created).toBeLessThanOrEqual(markdown.MAX_MARKDOWN_NODES / 2 + 500);
    expect(scan.mock.calls.reduce((total, [value]) => total + value.length, 0)).toBe(60000);
    expect(output).toEqual(markdown.parseMarkdown(source));
});

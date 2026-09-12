/**
 * Bounded markdown reader for assistant and user message text.
 *
 * The reader never produces an HTML string. It returns a token tree that
 * `MarkdownText.svelte` renders through ordinary Svelte elements, so message
 * text can never introduce markup, script, or navigation. Every function here
 * is total: malformed or partial input degrades to literal text instead of
 * throwing, because the streaming view must stay stable while a marker is still unterminated.
 */

/** Matches the backend generation ceiling; larger input renders as plain text. */
export const MAX_MARKDOWN_INPUT_BYTES = 256 * 1024;
/** Whole render-tree budget, including two reserved literal fallback units. */
export const MAX_MARKDOWN_NODES = 4096;
/** Maximum list items or block-quote lines grouped into one block. */
export const MAX_MARKDOWN_BLOCK_LINES = 512;

export type MarkdownInline =
    | { kind: 'text'; value: string }
    | { kind: 'strong'; children: MarkdownInline[] }
    | { kind: 'emphasis'; children: MarkdownInline[] }
    | { kind: 'code'; value: string }
    | { kind: 'link'; href: string; children: MarkdownInline[] };

export type MarkdownBlock =
    | { kind: 'paragraph'; children: MarkdownInline[] }
    | { kind: 'quote'; lines: MarkdownInline[][] }
    | { kind: 'code'; language: string | null; value: string }
    | { kind: 'list'; ordered: boolean; items: MarkdownInline[][] }
    | { kind: 'rule' };

const UNORDERED_ITEM = /^\s{0,3}[-*+]\s+(.*)$/;
const ORDERED_ITEM = /^\s{0,3}\d{1,9}[.)]\s+(.*)$/;
const QUOTE_LINE = /^\s{0,3}>\s?(.*)$/;
// Only `---` is a rule. CommonMark also accepts `***` and `___`, but those
// collide with emphasis markers: while a message streams in, `***bold***`
// would flash as a horizontal rule before its text arrives.
const RULE_LINE = /^\s{0,3}-{3,}\s*$/;
const FENCE_LINE = /^\s{0,3}(?:```|~~~)\s*([A-Za-z0-9_+-]{0,32})\s*$/;
const LINK = /^\[([^\]\n]{0,512})\]\(([^\s)]{1,2048})\)/;

/** Only these schemes are recognized; anything else stays literal text. */
const ALLOWED_LINK_SCHEMES = ['https://', 'http://'];

export function markdownByteLength(value: string): number {
    // Avoids allocating a TextEncoder for every streaming delta.
    let bytes = 0;
    for (const character of value) {
        const code = character.codePointAt(0) ?? 0;
        if (code < 0x80) bytes += 1;
        else if (code < 0x800) bytes += 2;
        else if (code < 0x10000) bytes += 3;
        else bytes += 4;
    }
    return bytes;
}

const BUDGET_EXHAUSTED = new Error('markdown render budget exhausted');

class NodeBudget {
    constructor(public remaining = MAX_MARKDOWN_NODES - 2) {}
    take(count = 1): void {
        if (this.remaining < count) throw BUDGET_EXHAUSTED;
        this.remaining -= count;
    }
}

function pushText(nodes: MarkdownInline[], value: string, budget: NodeBudget): void {
    if (value === '') return;
    const last = nodes.at(-1);
    if (last?.kind === 'text') {
        last.value += value;
        return;
    }
    budget.take();
    nodes.push({ kind: 'text', value });
}

function isAllowedHref(href: string): boolean {
    const lowered = href.toLowerCase();
    return ALLOWED_LINK_SCHEMES.some((scheme) => lowered.startsWith(scheme));
}

/**
 * Parses one line of inline markup.
 *
 * `depth` bounds emphasis nesting. When the budget or depth runs out the rest
 * of the line is appended verbatim, which keeps output length proportional to
 * input regardless of how adversarial the markers are.
 */
function parseInline(source: string, budget: NodeBudget, depth = 0): MarkdownInline[] {
    const nodes: MarkdownInline[] = [];
    let index = 0;
    let literalStart = 0;

    const flushLiteral = (end: number): void => {
        pushText(nodes, source.slice(literalStart, end), budget);
        literalStart = end;
    };

    while (index < source.length) {
        const character = source[index];
        if (character === undefined) break;

        if (character === '`') {
            const close = source.indexOf('`', index + 1);
            if (close > index) {
                budget.take(2);
                flushLiteral(index);
                nodes.push({ kind: 'code', value: source.slice(index + 1, close) });
                index = close + 1;
                literalStart = index;
                continue;
            }
        }

        if (character === '[') {
            const match = LINK.exec(source.slice(index));
            const consumed = match?.[0];
            const label = match?.[1];
            const href = match?.[2];
            if (
                consumed !== undefined &&
                label !== undefined &&
                href !== undefined &&
                isAllowedHref(href)
            ) {
                budget.take();
                if (depth >= 4) budget.take();
                flushLiteral(index);
                nodes.push({
                    kind: 'link',
                    href,
                    children:
                        depth >= 4
                            ? [{ kind: 'text', value: label }]
                            : parseInline(label, budget, depth + 1),
                });
                index += consumed.length;
                literalStart = index;
                continue;
            }
        }

        if (character === '*' || character === '_') {
            const strong = source.startsWith(character.repeat(2), index);
            const marker = strong ? character.repeat(2) : character;
            const close = source.indexOf(marker, index + marker.length);
            const inner = close > index ? source.slice(index + marker.length, close) : '';
            // An empty span is not emphasis; leaving it literal keeps `**` and
            // stray asterisks readable, which matters while text is streaming.
            if (close > index && inner.trim() !== '' && depth < 4) {
                budget.take();
                flushLiteral(index);
                nodes.push({
                    kind: strong ? 'strong' : 'emphasis',
                    children: parseInline(inner, budget, depth + 1),
                });
                index = close + marker.length;
                literalStart = index;
                continue;
            }
        }

        index += 1;
    }

    flushLiteral(source.length);
    return nodes;
}

/** Internal stable-prefix checkpoint for one mounted streaming message. */
export interface MarkdownCheckpoint {
    offset: number;
    count: number;
    remaining: number;
}
export interface MarkdownParseResult {
    blocks: MarkdownBlock[];
    settled: MarkdownCheckpoint | null;
}
export function markdownLiteral(source: string): MarkdownBlock[] {
    return source === ''
        ? []
        : [{ kind: 'paragraph', children: [{ kind: 'text', value: source }] }];
}

/** Reads a markdown message into a render tree. */
export function parseMarkdown(source: string): MarkdownBlock[] {
    if (markdownByteLength(source) > MAX_MARKDOWN_INPUT_BYTES) return markdownLiteral(source);
    return parseMarkdownSegment(source).blocks;
}

/** Internal suffix parser; its caller owns the whole-input byte ceiling. */
export function parseMarkdownSegment(
    source: string,
    remaining = MAX_MARKDOWN_NODES - 2,
): MarkdownParseResult {
    const budget = new NodeBudget(remaining);
    const blocks: MarkdownBlock[] = [];
    let settled: MarkdownCheckpoint | null = null;
    if (source === '') return { blocks, settled };
    if (remaining === 0) return { blocks: markdownLiteral(source), settled };
    const lines = source.split('\n');
    let index = 0;
    let offset = 0;
    const advance = () => {
        offset = Math.min(source.length, offset + (lines[index]?.length ?? 0) + 1);
        index += 1;
    };
    while (index < lines.length) {
        const line = lines[index] ?? '';
        // A complete blank line outside a fence cannot be reinterpreted by append.
        if (line.trim() === '' && index < lines.length - 1) {
            settled = { offset, count: blocks.length, remaining: budget.remaining };
        }
        if (budget.remaining === 0) {
            blocks.push(...markdownLiteral(source.slice(offset)));
            break;
        }
        if (line.trim() === '') {
            advance();
            if (index < lines.length)
                settled = { offset, count: blocks.length, remaining: budget.remaining };
            continue;
        }
        const blockStart = offset;
        try {
            const fence = FENCE_LINE.exec(line);
            if (fence !== null) {
                budget.take(3); // pre, code, text
                const marker = line.trimStart().slice(0, 3);
                const body: string[] = [];
                advance();
                while (
                    index < lines.length &&
                    !(lines[index] ?? '').trimStart().startsWith(marker)
                ) {
                    body.push(lines[index] ?? '');
                    advance();
                }
                if (index < lines.length) advance(); // An actual closing fence only.
                blocks.push({
                    kind: 'code',
                    language: fence[1] === undefined || fence[1] === '' ? null : fence[1],
                    value: body.join('\n'),
                });
                continue;
            }
            budget.take(); // Block element, including paragraph/list/quote/rule.
            if (RULE_LINE.test(line)) {
                blocks.push({ kind: 'rule' });
                advance();
                continue;
            }
            if (QUOTE_LINE.test(line)) {
                const quoted: MarkdownInline[][] = [];
                while (index < lines.length && quoted.length < MAX_MARKDOWN_BLOCK_LINES) {
                    const value = QUOTE_LINE.exec(lines[index] ?? '')?.[1];
                    if (value === undefined) break;
                    budget.take(); // Quote line paragraph.
                    quoted.push(parseInline(value, budget));
                    advance();
                }
                blocks.push({ kind: 'quote', lines: quoted });
                continue;
            }
            const ordered = ORDERED_ITEM.test(line);
            if (ordered || UNORDERED_ITEM.test(line)) {
                const pattern = ordered ? ORDERED_ITEM : UNORDERED_ITEM;
                const items: MarkdownInline[][] = [];
                while (index < lines.length && items.length < MAX_MARKDOWN_BLOCK_LINES) {
                    const item = pattern.exec(lines[index] ?? '')?.[1];
                    if (item === undefined) break;
                    budget.take(); // List item element.
                    items.push(parseInline(item, budget));
                    advance();
                }
                blocks.push({ kind: 'list', ordered, items });
                continue;
            }
            const paragraph: string[] = [];
            while (index < lines.length && paragraph.length < MAX_MARKDOWN_BLOCK_LINES) {
                const current = lines[index] ?? '';
                if (
                    current.trim() === '' ||
                    FENCE_LINE.test(current) ||
                    RULE_LINE.test(current) ||
                    QUOTE_LINE.test(current) ||
                    ORDERED_ITEM.test(current) ||
                    UNORDERED_ITEM.test(current)
                )
                    break;
                paragraph.push(current);
                advance();
            }
            blocks.push({ kind: 'paragraph', children: parseInline(paragraph.join('\n'), budget) });
        } catch (error) {
            if (error !== BUDGET_EXHAUSTED) throw error;
            // Discard the incomplete block, preserving its exact source plus the suffix.
            blocks.push(...markdownLiteral(source.slice(blockStart)));
            break;
        }
    }
    return { blocks, settled };
}

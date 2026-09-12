import { detachMarkdownBlocks } from './markdown-retention';
import {
    MAX_MARKDOWN_INPUT_BYTES,
    MAX_MARKDOWN_NODES,
    markdownByteLength,
    markdownLiteral,
    parseMarkdownSegment,
    type MarkdownBlock,
} from './markdown';

/** One latest input and one grammar-proven prefix; no cross-message cache. */
export function createIncrementalMarkdownParser(): (source: string) => MarkdownBlock[] {
    let previous = '';
    let bytes = 0;
    let prefix: MarkdownBlock[] = [];
    let offset = 0;
    let remaining = MAX_MARKDOWN_NODES - 2;
    let output: MarkdownBlock[] = [];
    return (source) => {
        if (source === previous) return output;
        if (source.startsWith(previous)) {
            bytes += markdownByteLength(source.slice(previous.length));
            // Two separately received UTF16 surrogates become one four-byte scalar.
            const before = previous.charCodeAt(previous.length - 1);
            const after = source.charCodeAt(previous.length);
            if (before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff)
                bytes -= 2;
        } else {
            bytes = markdownByteLength(source);
            prefix = [];
            offset = 0;
            remaining = MAX_MARKDOWN_NODES - 2;
        }
        previous = source;
        if (bytes > MAX_MARKDOWN_INPUT_BYTES) {
            prefix = [];
            offset = 0;
            remaining = MAX_MARKDOWN_NODES - 2;
            output = markdownLiteral(source);
            return output;
        }
        const parsed = parseMarkdownSegment(source.slice(offset), remaining);
        output = [...prefix, ...parsed.blocks];
        if (parsed.settled !== null) {
            detachMarkdownBlocks(parsed.blocks, parsed.settled.count);
            prefix = [...prefix, ...parsed.blocks.slice(0, parsed.settled.count)];
            offset += parsed.settled.offset;
            remaining = parsed.settled.remaining;
        }
        return output;
    };
}

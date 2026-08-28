const MAX_PATTERN_CHARS = 4_096;
const MAX_SOURCE_CHARS = 262_144;
const MAX_REPLACEMENT_CHARS = 262_144;

export type PortableRegexRequest =
    | {
          operation: 'test';
          source: string;
          pattern: string;
          flags: string;
      }
    | {
          operation: 'replace';
          source: string;
          pattern: string;
          flags: string;
          replacement: string;
      };

export type PortableRegexValue = string | boolean;

export type PortableRegexWorkerResult = { ok: true; value: PortableRegexValue } | { ok: false };

export function performPortableRegexOperation(
    request: PortableRegexRequest,
): PortableRegexWorkerResult {
    if (
        request.pattern.length > MAX_PATTERN_CHARS ||
        request.source.length > MAX_SOURCE_CHARS ||
        (request.operation === 'replace' && request.replacement.length > MAX_REPLACEMENT_CHARS)
    ) {
        return { ok: false };
    }
    try {
        const expression = new RegExp(request.pattern, safePortableRegexFlags(request.flags));
        if (request.operation === 'test') {
            return { ok: true, value: expression.test(request.source) };
        }
        const value = request.source.replace(expression, request.replacement);
        return value.length <= MAX_SOURCE_CHARS ? { ok: true, value } : { ok: false };
    } catch {
        return { ok: false };
    }
}

export function safePortableRegexFlags(flags: string): string {
    return [...new Set(flags.split('').filter((flag) => 'dgimsuvy'.includes(flag)))].join('');
}

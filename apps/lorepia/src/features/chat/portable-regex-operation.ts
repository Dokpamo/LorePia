const MAX_PORTABLE_REGEX_PATTERN_CHARS = 4_096;
const MAX_PORTABLE_REGEX_SOURCE_CHARS = 262_144;
const MAX_PORTABLE_REGEX_REPLACEMENT_CHARS = 262_144;

type PortableRegexRequest =
    | { operation: 'compile'; pattern: string; flags: string }
    | { operation: 'test'; source: string; pattern: string; flags: string }
    | {
          operation: 'replace';
          source: string;
          pattern: string;
          flags: string;
          replacement: string;
      };

type PortableRegexWorkerResult =
    | { ok: true; value: string | boolean }
    | {
          ok: false;
          reason: 'invalid_request' | 'invalid_pattern' | 'input_limit' | 'output_limit';
      };

export function performPortableRegexOperation(
    request: PortableRegexRequest,
): PortableRegexWorkerResult {
    if (
        request.pattern.length > MAX_PORTABLE_REGEX_PATTERN_CHARS ||
        (request.operation !== 'compile' &&
            request.source.length > MAX_PORTABLE_REGEX_SOURCE_CHARS) ||
        (request.operation === 'replace' &&
            request.replacement.length > MAX_PORTABLE_REGEX_REPLACEMENT_CHARS)
    ) {
        return { ok: false, reason: 'input_limit' };
    }
    try {
        const expression = new RegExp(request.pattern, safePortableRegexFlags(request.flags));
        if (request.operation === 'compile') return { ok: true, value: true };
        if (request.operation === 'test') {
            return { ok: true, value: expression.test(request.source) };
        }
        const value = request.source.replace(expression, request.replacement);
        return value.length <= MAX_PORTABLE_REGEX_SOURCE_CHARS
            ? { ok: true, value }
            : { ok: false, reason: 'output_limit' };
    } catch {
        return { ok: false, reason: 'invalid_pattern' };
    }
}

export function safePortableRegexFlags(flags: string): string {
    return [...new Set(flags.split('').filter((flag) => 'dgimsuvy'.includes(flag)))].join('');
}

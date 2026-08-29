export const MAX_PORTABLE_REGEX_PATTERN_CHARS = 4_096;
export const MAX_PORTABLE_REGEX_SOURCE_CHARS = 262_144;
export const MAX_PORTABLE_REGEX_REPLACEMENT_CHARS = 262_144;

export type PortableRegexRequest =
    | {
          operation: 'compile';
          pattern: string;
          flags: string;
      }
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

export type PortableRegexWorkerFailureReason =
    'invalid_request' | 'invalid_pattern' | 'input_limit' | 'output_limit';

export type PortableRegexWorkerResult =
    | { ok: true; value: PortableRegexValue }
    | { ok: false; reason: PortableRegexWorkerFailureReason };

export interface PortableRegexWorkerRequest {
    id: string;
    request: PortableRegexRequest;
}

export interface PortableRegexWorkerResponse {
    id: string;
    result: PortableRegexWorkerResult;
}

export function isPortableRegexWorkerResponse(
    value: unknown,
): value is PortableRegexWorkerResponse {
    if (!isRecord(value) || !validIdentifier(value.id) || !isRecord(value.result)) return false;
    if (value.result.ok === true) {
        return typeof value.result.value === 'string' || typeof value.result.value === 'boolean';
    }
    return (
        value.result.ok === false &&
        ['invalid_request', 'invalid_pattern', 'input_limit', 'output_limit'].includes(
            String(value.result.reason),
        )
    );
}

export function isPortableRegexWorkerRequest(value: unknown): value is PortableRegexWorkerRequest {
    return (
        isRecord(value) &&
        validIdentifier(value.id) &&
        isRecord(value.request) &&
        isPortableRegexRequest(value.request)
    );
}

function isPortableRegexRequest(value: Record<string, unknown>): value is PortableRegexRequest {
    if (
        !['compile', 'test', 'replace'].includes(String(value.operation)) ||
        typeof value.pattern !== 'string' ||
        typeof value.flags !== 'string'
    ) {
        return false;
    }
    if (value.operation === 'compile') return value.source === undefined;
    if (typeof value.source !== 'string') return false;
    return value.operation !== 'replace' || typeof value.replacement === 'string';
}

function validIdentifier(value: unknown): value is string {
    return typeof value === 'string' && value.length > 0 && value.length <= 128;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}

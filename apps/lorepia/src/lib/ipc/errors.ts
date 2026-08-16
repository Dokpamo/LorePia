import type { ShellErrorDto } from './contracts';

const DEFAULT_MESSAGE_KEY = 'error.unexpected';

export class LorepiaClientError extends Error {
    readonly code: string;
    readonly messageKey: string;
    readonly recoverable: boolean;
    readonly operationId: string | null;
    readonly fieldErrors: readonly { field: string; messageKey: string }[];

    constructor(dto: ShellErrorDto) {
        super(dto.message_key || DEFAULT_MESSAGE_KEY);
        this.name = 'LorepiaClientError';
        this.code = dto.code;
        this.messageKey = dto.message_key || DEFAULT_MESSAGE_KEY;
        this.recoverable = dto.recoverable;
        this.operationId = dto.operation_id;
        this.fieldErrors = dto.field_errors.map((error) => ({
            field: error.field,
            messageKey: error.message_key,
        }));
    }
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null;
}

function stringValue(record: Record<string, unknown>, key: string): string | null {
    const value = record[key];
    return typeof value === 'string' ? value : null;
}

export function normalizeClientError(error: unknown): LorepiaClientError {
    if (error instanceof LorepiaClientError) {
        return error;
    }

    if (isRecord(error)) {
        const nested = isRecord(error.error) ? error.error : error;
        const fieldErrors = Array.isArray(nested.field_errors)
            ? nested.field_errors.flatMap((value) => {
                  if (!isRecord(value)) {
                      return [];
                  }
                  const field = stringValue(value, 'field');
                  const messageKey = stringValue(value, 'message_key');
                  return field !== null && messageKey !== null
                      ? [{ field, message_key: messageKey }]
                      : [];
              })
            : [];

        return new LorepiaClientError({
            code: stringValue(nested, 'code') ?? 'unexpected',
            message_key: stringValue(nested, 'message_key') ?? DEFAULT_MESSAGE_KEY,
            recoverable: nested.recoverable === true,
            operation_id: stringValue(nested, 'operation_id'),
            field_errors: fieldErrors,
        });
    }

    return new LorepiaClientError({
        code: 'unexpected',
        message_key: DEFAULT_MESSAGE_KEY,
        recoverable: false,
        operation_id: null,
        field_errors: [],
    });
}

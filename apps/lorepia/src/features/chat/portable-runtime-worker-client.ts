import {
    MAX_PORTABLE_RUNTIME_WORKER_MESSAGE_BYTES,
    isPortableRuntimeWorkerMessage,
    portableRuntimeMessageByteLength,
    portableRuntimeMessageWithinLimit,
    type PortableRuntimeHostCallMessage,
    type PortableRuntimeHostResultMessage,
    type PortableRuntimeMainMessage,
    type PortableRuntimePersistedState,
    type PortableRuntimeWorkerMessage,
    type PortableRuntimeWorkerOperation,
    type PortableRuntimeWorkerResult,
    type PortableRuntimeWorkerSnapshot,
} from './portable-runtime-protocol';

const MAX_INBOUND_MESSAGES_PER_REQUEST = 64;
const MAX_INBOUND_BYTES_PER_REQUEST = 12 * 1024 * 1024;

export type PortableRuntimeWorkerEndpoint = Pick<
    Worker,
    'addEventListener' | 'removeEventListener' | 'postMessage' | 'terminate'
>;

export type PortableRuntimeWorkerFactory = () => PortableRuntimeWorkerEndpoint;

export interface PortableRuntimeWorkerClientHandlers {
    onHostCall: (call: PortableRuntimeHostCallMessage) => Promise<unknown>;
    onState: (persisted: PortableRuntimePersistedState) => void;
    onChanged: () => void;
    onNotice: (message: string, error: boolean) => void;
}

export class PortableRuntimeWorkerError extends Error {
    constructor(
        readonly code:
            'execution-timeout' | 'runtime-error' | 'protocol-error' | 'worker-terminated',
        message: string,
    ) {
        super(message);
        this.name = 'PortableRuntimeWorkerError';
    }
}

export class PortableRuntimeWorkerClient {
    private readonly worker: PortableRuntimeWorkerEndpoint;
    private readonly handlers: PortableRuntimeWorkerClientHandlers;
    private readonly pending = new Map<
        string,
        {
            resolve: (value: {
                result: PortableRuntimeWorkerResult;
                snapshot: PortableRuntimeWorkerSnapshot;
            }) => void;
            reject: (error: Error) => void;
        }
    >();
    private nextRequestId = 1;
    private inboundMessages = 0;
    private inboundBytes = 0;
    private closed = false;

    private readonly handleMessage = (event: MessageEvent<unknown>): void => {
        if (!this.beginInboundMessage()) {
            this.close(
                new PortableRuntimeWorkerError(
                    'protocol-error',
                    'portable runtime worker exceeded its message rate limit',
                ),
            );
            return;
        }
        const message = event.data;
        const byteLength = portableRuntimeMessageByteLength(message);
        if (
            byteLength === null ||
            byteLength > MAX_PORTABLE_RUNTIME_WORKER_MESSAGE_BYTES ||
            !this.reserveInboundBytes(byteLength) ||
            !isPortableRuntimeWorkerMessage(message)
        ) {
            this.close(
                new PortableRuntimeWorkerError(
                    'protocol-error',
                    'portable runtime worker returned an invalid message',
                ),
            );
            return;
        }
        this.routeMessage(message);
    };

    private readonly handleError = (): void => {
        this.close(
            new PortableRuntimeWorkerError(
                'worker-terminated',
                'portable runtime worker stopped unexpectedly',
            ),
        );
    };

    constructor(
        factory: PortableRuntimeWorkerFactory = createPortableRuntimeWorker,
        handlers: PortableRuntimeWorkerClientHandlers,
    ) {
        this.handlers = handlers;
        this.worker = factory();
        this.worker.addEventListener('message', this.handleMessage);
        this.worker.addEventListener('error', this.handleError);
        this.worker.addEventListener('messageerror', this.handleError);
    }

    request(operation: PortableRuntimeWorkerOperation): Promise<{
        result: PortableRuntimeWorkerResult;
        snapshot: PortableRuntimeWorkerSnapshot;
    }> {
        if (this.closed) {
            return Promise.reject(
                new PortableRuntimeWorkerError(
                    'worker-terminated',
                    'portable runtime worker is not available',
                ),
            );
        }
        const requestId = `runtime-${String(this.nextRequestId)}`;
        this.nextRequestId += 1;
        const message: PortableRuntimeMainMessage = {
            channel: 'lorepia-portable-runtime-v1',
            type: 'request',
            requestId,
            operation,
        };
        if (!portableRuntimeMessageWithinLimit(message)) {
            return Promise.reject(
                new PortableRuntimeWorkerError(
                    'protocol-error',
                    'portable runtime worker request exceeds the message limit',
                ),
            );
        }
        this.inboundMessages = 0;
        this.inboundBytes = 0;
        return new Promise((resolve, reject) => {
            this.pending.set(requestId, { resolve, reject });
            try {
                this.worker.postMessage(message);
            } catch (error) {
                this.pending.delete(requestId);
                reject(
                    new PortableRuntimeWorkerError(
                        'protocol-error',
                        error instanceof Error
                            ? error.message
                            : 'portable runtime worker request could not be sent',
                    ),
                );
            }
        });
    }

    close(
        reason: Error = new PortableRuntimeWorkerError(
            'worker-terminated',
            'portable runtime worker was closed',
        ),
    ): void {
        if (this.closed) return;
        this.closed = true;
        this.worker.removeEventListener('message', this.handleMessage);
        this.worker.removeEventListener('error', this.handleError);
        this.worker.removeEventListener('messageerror', this.handleError);
        this.worker.terminate();
        for (const pending of this.pending.values()) pending.reject(reason);
        this.pending.clear();
    }

    private routeMessage(message: PortableRuntimeWorkerMessage): void {
        if (message.type === 'response') {
            const pending = this.pending.get(message.requestId);
            if (pending === undefined) return;
            this.pending.delete(message.requestId);
            if (!message.ok || message.result === undefined || message.snapshot === undefined) {
                pending.reject(
                    new PortableRuntimeWorkerError(
                        message.error?.code ?? 'protocol-error',
                        message.error?.message ?? 'portable runtime worker request failed',
                    ),
                );
                return;
            }
            pending.resolve({ result: message.result, snapshot: message.snapshot });
            return;
        }
        if (message.type === 'host-call') {
            void this.answerHostCall(message);
            return;
        }
        if (message.type === 'state') {
            this.handlers.onState(message.persisted);
            return;
        }
        if (message.type === 'changed') {
            this.handlers.onChanged();
            return;
        }
        this.handlers.onNotice(message.message, message.error);
    }

    private async answerHostCall(call: PortableRuntimeHostCallMessage): Promise<void> {
        let message: PortableRuntimeHostResultMessage;
        try {
            const value = await this.handlers.onHostCall(call);
            message = {
                channel: 'lorepia-portable-runtime-v1',
                type: 'host-result',
                callId: call.callId,
                ok: true,
                value,
            };
        } catch (error) {
            message = {
                channel: 'lorepia-portable-runtime-v1',
                type: 'host-result',
                callId: call.callId,
                ok: false,
                error: error instanceof Error ? error.message : 'portable runtime host call failed',
            };
        }
        if (this.closed) return;
        if (!portableRuntimeMessageWithinLimit(message)) {
            this.close(
                new PortableRuntimeWorkerError(
                    'protocol-error',
                    'portable runtime host response exceeds the message limit',
                ),
            );
            return;
        }
        try {
            this.worker.postMessage(message);
        } catch {
            this.close(
                new PortableRuntimeWorkerError(
                    'protocol-error',
                    'portable runtime host response could not be sent',
                ),
            );
        }
    }

    private beginInboundMessage(): boolean {
        this.inboundMessages += 1;
        return this.inboundMessages <= MAX_INBOUND_MESSAGES_PER_REQUEST;
    }

    private reserveInboundBytes(byteLength: number): boolean {
        this.inboundBytes += byteLength;
        return this.inboundBytes <= MAX_INBOUND_BYTES_PER_REQUEST;
    }
}

function createPortableRuntimeWorker(): PortableRuntimeWorkerEndpoint {
    return new Worker(new URL('./portable-runtime.worker.ts', import.meta.url), {
        type: 'module',
        name: 'lorepia-portable-runtime',
    });
}

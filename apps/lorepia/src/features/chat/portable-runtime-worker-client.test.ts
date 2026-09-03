import { describe, expect, it, vi } from 'vitest';

import type {
    PortableRuntimeMainMessage,
    PortableRuntimePersistedState,
    PortableRuntimeWorkerMessage,
} from './portable-runtime-protocol';
import {
    PortableRuntimeWorkerClient,
    type PortableRuntimeWorkerEndpoint,
} from './portable-runtime-worker-client';

describe('portable runtime worker client', () => {
    it('normalizes reactive proxy payloads before crossing the Worker boundary', async () => {
        type ListenerType = 'message' | 'error' | 'messageerror';
        const listeners: Record<ListenerType, Set<EventListenerOrEventListenerObject>> = {
            message: new Set(),
            error: new Set(),
            messageerror: new Set(),
        };
        const persisted: PortableRuntimePersistedState = {
            options: {},
            chatVars: {},
            state: {},
            messageOverrides: {},
            background: '',
            auxiliarySelection: null,
        };
        const endpoint: PortableRuntimeWorkerEndpoint = {
            addEventListener: (
                type: string,
                listener: EventListenerOrEventListenerObject | null,
            ) => {
                if (listener !== null && type in listeners) {
                    listeners[type as ListenerType].add(listener);
                }
            },
            removeEventListener: (
                type: string,
                listener: EventListenerOrEventListenerObject | null,
            ) => {
                if (listener !== null && type in listeners) {
                    listeners[type as ListenerType].delete(listener);
                }
            },
            postMessage: (value: unknown) => {
                const request = structuredClone(value) as PortableRuntimeMainMessage;
                if (request.type !== 'request') return;
                queueMicrotask(() => {
                    const response: PortableRuntimeWorkerMessage = {
                        channel: 'lorepia-portable-runtime-v1',
                        type: 'response',
                        requestId: request.requestId,
                        ok: true,
                        result: { type: 'display', entries: [] },
                        snapshot: {
                            persisted,
                            virtualMessage: null,
                            stopped: false,
                        },
                    };
                    for (const listener of [...listeners.message]) {
                        const event = new MessageEvent('message', { data: response });
                        if (typeof listener === 'function') listener.call(endpoint, event);
                        else listener.handleEvent(event);
                    }
                });
            },
            terminate: vi.fn(),
        };
        const client = new PortableRuntimeWorkerClient(() => endpoint, {
            onHostCall: () => Promise.resolve(null),
            onState: vi.fn(),
            onChanged: vi.fn(),
            onNotice: vi.fn(),
        });
        const proxiedPersisted = new Proxy(persisted, {});

        const response = await client.request({
            type: 'refresh-display',
            context: {
                persisted: proxiedPersisted,
                messages: [],
                virtualMessage: null,
                activeLoreEntries: [],
                stopped: false,
            },
        });

        expect(response.result).toEqual({ type: 'display', entries: [] });
        client.close();
    });

    it('terminates a worker before an inbound event flood can drain on the renderer', () => {
        type ListenerType = 'message' | 'error' | 'messageerror';
        const listeners: Record<ListenerType, Set<EventListenerOrEventListenerObject>> = {
            message: new Set(),
            error: new Set(),
            messageerror: new Set(),
        };
        const terminate = vi.fn();
        const endpoint: PortableRuntimeWorkerEndpoint = {
            addEventListener: (
                type: string,
                listener: EventListenerOrEventListenerObject | null,
            ) => {
                if (listener !== null && type in listeners) {
                    listeners[type as ListenerType].add(listener);
                }
            },
            removeEventListener: (
                type: string,
                listener: EventListenerOrEventListenerObject | null,
            ) => {
                if (listener !== null && type in listeners) {
                    listeners[type as ListenerType].delete(listener);
                }
            },
            postMessage: vi.fn(),
            terminate,
        };
        const changed = vi.fn();
        const client = new PortableRuntimeWorkerClient(() => endpoint, {
            onHostCall: () => Promise.resolve(null),
            onState: vi.fn(),
            onChanged: changed,
            onNotice: vi.fn(),
        });
        const event: PortableRuntimeWorkerMessage = {
            channel: 'lorepia-portable-runtime-v1',
            type: 'changed',
        };

        for (let index = 0; index < 100; index += 1) {
            for (const listener of [...listeners.message]) {
                const messageEvent = new MessageEvent('message', { data: event });
                if (typeof listener === 'function') listener.call(endpoint, messageEvent);
                else listener.handleEvent(messageEvent);
            }
        }

        expect(terminate).toHaveBeenCalledOnce();
        expect(changed).toHaveBeenCalled();
        expect(changed.mock.calls.length).toBeLessThanOrEqual(64);
        client.close();
    });
});

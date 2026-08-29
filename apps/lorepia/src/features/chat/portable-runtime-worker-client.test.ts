import { describe, expect, it, vi } from 'vitest';

import type { PortableRuntimeWorkerMessage } from './portable-runtime-protocol';
import {
    PortableRuntimeWorkerClient,
    type PortableRuntimeWorkerEndpoint,
} from './portable-runtime-worker-client';

describe('portable runtime worker client', () => {
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

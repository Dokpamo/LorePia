/// <reference lib="webworker" />

import { PortableRuntimeKernel } from './portable-runtime-kernel';
import {
    isPortableRuntimeMainMessage,
    portableRuntimeMessageWithinLimit,
} from './portable-runtime-protocol';

const scope = globalThis as unknown as DedicatedWorkerGlobalScope;
const kernel = new PortableRuntimeKernel({
    postMessage: (message) => {
        if (!portableRuntimeMessageWithinLimit(message)) {
            throw new Error('portable runtime worker message exceeds its limit');
        }
        scope.postMessage(message);
    },
});

scope.addEventListener('message', (event: MessageEvent<unknown>) => {
    if (
        !portableRuntimeMessageWithinLimit(event.data) ||
        !isPortableRuntimeMainMessage(event.data)
    ) {
        return;
    }
    kernel.receive(event.data);
});

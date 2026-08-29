#!/usr/bin/env node

import assert from 'node:assert/strict';
import { Worker } from 'node:worker_threads';

const operationUrl = new URL(
    '../apps/lorepia/src/features/chat/portable-regex-operation.ts',
    import.meta.url,
).href;

function runInWorker(request, timeoutMs) {
    const source = `
        import { parentPort } from 'node:worker_threads';
        import { performPortableRegexOperation } from ${JSON.stringify(operationUrl)};
        parentPort.once('message', (request) => {
            parentPort.postMessage(performPortableRegexOperation(request));
        });
    `;
    const worker = new Worker(
        new URL(`data:text/javascript,${encodeURIComponent(source)}`),
        { type: 'module' },
    );
    return new Promise((resolve, reject) => {
        let settled = false;
        const finish = async (result) => {
            if (settled) return;
            settled = true;
            clearTimeout(timeout);
            await worker.terminate();
            resolve(result);
        };
        const timeout = setTimeout(
            () => void finish({ ok: false, timedOut: true }),
            timeoutMs,
        );
        worker.once('message', (result) => void finish(result));
        worker.once('error', reject);
        worker.postMessage(request);
    });
}

const compatible = await runInWorker(
    {
        operation: 'replace',
        source: 'prefix-item-item',
        pattern: '(?<=prefix-)(item)-\\1',
        flags: 'u',
        replacement: '$1-ok',
    },
    1_000,
);
assert.deepEqual(compatible, { ok: true, value: 'prefix-item-ok' });

let heartbeat = 0;
const heartbeatTimer = setInterval(() => {
    heartbeat += 1;
}, 5);
const catastrophic = await runInWorker(
    {
        operation: 'test',
        source: `${'a'.repeat(64_000)}!`,
        pattern: '(a+)+$',
        flags: '',
    },
    75,
);
clearInterval(heartbeatTimer);
assert.deepEqual(catastrophic, { ok: false, timedOut: true });
assert.ok(heartbeat >= 3, `main-thread heartbeat advanced only ${heartbeat} times`);

const recovered = await runInWorker(
    {
        operation: 'test',
        source: 'ordinary input',
        pattern: '^ordinary\\s+input$',
        flags: 'u',
    },
    1_000,
);
assert.deepEqual(recovered, { ok: true, value: true });

console.log('portable regex worker: PASS');

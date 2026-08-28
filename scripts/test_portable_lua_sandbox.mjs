#!/usr/bin/env node

import { readFile } from 'node:fs/promises';
import { Worker } from 'node:worker_threads';
import { fileURLToPath, pathToFileURL } from 'node:url';
import path from 'node:path';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const appRoot = path.join(repositoryRoot, 'apps', 'lorepia');
const hardening = await readFile(
    path.join(appRoot, 'src', 'features', 'chat', 'portable-runtime-sandbox.lua'),
    'utf8',
);
const runtimeSource = await readFile(
    path.join(appRoot, 'src', 'features', 'chat', 'portable-runtime.ts'),
    'utf8',
);
const asyncBridgeMatch = runtimeSource.match(
    /const LUA_ASYNC_BRIDGE = String\.raw`\n([\s\S]*?)\n`;/,
);
if (asyncBridgeMatch?.[1] === undefined) {
    throw new Error('portable Lua async bridge source was not found');
}

const workerSource = String.raw`
const { parentPort, workerData } = require('node:worker_threads');

(async () => {
    const { LuaFactory } = await import(workerData.wasmoonUrl);
    const nativePromiseThen = Promise.prototype.then;
    const isNativePromise = (value) => {
        if (value === null || (typeof value !== 'object' && typeof value !== 'function')) {
            return false;
        }
        try {
            Reflect.apply(nativePromiseThen, value, [() => undefined, () => undefined]);
            return true;
        } catch {
            return false;
        }
    };
    const factory = new LuaFactory(workerData.wasmPath);
    const engine = await factory.createEngine({
        injectObjects: true,
        enableProxy: false,
        traceAllocations: true,
        functionTimeout: 100,
    });
    engine.global.setMemoryMax(32 * 1024 * 1024);
    engine.global.set('__hostIsRuntimePromise', isNativePromise);
    engine.global.set('__hostRuntimeYield', () => new Promise((resolve) => setTimeout(resolve, 0)));
    engine.global.set('__probeResolvedPromise', () => Promise.resolve());

    async function run(source) {
        const thread = engine.global.newThread();
        const threadIndex = engine.global.getTop();
        try {
            thread.loadString(source);
            await thread.run(0, { timeout: 100 });
        } finally {
            engine.global.remove(threadIndex);
        }
    }

    await run(workerData.asyncBridge);
    await run(workerData.hardening);
    if (workerData.deadline) {
        await run('onStart = async(function() ' + workerData.source + ' end)');
        await Promise.race([
            engine.global.get('onStart')(),
            new Promise((_, reject) =>
                setTimeout(() => reject(new Error('event deadline')), 250),
            ),
        ]);
    } else if (workerData.callback) {
        await run('function __sandbox_probe() ' + workerData.source + ' end');
        await engine.global.get('__sandbox_probe')();
    } else {
        await run(workerData.source);
    }
    parentPort.postMessage({ status: 'completed' });
})().catch((error) => {
    parentPort.postMessage({ status: 'rejected', message: String(error) });
});
`;

const wasmoonUrl = pathToFileURL(
    path.join(appRoot, 'node_modules', 'wasmoon', 'dist', 'index.js'),
).href;
const wasmPath = path.join(appRoot, 'node_modules', 'wasmoon', 'dist', 'glue.wasm');

async function runProbe(
    name,
    source,
    { callback = false, deadline = false, expectedStatus = 'rejected', expectedPattern = /timeout/i } = {},
) {
    const worker = new Worker(workerSource, {
        eval: true,
        workerData: {
            callback,
            deadline,
            asyncBridge: asyncBridgeMatch[1],
            hardening,
            source,
            wasmPath,
            wasmoonUrl,
        },
    });
    let timer;
    const outcome = await new Promise((resolve) => {
        timer = setTimeout(() => resolve({ status: 'hung' }), 1_500);
        worker.once('message', resolve);
        worker.once('error', (error) =>
            resolve({ status: 'worker-error', message: String(error) }),
        );
        worker.once('exit', (code) => {
            if (code !== 0) resolve({ status: 'worker-exit', message: String(code) });
        });
    });
    clearTimeout(timer);
    await worker.terminate();
    if (
        outcome.status !== expectedStatus ||
        (expectedStatus === 'rejected' && !expectedPattern.test(outcome.message ?? ''))
    ) {
        throw new Error(`${name} violated the Lua sandbox contract: ${JSON.stringify(outcome)}`);
    }
}

await runProbe(
    'protected startup call',
    'while true do pcall(function() while true do end end) end',
);
await runProbe(
    'masked xpcall startup call',
    'while true do xpcall(function() while true do end end, function() return "masked" end) end',
);
await runProbe(
    'coroutine callback call',
    'while true do local co = coroutine.create(function() while true do end end); coroutine.resume(co) end',
    { callback: true },
);
await runProbe(
    'Promise catch starvation',
    'function spin() return Promise.resolve(nil):finally(function() while true do end end):catch(function() return spin() end) end; return spin()',
    { expectedPattern: /Promise|nil|index/i },
);
await runProbe(
    'public Promise callbacks',
    'local p = __probeResolvedPromise(); assert(p.next == nil); assert(p.catch == nil); assert(p.finally == nil)',
    { expectedStatus: 'completed' },
);
await runProbe(
    'resolved Promise await deadline',
    'local p = __probeResolvedPromise(); while true do p:await() end',
    { deadline: true, expectedPattern: /event deadline/i },
);
await runProbe(
    'Lua thenable assimilation',
    'local function repeat_then(resolve) resolve({ ["then"] = repeat_then }) end; coroutine.yield({ ["then"] = repeat_then })',
    { deadline: true, expectedStatus: 'completed' },
);
await runProbe(
    'Lua return thenable assimilation',
    'local function repeat_then(resolve) resolve({ ["then"] = repeat_then }) end; return { ["then"] = repeat_then }',
    { deadline: true, expectedStatus: 'completed' },
);
await runProbe(
    'forged Promise prototype chain',
    'local p = __probeResolvedPromise(); local function repeat_then(resolve) resolve({ ["then"] = repeat_then }) end; local function fake_finally() return __probeResolvedPromise() end; local proto = { ["__proto__"] = p, ["then"] = repeat_then, ["finally"] = fake_finally }; local fake = { ["__proto__"] = proto }; coroutine.yield(fake)',
    { deadline: true, expectedStatus: 'completed' },
);
await runProbe(
    'forged Promise await receiver',
    'local p = __probeResolvedPromise(); local await_method = p.await; local function repeat_then(resolve) resolve({ ["then"] = repeat_then }) end; await_method({ ["then"] = repeat_then })',
    { expectedPattern: /native runtime Promise/i },
);

console.log('portable Lua timeout sandbox regression: PASS');

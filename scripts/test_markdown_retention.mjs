#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { stripTypeScriptTypes } from "node:module";
import { spawnSync } from "node:child_process";
import { setImmediate as nextTurn } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const limit = 16 * 1024 * 1024;
const negative = process.argv.includes("--without-detachment");

async function loadParser() {
    const modules = new Map();
    for (const name of [
        "markdown",
        "markdown-retention",
        "markdown-incremental",
    ]) {
        const url = new URL(
            `../apps/lorepia/src/features/chat/${name}.ts`,
            import.meta.url,
        );
        let source =
            negative && name === "markdown-retention"
                ? "export function detachMarkdownBlocks() {}"
                : stripTypeScriptTypes(await readFile(url, "utf8"), {
                      mode: "transform",
                  });
        for (const [dependency, compiled] of modules)
            source = source.replaceAll(
                `'./${dependency}'`,
                JSON.stringify(compiled),
            );
        modules.set(name, `data:text/javascript,${encodeURIComponent(source)}`);
    }
    return (await import(modules.get("markdown-incremental")))
        .createIncrementalMarkdownParser;
}

async function collect() {
    for (let index = 0; index < 5; index++) {
        await nextTurn();
        global.gc();
    }
    return process.memoryUsage().heapUsed;
}

async function measure() {
    const createParser = await loadParser();
    let parser = createParser();
    let source = "";
    let blocks;
    let first;
    for (let index = 0; index < 1800; index++) {
        const paragraph = `${String(index).padStart(5, "0")}:${"abcdefghijklmnopqrstuvwx".repeat(4)}\n\n`;
        // Fresh flat IPC-like inputs keep fixture ropes out of the retained graph.
        source = Buffer.from(source + paragraph, "utf8").toString("utf8");
        blocks = parser(source);
        if (index === 0) first = blocks[0];
        assert.equal(blocks[0], first);
        if (index % 50 === 0) await nextTurn();
    }
    assert.equal(blocks.length, 1800);
    assert.equal(Buffer.byteLength(source), 187200);
    globalThis.retainedMarkdown = { parser, source, blocks };
    parser = source = blocks = first = null;
    const retained = await collect();
    globalThis.retainedMarkdown = null;
    const dropped = await collect();
    return retained - dropped;
}

if (process.argv.includes("--measure")) {
    console.log(JSON.stringify({ retainedBytes: await measure() }));
} else {
    for (const control of [false, true]) {
        const child = spawnSync(
            process.execPath,
            [
                "--expose-gc",
                fileURLToPath(import.meta.url),
                "--measure",
                ...(control ? ["--without-detachment"] : []),
            ],
            { encoding: "utf8" },
        );
        assert.equal(child.status, 0, child.stderr);
        const { retainedBytes } = JSON.parse(child.stdout);
        assert.ok(Number.isFinite(retainedBytes));
        if (control)
            assert.ok(
                retainedBytes > limit,
                "negative control must fail the heap bound",
            );
        else
            assert.ok(
                retainedBytes < limit,
                `Markdown retained ${retainedBytes} bytes (limit ${limit})`,
            );
        console.log(
            `Markdown ${control ? "negative control" : "retention"}: ${retainedBytes} bytes`,
        );
    }
}

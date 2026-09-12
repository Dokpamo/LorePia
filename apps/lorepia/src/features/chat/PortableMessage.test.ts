import { cleanup, render, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { CharacterRenderProfileDto, LorepiaClient } from '../../lib/ipc/contracts';

const tauriMocks = vi.hoisted(() => ({
    convertFileSrc: vi.fn<(filePath: string, protocol?: string) => string>(),
}));

vi.mock('@tauri-apps/api/core', () => tauriMocks);

import PortableMessage from './PortableMessage.svelte';
import { PORTABLE_RENDERER_CHANNEL } from './portable-renderer-protocol';

const SHA256 = 'cd'.repeat(32);
const profile: CharacterRenderProfileDto = {
    character_id: 'character',
    character_content_revision_id: 'revision',
    assets: [
        {
            asset_id: 'asset-expression',
            aliases: ['assets/other/image/Guide_smile_1.png.png', 'Guide_smile_1.png'],
        },
    ],
    background_markup: '<style>.panel { color: rgb(1, 2, 3); }</style>',
    toggle_schema: '',
    initial_variables: {},
    output_transforms: [],
    display_transforms: [],
    runtime_scripts: [],
    required_runtime_capabilities: [],
    runtime_capabilities_declared: false,
    runtime_knowledge: [],
    runtime_script_count: 0,
};

beforeEach(() => {
    tauriMocks.convertFileSrc.mockReset();
    tauriMocks.convertFileSrc.mockImplementation(
        (filePath: string, protocol = 'asset') => `http://${protocol}.localhost/${filePath}`,
    );
});

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

async function portableFrame(container: HTMLElement): Promise<HTMLIFrameElement> {
    await waitFor(() => {
        const frame = container.querySelector<HTMLIFrameElement>('.portable-frame');
        expect(frame).not.toBeNull();
        expect(frame?.srcdoc).toContain('<!doctype html>');
    });
    const frame = container.querySelector<HTMLIFrameElement>('.portable-frame');
    if (frame === null) throw new Error('portable frame was not rendered');
    return frame;
}

function frameDocument(frame: HTMLIFrameElement): Document {
    return new DOMParser().parseFromString(frame.srcdoc, 'text/html');
}

function runtimeId(frame: HTMLIFrameElement): string {
    const id = frameDocument(frame).querySelector('script[nonce]')?.getAttribute('data-runtime-id');
    if (!id) throw new Error('portable runtime id is missing');
    return id;
}

describe('PortableMessage', () => {
    it('renders background-only controls and verified assets through the same isolated sanitizer', async () => {
        const resolveAssetDelivery = vi
            .fn()
            .mockResolvedValue({ asset_id: 'asset-expression', sha256: SHA256 });
        const roomProfile = {
            ...profile,
            background_markup:
                '<style>.floating { position: fixed; top: 0; }</style><div class="floating">{{button::Settings::ToggleSettings}}</div><section id="background-panel">Panel<img=Guide_smile_1.png><script>evil()</script><img src="https://evil.test/pixel" onerror="evil()"></section>',
        };
        const client = { resolveAssetDelivery } as unknown as LorepiaClient;
        const room = render(PortableMessage, {
            text: 'Plain chat',
            client,
            profile: roomProfile,
            surface: 'room',
        });
        const frame = await portableFrame(room.container);
        const doc = frameDocument(frame);
        expect(doc.querySelector('[data-portable-action="ToggleSettings"]')).not.toBeNull();
        expect(doc.querySelector('.portable-message')?.textContent).not.toContain('Plain chat');
        expect(doc.querySelector('#background-panel')?.textContent).toContain('Panel');
        expect(doc.querySelector('#background-panel img')?.getAttribute('src')).toBe(
            `http://lorepia-asset.localhost/sha256/${SHA256}`,
        );
        expect(
            doc.querySelector('.portable-message script, [onerror], [src^="https://evil.test"]'),
        ).toBeNull();
        expect(room.container.querySelector('#background-panel')).toBeNull();
        expect(frame.getAttribute('sandbox')).toBe('allow-scripts');
        expect(resolveAssetDelivery).toHaveBeenCalledOnce();
        const inline = render(PortableMessage, {
            text: '<div>Inline message</div>',
            client,
            profile: roomProfile,
        });
        expect(
            frameDocument(await portableFrame(inline.container)).querySelector('#background-panel'),
        ).toBeNull();
        await room.rerender({
            text: '<p>Chat text</p><div class="floating">Message control</div>',
            client,
            profile: roomProfile,
            surface: 'room',
        });
        await waitFor(() =>
            expect(frameDocument(frame).querySelector('.portable-message')?.textContent).toContain(
                'Message control',
            ),
        );
        expect(frameDocument(frame).querySelector('#background-panel')).not.toBeNull();
        expect(frameDocument(frame).querySelector('.portable-message')?.textContent).not.toContain(
            'Chat text',
        );
    });

    it('shares the asset reference budget between room content and background markup', async () => {
        const resolveAssetDelivery = vi.fn();
        const client = { resolveAssetDelivery } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text:
                '<div>' +
                Array.from({ length: 70 }, (_, i) => `<img=message-${String(i)}>`).join('') +
                '</div>',
            client,
            profile: {
                ...profile,
                background_markup: Array.from(
                    { length: 70 },
                    (_, i) => `<img=background-${String(i)}>`,
                ).join(''),
            },
            surface: 'room',
        });
        const doc = frameDocument(await portableFrame(view.container));
        expect(resolveAssetDelivery).not.toHaveBeenCalled();
        expect(doc.querySelector('.portable-message')?.textContent).toBeTruthy();
        expect(doc.querySelector('.portable-message img')).toBeNull();
    });

    it('rebuilds width-dependent markup on frame resize without rebuilding on height-only changes', async () => {
        let width = 320;
        let resized: ResizeObserverCallback | undefined;
        const observe = vi.fn();
        const disconnect = vi.fn();
        vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(() => width);
        vi.stubGlobal(
            'ResizeObserver',
            class {
                constructor(callback: ResizeObserverCallback) {
                    resized = callback;
                }
                observe = observe;
                disconnect = disconnect;
            },
        );
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: '<div>{{#if {{? {{screen_width}} <= 600}}}}small{{:else}}wide{{/}}</div>',
            client,
            profile,
        });
        const frame = await portableFrame(view.container);
        expect(observe).toHaveBeenCalledWith(frame);
        await waitFor(() =>
            expect(frameDocument(frame).querySelector('.portable-message')?.textContent).toBe(
                'small',
            ),
        );
        const firstRuntime = runtimeId(frame);
        resized?.([], {} as ResizeObserver);
        await tick();
        expect(runtimeId(frame)).toBe(firstRuntime);
        width = 900;
        resized?.([], {} as ResizeObserver);
        await waitFor(() =>
            expect(frameDocument(frame).querySelector('.portable-message')?.textContent).toBe(
                'wide',
            ),
        );
        expect(runtimeId(frame)).not.toBe(firstRuntime);
        view.unmount();
        expect(disconnect).toHaveBeenCalledOnce();
    });

    it('keeps imported floating controls only inside the isolated room surface', async () => {
        const roomProfile = {
            ...profile,
            assets: [],
            background_markup:
                '<style>.floating { position: fixed; right: 20px; top: 40px; } .panel { position: fixed; height: 100vh; }</style>',
        };
        const text =
            '<p>Chat text</p><div class="floating">{{button::Settings::ToggleSettings}}</div><section class="panel">Panel</section>';
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const inline = render(PortableMessage, { text, client, profile: roomProfile });
        const inlineDoc = frameDocument(await portableFrame(inline.container));
        expect(inlineDoc.querySelector('.floating')).toBeNull();
        expect(inlineDoc.body.textContent).toContain('Chat text');
        const room = render(PortableMessage, {
            text,
            client,
            profile: roomProfile,
            surface: 'room',
        });
        const frame = await portableFrame(room.container);
        const roomDoc = frameDocument(frame);
        expect(frame.getAttribute('sandbox')).toBe('allow-scripts');
        expect(roomDoc.querySelector('[data-portable-action="ToggleSettings"]')).not.toBeNull();
        expect(roomDoc.querySelector('.portable-message')?.textContent).not.toContain('Chat text');
        expect(frame.srcdoc).toContain('position:fixed');
        expect(
            roomDoc
                .querySelector('meta[http-equiv="Content-Security-Policy"]')
                ?.getAttribute('content'),
        ).toContain("connect-src 'none'");
    });
    it('refuses oversized portable source before parsing it as card markup', async () => {
        const resolveAssetDelivery = vi.fn();
        const client = { resolveAssetDelivery } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: `<div>${'x'.repeat(262_144)}</div>`,
            client,
            profile,
        });

        await waitFor(() =>
            expect(view.container).toHaveTextContent(
                '카드 콘텐츠가 안전 제한을 초과해 표시하지 않았습니다.',
            ),
        );
        expect(view.container.querySelector('.portable-frame')).toBeNull();
        expect(resolveAssetDelivery).not.toHaveBeenCalled();
    });

    it('bounds markup nodes and unique asset references before resolution', async () => {
        const resolveAssetDelivery = vi.fn();
        const client = { resolveAssetDelivery } as unknown as LorepiaClient;
        const tooManyNodes = render(PortableMessage, {
            text: `<div>${'<span>x</span>'.repeat(2_100)}</div>`,
            client,
            profile,
        });
        const nodeFrame = await portableFrame(tooManyNodes.container);
        expect(frameDocument(nodeFrame).body.textContent).toContain(
            '카드 콘텐츠가 안전 제한을 초과해 표시하지 않았습니다.',
        );

        const tooManyReferences = render(PortableMessage, {
            text: `<div>${Array.from({ length: 129 }, (_, index) => `<img="ref-${String(index)}">`).join('')}</div>`,
            client,
            profile,
        });
        const referenceFrame = await portableFrame(tooManyReferences.container);
        expect(frameDocument(referenceFrame).body.textContent).toContain(
            '카드 콘텐츠가 안전 제한을 초과해 표시하지 않았습니다.',
        );
        expect(resolveAssetDelivery).not.toHaveBeenCalled();
    });

    it('keeps the previous completed frame visible while a replacement resolves', async () => {
        const client = {
            resolveAssetDelivery: vi.fn().mockRejectedValue(new Error('unavailable')),
        } as unknown as LorepiaClient;
        const view = render(PortableMessage, { text: '<div>First</div>', client, profile });
        const frame = await portableFrame(view.container);
        const previous = frame.srcdoc;
        let rejectDelivery: (reason: Error) => void = vi.fn();
        const waiting = new Promise<never>((_resolve, reject) => {
            rejectDelivery = reject;
        });
        const resolveAssetDelivery = vi.fn().mockReturnValue(waiting);
        await view.rerender({
            text: '<div>Second<img="Guide_smile"></div>',
            client: { ...client, resolveAssetDelivery },
            profile,
        });
        await waitFor(() => expect(resolveAssetDelivery).toHaveBeenCalledOnce());
        expect(frame.srcdoc).toBe(previous);
        rejectDelivery(new Error('unavailable'));
        await waitFor(() => expect(frame.srcdoc).toContain('Second'));
    });

    it('resolves a command prefix to one verified character asset inside the sandbox', async () => {
        const resolveAssetDelivery = vi.fn().mockResolvedValue({
            asset_id: 'asset-expression',
            sha256: SHA256,
            media_type: 'image/png',
            kind: 'image',
            size_bytes: 100,
            width: 320,
            height: 480,
            duration_ms: null,
            url: `lorepia-asset://sha256/${SHA256}`,
        });
        const client = { resolveAssetDelivery } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: 'hello\n<img="Guide_smile">',
            client,
            profile,
        });

        const frame = await portableFrame(view.container);
        expect(frameDocument(frame).querySelector('img')?.getAttribute('src')).toBe(
            `http://lorepia-asset.localhost/sha256/${SHA256}`,
        );
        expect(resolveAssetDelivery).toHaveBeenCalledWith({
            selector: { kind: 'asset_id', asset_id: 'asset-expression' },
        });
    });

    it('normalizes provider output before resolving live portable markup', async () => {
        const resolveAssetDelivery = vi.fn().mockResolvedValue({
            asset_id: 'asset-expression',
            sha256: SHA256,
            media_type: 'image/png',
            kind: 'image',
            size_bytes: 100,
            width: 320,
            height: 480,
            duration_ms: null,
            url: `lorepia-asset://sha256/${SHA256}`,
        });
        const client = { resolveAssetDelivery } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: '<lmg="Guide_smile">',
            client,
            profile: {
                ...profile,
                output_transforms: [{ pattern: '<lmg="', replacement: '<img="', flags: '' }],
            },
        });

        const frame = await portableFrame(view.container);
        expect(frameDocument(frame).querySelector('img')?.getAttribute('src')).toBe(
            `http://lorepia-asset.localhost/sha256/${SHA256}`,
        );
    });

    it('shows why one malformed card regex was disabled while preserving the message', async () => {
        const view = render(PortableMessage, {
            text: 'ordinary text',
            profile: {
                ...profile,
                output_transforms: [{ pattern: '(', replacement: 'lost', flags: '' }],
            },
        });

        await waitFor(() => {
            expect(view.container).toHaveTextContent('ordinary text');
            expect(view.container).toHaveTextContent(
                '카드 정규식 규칙 1이 올바르지 않아 비활성화했습니다.',
            );
        });
    });

    it('keeps card markup out of the parent DOM and gives its frame no same-origin authority', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: '<details class="panel"><summary>Status</summary><script>bad()</script><span onclick="bad()">OK</span></details>',
            client,
            profile,
        });

        const frame = await portableFrame(view.container);
        const rendered = frameDocument(frame);
        expect(view.container.querySelector('details')).toBeNull();
        expect(frame).toHaveAttribute('sandbox', 'allow-scripts');
        expect(frame.getAttribute('sandbox')).not.toContain('allow-same-origin');
        expect(rendered.querySelector('details')).not.toBeNull();
        expect(rendered.querySelectorAll('script')).toHaveLength(1);
        const bridge = rendered.querySelector('script[nonce]');
        expect(bridge?.textContent).toBe('');
        expect(bridge?.getAttribute('src')).toBe(
            new URL('/src/features/chat/portable-renderer-bridge.js?no-inline', document.baseURI)
                .href,
        );
        expect(rendered.querySelector('script[nonce]')?.textContent).not.toContain('bad()');
        expect(rendered.querySelector('[onclick]')).toBeNull();
        expect(rendered.body.textContent).toContain('OK');
        expect(rendered.querySelector('style')?.textContent).toContain(
            '.panel{color:rgb(1, 2, 3);}',
        );
        expect(frame.srcdoc).not.toMatch(/__TAURI|invoke\s*\(|fetch\s*\(|XMLHttpRequest/i);
    });

    it('blocks alternate network surfaces, CSS URLs, and viewport overlays', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: [
                '<style>@import "https://evil.test/a.css"; .overlay { position: fixed; inset: 0; z-index: 999999; width: 100vw; background: image-set(url(https://evil.test/pixel) 1x); pointer-events: all; color: red; }</style>',
                '<svg><foreignObject><div>spoof</div></foreignObject></svg>',
                '<picture><source srcset="https://evil.test/a.png"><img srcset="https://evil.test/b.png" poster="https://evil.test/p.png"></picture>',
                '<form action="https://evil.test"><input autofocus formaction="https://evil.test"></form>',
                '<a href="https://evil.test" ping="https://evil.test">remote</a>',
                '<button data-portable-action="spoof">bad</button>',
            ].join(''),
            client,
            profile,
        });

        const frame = await portableFrame(view.container);
        const rendered = frameDocument(frame);
        expect(rendered.querySelector('svg, foreignObject, picture, source, form')).toBeNull();
        expect(
            rendered.querySelector('[srcset], [poster], [autofocus], [href], [ping]'),
        ).toBeNull();
        expect(rendered.querySelector('[data-portable-action]')).toBeNull();
        const importedStyle = rendered.querySelector('.portable-message style')?.textContent ?? '';
        expect(importedStyle).not.toMatch(
            /@import|position\s*:\s*fixed|z-index|100vw|image-set|url\s*\(|pointer-events/i,
        );
        expect(importedStyle).toContain('color:red');
        const csp = rendered
            .querySelector('meta[http-equiv="Content-Security-Policy"]')
            ?.getAttribute('content');
        expect(csp).toContain("default-src 'none'");
        expect(csp).toContain("connect-src 'none'");
        expect(csp).toContain('img-src lorepia-asset: http://lorepia-asset.localhost');
    });

    it('strips browser top-layer primitives from imported markup', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: [
                '<style>select { appearance: base-select; } select::picker(select) { position: fixed; }</style>',
                '<button popovertarget="spoof" command="show-popover" commandfor="spoof">Open</button>',
                '<div id="spoof" popover>spoof</div>',
                '<dialog open>trusted-looking dialog</dialog>',
                '<select><option>trusted-looking picker</option></select>',
            ].join(''),
            client,
            profile,
        });

        const rendered = frameDocument(await portableFrame(view.container));
        expect(rendered.querySelector('dialog, select, option')).toBeNull();
        expect(
            rendered.querySelector('[popover], [popovertarget], [command], [commandfor]'),
        ).toBeNull();
        expect(rendered.querySelector('.portable-message style')?.textContent).not.toMatch(
            /appearance|::picker/i,
        );
    });

    it('rejects escaped CSS policy bypasses and retains the parent paint boundary', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: String.raw`<style>
                :host { contain: none !important; overflow: visible !important; }
                .overlay { position: f\69xed; z\2d index: 999999; inset: 0; }
            </style><div class="overlay">spoof</div>`,
            client,
            profile,
        });

        const frame = await portableFrame(view.container);
        const boundary = view.container.querySelector('.portable-boundary');
        const importedStyle =
            frameDocument(frame).querySelector('.portable-message style')?.textContent ?? '';
        expect(boundary).not.toBeNull();
        expect(importedStyle).toBe('');
        if (boundary === null) throw new Error('portable boundary is missing');
        expect(getComputedStyle(boundary).contain).toContain('paint');
        expect(getComputedStyle(boundary).overflow).not.toBe('visible');
    });

    it('uses the ordinary markdown renderer when no portable markup is present', () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: '**ordinary** message',
            client,
            profile,
        });

        expect(view.container.querySelector('.portable-frame')).toBeNull();
        expect(view.container.querySelector('strong')).toHaveTextContent('ordinary');
    });

    it('renders safe external greeting macros without granting imported UI code', () => {
        const view = render(PortableMessage, {
            text: [
                '{{#if_pure {{equal::{{getvar::lang}}::0}}}}English{{/if}}',
                '{{#if_pure {{equal::{{getvar::lang}}::1}}}}Hello {{user}}, {{char}}{{/if}}',
            ].join(''),
            profile: { ...profile, initial_variables: { lang: '1' } },
            enabled: false,
            expandMacros: true,
            characterName: 'Hunters',
            userName: 'Tester',
        });

        expect(view.container.querySelector('.portable-frame')).toBeNull();
        expect(view.container).toHaveTextContent('Hello Tester, Hunters');
        expect(view.container).not.toHaveTextContent('{{');
    });

    it('normalizes plain assistant output without rewriting disabled user messages', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const activeProfile = {
            ...profile,
            output_transforms: [{ pattern: 'A-ri_', replacement: 'Ari_', flags: 'g' }],
        };
        const assistant = render(PortableMessage, {
            text: 'A-ri_smile',
            client,
            profile: activeProfile,
        });
        const user = render(PortableMessage, {
            text: 'A-ri_smile',
            client,
            profile: activeProfile,
            enabled: false,
        });

        await waitFor(() => expect(assistant.container).toHaveTextContent('Ari_smile'));
        expect(user.container).toHaveTextContent('A-ri_smile');
    });

    it('evaluates presentation-style toggle blocks before installing them', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const disabled = render(PortableMessage, {
            text: '<details><summary>Status</summary></details>',
            client,
            profile: {
                ...profile,
                background_markup:
                    '{{#when::toggle::compact}}<style>.compact { width: 1px; }</style>{{/when}}',
                initial_variables: { compact: '0' },
            },
        });
        const enabled = render(PortableMessage, {
            text: '<details><summary>Status</summary></details>',
            client,
            profile: {
                ...profile,
                background_markup:
                    '{{#when::toggle::compact}}<style>.compact { width: 1px; }</style>{{/when}}',
                initial_variables: { compact: '1' },
            },
        });

        const disabledSource = (await portableFrame(disabled.container)).srcdoc;
        const enabledSource = (await portableFrame(enabled.container)).srcdoc;
        expect(disabledSource).not.toContain('.compact{width:1px;}');
        expect(enabledSource).toContain('.compact{width:1px;}');
    });

    it('accepts only validated actions from the exact active opaque-origin frame', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const onAction = vi.fn();
        const view = render(PortableMessage, {
            text: [
                '<input type="checkbox" card-btn="generate__radio__1">',
                '<button evil-btn="forged">Forged</button>',
                '<div card-btn="non_interactive">No</div>',
            ].join(''),
            client,
            profile,
            onAction,
        });

        const frame = await portableFrame(view.container);
        const rendered = frameDocument(frame);
        expect(rendered.querySelector('input')?.getAttribute('data-portable-action')).toBe(
            'generate__radio__1',
        );
        expect(rendered.querySelector('[evil-btn]')).toBeNull();
        expect(rendered.querySelector('div[data-portable-action]')).toBeNull();

        const id = runtimeId(frame);
        const send = (source: MessageEventSource | null, candidateId: string, action: string) => {
            globalThis.dispatchEvent(
                new MessageEvent('message', {
                    origin: 'null',
                    source,
                    data: {
                        channel: PORTABLE_RENDERER_CHANNEL,
                        type: 'portable_action',
                        runtimeId: candidateId,
                        action,
                    },
                }),
            );
        };
        send(window, id, 'generate__radio__1');
        send(frame.contentWindow, '00000000-0000-4000-8000-000000000000', 'generate__radio__1');
        send(frame.contentWindow, id, 'contains spaces');
        expect(onAction).not.toHaveBeenCalled();

        send(frame.contentWindow, id, 'generate__radio__1');
        expect(onAction).toHaveBeenCalledOnce();
        expect(onAction).toHaveBeenCalledWith('generate__radio__1');
    });

    it('accepts bounded resize messages only from the exact frame', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: '<details><summary>Status</summary></details>',
            client,
            profile,
        });
        const frame = await portableFrame(view.container);
        const id = runtimeId(frame);

        globalThis.dispatchEvent(
            new MessageEvent('message', {
                origin: 'null',
                source: frame.contentWindow,
                data: {
                    channel: PORTABLE_RENDERER_CHANNEL,
                    type: 'portable_resize',
                    runtimeId: id,
                    height: 200,
                },
            }),
        );
        expect(frame.style.height).toBe('200px');

        globalThis.dispatchEvent(
            new MessageEvent('message', {
                origin: 'null',
                source: frame.contentWindow,
                data: {
                    channel: PORTABLE_RENDERER_CHANNEL,
                    type: 'portable_resize',
                    runtimeId: id,
                    height: 100_000,
                },
            }),
        );
        expect(frame.style.height).toBe('200px');
    });

    it('resolves embedded background audio without exposing a local path', async () => {
        const resolveAssetDelivery = vi.fn().mockResolvedValue({
            asset_id: 'asset-audio',
            sha256: SHA256,
            media_type: 'audio/mpeg',
            kind: 'audio',
            size_bytes: 100,
            width: null,
            height: null,
            duration_ms: 1_000,
            url: '/Users/private/original.mp3',
        });
        const client = { resolveAssetDelivery } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: [
                '{{#when::{{contains::{{lastcharmessage}}::Health: 0/}}}}',
                '{{#when::toggle::music}}{{bgm::scene-track.mp3}}{{/when}}',
                '<div>GAME OVER</div>',
                '{{/when}}',
            ].join(''),
            client,
            profile: {
                ...profile,
                assets: [{ asset_id: 'asset-audio', aliases: ['scene-track.mp3'] }],
            },
            variables: { music: '1' },
            lastCharacterMessage: '[Status]\nHealth: 0/100\n[/Status]',
        });

        const frame = await portableFrame(view.container);
        const rendered = frameDocument(frame);
        expect(rendered.querySelector('audio')?.getAttribute('src')).toBe(
            `http://lorepia-asset.localhost/sha256/${SHA256}`,
        );
        expect(rendered.querySelector('audio')?.hasAttribute('autoplay')).toBe(false);
        expect(rendered.querySelector('audio')?.getAttribute('data-portable-autoplay')).toBe(
            'true',
        );
        expect(rendered.querySelector('audio')?.hasAttribute('loop')).toBe(true);
        expect(rendered.body.textContent).toContain('GAME OVER');
        expect(frame.srcdoc).not.toContain('/Users/private/original.mp3');
    });
});

it('keeps identical sanitized srcdoc and accepts its existing bridge after a no-op rebuild', async () => {
    const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
    const onAction = vi.fn();
    const view = render(PortableMessage, {
        text: '<div>{{button::Run::run}}<script>first()</script></div>',
        client,
        profile,
        onAction,
    });
    const frame = await portableFrame(view.container);
    const previous = frame.srcdoc;
    const id = runtimeId(frame);
    await view.rerender({
        text: '<div>{{button::Run::run}}<script>second()</script></div>',
        client,
        profile,
        onAction,
    });
    await tick();
    await waitFor(() => {
        globalThis.dispatchEvent(
            new MessageEvent('message', {
                origin: 'null',
                source: frame.contentWindow,
                data: {
                    channel: PORTABLE_RENDERER_CHANNEL,
                    type: 'portable_action',
                    runtimeId: id,
                    action: 'run',
                },
            }),
        );
        expect(onAction).toHaveBeenCalled();
    });
    expect(frame.srcdoc).toBe(previous);
    expect(runtimeId(frame)).toBe(id);
});

it('defers historical audio loading/autoplay while preserving room BGM attributes', async () => {
    const client = {
        resolveAssetDelivery: vi.fn().mockResolvedValue({ asset_id: 'audio', sha256: SHA256 }),
    } as unknown as LorepiaClient;
    const audioProfile = { ...profile, assets: [{ asset_id: 'audio', aliases: ['track.mp3'] }] };
    const message = render(PortableMessage, {
        text: '{{bgm::track.mp3}}',
        client,
        profile: audioProfile,
    });
    const audio = frameDocument(await portableFrame(message.container)).querySelector('audio');
    expect(audio?.hasAttribute('autoplay')).toBe(false);
    expect(audio?.getAttribute('data-portable-autoplay')).toBe('true');
    expect(audio?.getAttribute('preload')).toBe('none');
    const room = render(PortableMessage, {
        text: '{{bgm::track.mp3}}',
        client,
        profile: { ...audioProfile, background_markup: '<div>{{bgm::track.mp3}}</div>' },
        surface: 'room',
    });
    expect(
        frameDocument(await portableFrame(room.container))
            .querySelector('audio')
            ?.hasAttribute('autoplay'),
    ).toBe(true);
});

it('retains an unchanged frame when only the render-only last-message index advances', async () => {
    const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
    const onAction = vi.fn();
    const props = {
        text: '<div>static{{button::Run::run}}</div>',
        client,
        profile,
        onAction,
        messageIndex: 0,
    };
    const view = render(PortableMessage, { ...props, lastMessageId: 1 });
    const frame = await portableFrame(view.container);
    const previous = frame.srcdoc;
    const id = runtimeId(frame);
    await view.rerender({ ...props, lastMessageId: 2 });
    await vi.waitFor(() => {
        globalThis.dispatchEvent(
            new MessageEvent('message', {
                origin: 'null',
                source: frame.contentWindow,
                data: {
                    channel: PORTABLE_RENDERER_CHANNEL,
                    type: 'portable_action',
                    runtimeId: id,
                    action: 'run',
                },
            }),
        );
        expect(onAction).toHaveBeenCalled();
    });
    expect(frame.srcdoc).toBe(previous);
    expect(runtimeId(frame)).toBe(id);
    onAction.mockClear();
    await view.rerender({
        ...props,
        text: '<div>{{lastmessageid}}{{button::Run::run}}</div>',
        lastMessageId: 3,
    });
    await vi.waitFor(() => expect(runtimeId(frame)).not.toBe(id));
    expect(frameDocument(frame).body.textContent).toContain('3');
    globalThis.dispatchEvent(
        new MessageEvent('message', {
            origin: 'null',
            source: frame.contentWindow,
            data: {
                channel: PORTABLE_RENDERER_CHANNEL,
                type: 'portable_action',
                runtimeId: id,
                action: 'run',
            },
        }),
    );
    expect(onAction).not.toHaveBeenCalled();
});

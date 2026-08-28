import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { CharacterRenderProfileDto, LorepiaClient } from '../../lib/ipc/contracts';

const tauriMocks = vi.hoisted(() => ({
    convertFileSrc: vi.fn<(filePath: string, protocol?: string) => string>(),
}));

vi.mock('@tauri-apps/api/core', () => tauriMocks);

import PortableMessage from './PortableMessage.svelte';

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
});

describe('PortableMessage', () => {
    it('resolves a command prefix to one verified character asset', async () => {
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

        await waitFor(() => {
            const shadow = view.container.querySelector('.portable-host')?.shadowRoot;
            expect(shadow?.querySelector('img')).toHaveAttribute(
                'src',
                `http://lorepia-asset.localhost/sha256/${SHA256}`,
            );
        });
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

        await waitFor(() => {
            expect(
                view.container.querySelector('.portable-host')?.shadowRoot?.querySelector('img'),
            ).toHaveAttribute('src', `http://lorepia-asset.localhost/sha256/${SHA256}`);
        });
    });

    it('keeps imported presentation styles isolated and removes executable markup', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: '<details class="panel"><summary>Status</summary><script>bad()</script><span onclick="bad()">OK</span></details>',
            client,
            profile,
        });

        await waitFor(() => {
            const shadow = view.container.querySelector('.portable-host')?.shadowRoot;
            expect(shadow?.querySelector('details')).not.toBeNull();
            expect(shadow?.querySelector('script')).toBeNull();
            expect(shadow?.querySelector('[onclick]')).toBeNull();
            expect(shadow?.textContent).toContain('OK');
            expect(shadow?.textContent).toContain('.panel { color: rgb(1, 2, 3); }');
        });
    });

    it('contains fixed overlays and strips alternate network-loading surfaces', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: [
                '<style>@import "https://evil.test/a.css"; .overlay { position: fixed; inset: 0; z-index: 999999; background: url(https://evil.test/pixel); }</style>',
                '<svg><foreignObject><div>spoof</div></foreignObject></svg>',
                '<picture><source srcset="https://evil.test/a.png"><img srcset="https://evil.test/b.png" poster="https://evil.test/p.png"></picture>',
                '<button data-portable-action="spoof">bad</button>',
            ].join(''),
            client,
            profile,
        });

        await waitFor(() => {
            const shadow = view.container.querySelector('.portable-host')?.shadowRoot;
            expect(shadow).not.toBeNull();
            expect(shadow?.querySelector('svg, foreignObject, picture, source')).toBeNull();
            expect(shadow?.querySelector('[srcset], [poster]')).toBeNull();
            expect(shadow?.querySelector('[data-portable-action]')).toBeNull();
            expect(shadow?.textContent).not.toMatch(/@import|position\s*:\s*fixed|url\s*\(/i);
            expect(shadow?.textContent).toContain('contain: layout paint style');
        });
    });

    it('strips browser top-layer primitives from imported markup', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: [
                '<style>select { appearance: base-select; } select::picker(select) { position: fixed; }</style>',
                '<button popovertarget="spoof" popovertargetaction="show" command="show-popover" commandfor="spoof">Open</button>',
                '<div id="spoof" popover>spoof</div>',
                '<dialog open>trusted-looking dialog</dialog>',
                '<select><option>trusted-looking picker</option></select>',
            ].join(''),
            client,
            profile,
        });

        await waitFor(() => {
            const shadow = view.container.querySelector('.portable-host')?.shadowRoot;
            expect(shadow).not.toBeNull();
            expect(shadow?.querySelector('dialog, select, option')).toBeNull();
            expect(
                shadow?.querySelector(
                    '[popover], [popovertarget], [popovertargetaction], [command], [commandfor]',
                ),
            ).toBeNull();
            expect(shadow?.textContent).not.toMatch(/base-select|::picker/i);
        });
    });

    it('keeps escaped host selectors inside a trusted non-styleable paint boundary', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: String.raw`<style>
                :host { contain: none !important; overflow: visible !important; }
                .overlay { position: f\69xed; z\2d index: 999999; inset: 0; }
            </style><div class="overlay">spoof</div>`,
            client,
            profile,
        });

        await waitFor(() => {
            const boundary = view.container.querySelector('.portable-boundary');
            const shadow = view.container.querySelector('.portable-host')?.shadowRoot;
            expect(boundary).not.toBeNull();
            expect(shadow).not.toBeNull();
            if (boundary === null) throw new Error('portable boundary is missing');
            expect(shadow?.querySelector('.portable-message style')?.textContent).not.toMatch(
                /:host|\\|contain\s*:\s*none/i,
            );
            expect(getComputedStyle(boundary).contain).toContain('paint');
            expect(getComputedStyle(boundary).overflow).not.toBe('visible');
        });
    });

    it('uses the ordinary markdown renderer when no portable markup is present', () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: '**ordinary** message',
            client,
            profile,
        });

        expect(view.container.querySelector('.portable-host')).toBeNull();
        expect(view.container.querySelector('strong')).toHaveTextContent('ordinary');
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

        await waitFor(() => {
            const disabledText =
                disabled.container.querySelector('.portable-host')?.shadowRoot?.textContent;
            const enabledText =
                enabled.container.querySelector('.portable-host')?.shadowRoot?.textContent;
            expect(disabledText).not.toContain('.compact');
            expect(enabledText).toContain('.compact { width: 1px; }');
        });
    });

    it('maps portable button attributes to runtime actions', async () => {
        const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
        const onAction = vi.fn();
        const view = render(PortableMessage, {
            text: '<div><input type="checkbox" card-btn="generate__radio__1"><span>Generate</span></div>',
            client,
            profile,
            onAction,
        });

        await waitFor(() => {
            const control =
                view.container
                    .querySelector('.portable-host')
                    ?.shadowRoot?.querySelector<HTMLInputElement>('input') ?? null;
            expect(control).not.toBeNull();
            expect(control).toHaveAttribute('data-portable-action', 'generate__radio__1');
            expect(control).not.toHaveAttribute('card-btn');
        });
        const control = view.container
            .querySelector('.portable-host')
            ?.shadowRoot?.querySelector<HTMLInputElement>('input');
        if (!(control instanceof HTMLInputElement)) {
            throw new Error('portable action control was not rendered');
        }
        await fireEvent.click(control);
        expect(onAction).toHaveBeenCalledWith('generate__radio__1');
    });

    it('resolves and starts embedded background audio', async () => {
        const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
        const resolveAssetDelivery = vi.fn().mockResolvedValue({
            asset_id: 'asset-audio',
            sha256: SHA256,
            media_type: 'audio/mpeg',
            kind: 'audio',
            size_bytes: 100,
            width: null,
            height: null,
            duration_ms: 1_000,
            url: `lorepia-asset://sha256/${SHA256}`,
        });
        const client = { resolveAssetDelivery } as unknown as LorepiaClient;
        const view = render(PortableMessage, {
            text: [
                '<style>.death-overlay { position: fixed; }</style>',
                '{{#when::{{contains::{{lastcharmessage}}::Health: 0/}}}}',
                '{{#when::toggle::music}}{{bgm::scene-track.mp3}}{{/when}}',
                '<div class="death-overlay">GAME OVER</div>',
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

        await waitFor(() => {
            const audio =
                view.container
                    .querySelector('.portable-host')
                    ?.shadowRoot?.querySelector<HTMLAudioElement>('audio') ?? null;
            expect(audio).not.toBeNull();
            expect(audio).toHaveAttribute('src', `http://lorepia-asset.localhost/sha256/${SHA256}`);
            expect(audio?.loop).toBe(true);
            expect(play).toHaveBeenCalled();
            expect(audio?.closest('.portable-message')).toHaveTextContent('GAME OVER');
        });
    });
});

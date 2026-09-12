import { tick } from 'svelte';
import { cleanup, render, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, it, expect, vi } from 'vitest';
import type { CharacterRenderProfileDto, LorepiaClient } from '../../lib/ipc/contracts';
import PortableMessage from './PortableMessage.svelte';
import * as display from './portable-display';
import * as policy from './portable-renderer-policy';
vi.mock('@tauri-apps/api/core', () => ({
    convertFileSrc: (value: string) => `http://lorepia-asset.localhost/${value}`,
}));
vi.mock('./portable-display', async (importOriginal) => {
    const m = await importOriginal<typeof display>();
    return {
        ...m,
        renderPortableDisplay: vi.fn(m.renderPortableDisplay),
        applyPortableTransforms: vi.fn(m.applyPortableTransforms),
    };
});
vi.mock('./portable-renderer-policy', async (importOriginal) => {
    const m = await importOriginal<typeof policy>();
    return { ...m, sanitizePortableTree: vi.fn(m.sanitizePortableTree) };
});
afterEach(cleanup);
beforeEach(() => vi.clearAllMocks());
const profile: CharacterRenderProfileDto = {
    character_id: 'synthetic',
    character_content_revision_id: 'r',
    assets: [{ asset_id: 'same', aliases: ['image'] }],
    background_markup: '',
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
it('skips normalization, display, asset resolution and sanitization for static context updates', async () => {
    const resolveAssetDelivery = vi
        .fn()
        .mockResolvedValue({ asset_id: 'same', sha256: 'aa'.repeat(32) });
    const client = { resolveAssetDelivery } as unknown as LorepiaClient;
    const props = { text: '<div><img=image>static</div>', profile, client, lastMessageId: 1 };
    const views = Array.from({ length: 3 }, () => render(PortableMessage, props));
    await waitFor(() =>
        expect(
            views.every((v) =>
                v.container.querySelector('iframe')?.srcdoc.includes('<!doctype html>'),
            ),
        ).toBe(true),
    );
    const old = views.map((v) => v.container.querySelector('iframe')?.srcdoc);
    const before = {
        resolve: resolveAssetDelivery.mock.calls.length,
        display: vi.mocked(display.renderPortableDisplay).mock.calls.length,
        output: vi.mocked(display.applyPortableTransforms).mock.calls.length,
        sanitize: vi.mocked(policy.sanitizePortableTree).mock.calls.length,
    };
    for (const view of views)
        await view.rerender({
            lastMessageId: 2,
            lastCharacterMessage: 'new unrelated assistant',
            variables: { unrelated: 'new' },
        });
    await tick();
    await new Promise((resolve) => setTimeout(resolve, 0));
    const delta = {
        resolve: resolveAssetDelivery.mock.calls.length - before.resolve,
        display: vi.mocked(display.renderPortableDisplay).mock.calls.length - before.display,
        output: vi.mocked(display.applyPortableTransforms).mock.calls.length - before.output,
        sanitize: vi.mocked(policy.sanitizePortableTree).mock.calls.length - before.sanitize,
        changedSrcdoc: views.filter(
            (v, i) => v.container.querySelector('iframe')?.srcdoc !== old[i],
        ).length,
    };
    expect(delta).toEqual({ resolve: 0, display: 0, output: 0, sanitize: 0, changedSrcdoc: 0 });
});

it('revalidates static assets when profile, client or source changes', async () => {
    const resolveAssetDelivery = vi
        .fn()
        .mockResolvedValue({ asset_id: 'same', sha256: 'aa'.repeat(32) });
    const client = { resolveAssetDelivery } as unknown as LorepiaClient;
    const view = render(PortableMessage, { text: '<div><img=image>static</div>', profile, client });
    await waitFor(() => expect(view.container.querySelector('iframe')?.srcdoc).toContain('static'));
    for (const props of [
        { profile: { ...profile, character_content_revision_id: 'next' } },
        { client: { resolveAssetDelivery } as unknown as LorepiaClient },
        { text: '<div><img=image>changed</div>' },
    ]) {
        const calls = resolveAssetDelivery.mock.calls.length;
        await view.rerender(props);
        await waitFor(() => expect(resolveAssetDelivery).toHaveBeenCalledTimes(calls + 1));
        await new Promise((resolve) => setTimeout(resolve, 0));
    }
    expect(policy.sanitizePortableTree).toHaveBeenCalledTimes(4);
});

it('retains dynamic head evaluation in source and background and on static-to-dynamic transitions', async () => {
    const resolveAssetDelivery = vi
        .fn()
        .mockResolvedValue({ asset_id: 'same', sha256: 'aa'.repeat(32) });
    const client = { resolveAssetDelivery } as unknown as LorepiaClient;
    const view = render(PortableMessage, {
        text: '<div><img=image>static</div>',
        profile,
        client,
        lastMessageId: 1,
    });
    await waitFor(() => expect(view.container.querySelector('iframe')?.srcdoc).toContain('static'));
    await view.rerender({ text: '<div><img=image>{{lastmessageid}}</div>', lastMessageId: 2 });
    await waitFor(() => expect(resolveAssetDelivery).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(view.container.querySelector('iframe')?.srcdoc).toContain('>2<'));
    await view.rerender({ lastMessageId: 3 });
    await waitFor(() => expect(resolveAssetDelivery).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(view.container.querySelector('iframe')?.srcdoc).toContain('>3<'));
    await view.rerender({
        surface: 'room',
        backgroundMarkup: '<div>{{lastmessageid}}</div>',
        lastMessageId: 4,
    });
    await waitFor(() => expect(view.container.querySelector('iframe')?.srcdoc).toContain('>4<'));
    await view.rerender({ lastMessageId: 5 });
    await waitFor(() => expect(view.container.querySelector('iframe')?.srcdoc).toContain('>5<'));
});

it('keeps live variables and last-assistant dependencies when transforms or macros are present', async () => {
    const client = { resolveAssetDelivery: vi.fn() } as unknown as LorepiaClient;
    const transformed = {
        ...profile,
        output_transforms: [{ pattern: 'TOKEN', replacement: '{{lastmessageid}}', flags: '' }],
    };
    const view = render(PortableMessage, {
        text: '<div>TOKEN</div>',
        profile: transformed,
        client,
        lastMessageId: 1,
    });
    await waitFor(() => expect(view.container.querySelector('iframe')?.srcdoc).toContain('>1<'));
    await view.rerender({ lastMessageId: 2 });
    await waitFor(() => expect(view.container.querySelector('iframe')?.srcdoc).toContain('>2<'));
    await view.rerender({
        profile,
        text: '<div>{{getvar::tone}}|{{lastcharmessage}}</div>',
        variables: { tone: 'A' },
        lastCharacterMessage: 'first',
    });
    await waitFor(() =>
        expect(view.container.querySelector('iframe')?.srcdoc).toContain('A|first'),
    );
    await view.rerender({ variables: { tone: 'B' }, lastCharacterMessage: 'second' });
    await waitFor(() =>
        expect(view.container.querySelector('iframe')?.srcdoc).toContain('B|second'),
    );
});

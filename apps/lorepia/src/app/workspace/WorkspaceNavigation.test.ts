import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import WorkspaceApp from './WorkspaceApp.svelte';
import { createPreviewClient } from '../../preview/mock-client';
import { t } from '../../lib/i18n';
import type { AssetDeliveryDto } from '../../lib/ipc/contracts';

vi.mock('@tauri-apps/api/core', () => ({
    convertFileSrc: (value: string, protocol: string) => `http://${protocol}.localhost/${value}`,
}));

beforeAll(() => {
    if (typeof Reflect.get(Element.prototype, 'getAnimations') !== 'function')
        Object.defineProperty(Element.prototype, 'getAnimations', {
            configurable: true,
            value: () => [],
        });
});
afterEach(cleanup);

describe('workspace settings navigation', () => {
    it.each(['history', 'chat'] as const)(
        'returns from room AI settings to the same room panel and its %s origin',
        async (origin) => {
            const client = createPreviewClient();
            const [character] = await client.listCharacters();
            if (!character) throw new Error('Character fixture missing');
            const [conversation] = await client.listConversations(character.id);
            if (!conversation) throw new Error('Conversation fixture missing');
            render(WorkspaceApp, { client });
            await fireEvent.click(screen.getByRole('button', { name: t('navigation.chats') }));
            await fireEvent.click(
                await screen.findByRole('button', {
                    name: new RegExp('^' + conversation.title + ' ·'),
                }),
            );
            await screen.findByRole('textbox', { name: t('uiPreview.message') });
            if (origin === 'history') {
                await fireEvent.keyDown(window, { key: 'Escape' });
                await screen.findByRole('region', { name: t('uiPreview.history') });
                expect(
                    screen.queryByRole('button', {
                        name: t('uiPreview.namedRoomSettings', { title: conversation.title }),
                    }),
                ).toBeNull();
                await fireEvent.click(
                    screen.getByRole('button', {
                        name: new RegExp('^' + conversation.title + ' ·'),
                    }),
                );
            }
            await fireEvent.click(
                await screen.findByRole('button', {
                    name: t('uiPreview.roomSettings'),
                }),
            );
            const room = await screen.findByRole('dialog', { name: t('uiPreview.roomSettings') });
            await fireEvent.click(
                within(room).getByRole('button', { name: t('workspace.aiSettings') }),
            );
            const ai = await screen.findByRole(
                'dialog',
                { name: t('workspaceAi.title') },
                { timeout: 5000 },
            );
            await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([ai]));
            expect(room.isConnected).toBe(true);
            expect(room).toHaveProperty('inert', true);
            await fireEvent.click(within(ai).getByRole('button', { name: t('uiPreview.back') }));
            await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([room]));
            expect(room).toHaveProperty('inert', false);
            await fireEvent.click(
                within(room).getByRole('button', { name: t('uiPreview.closeChoices') }),
            );
            await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
            expect(screen.getByRole('textbox', { name: t('uiPreview.message') })).toBeVisible();
        },
    );

    it('returns from connections to the retained AI parent before the settings root', async () => {
        const client = createPreviewClient();
        vi.spyOn(client, 'listCharacters').mockResolvedValue([]);
        render(WorkspaceApp, { client });
        await screen.findByText(t('workspace.emptyLibrary'));
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.settings') }));
        const settings = await screen.findByRole('region', { name: t('navigation.settings') });
        await fireEvent.click(
            within(settings).getByRole('button', { name: t('uiPreview.aiConnection') }),
        );
        const ai = await screen.findByRole('dialog', { name: t('workspaceAi.title') });
        await fireEvent.click(
            within(ai).getByRole('button', { name: t('workspaceAi.connections') }),
        );
        const connections = await screen.findByRole('dialog', {
            name: t('workspaceAi.connections'),
        });
        await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([connections]));
        for (const parent of [settings, ai]) {
            expect(parent.isConnected).toBe(true);
            expect(parent).toHaveProperty('inert', true);
        }
        await fireEvent.click(
            within(connections).getByRole('button', { name: t('uiPreview.back') }),
        );
        await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([ai]));
        expect(ai).toHaveProperty('inert', false);
        expect(settings).toHaveProperty('inert', true);
        await fireEvent.click(within(ai).getByRole('button', { name: t('uiPreview.back') }));
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        expect(settings).toHaveProperty('inert', false);
    });

    it.each(['settings', 'chat'] as const)(
        'retains loaded card images across %s navigation',
        async (destination) => {
            const client = createPreviewClient();
            const [character] = await client.listCharacters();
            if (!character) throw new Error('Character fixture missing');
            const digest = 'ab'.repeat(32);
            vi.spyOn(client, 'listCharacters').mockResolvedValue([
                { ...character, avatar_asset_id: digest },
            ]);
            const descriptor: AssetDeliveryDto = {
                asset_id: 'navigation-avatar',
                sha256: digest,
                kind: 'image',
                media_type: 'image/png',
                size_bytes: 2048,
                width: 640,
                height: 480,
                duration_ms: null,
                url: `lorepia-asset://sha256/${digest}`,
            };
            const resolveAsset = vi.fn().mockResolvedValue(descriptor);
            client.resolveAssetDelivery = resolveAsset;
            const view = render(WorkspaceApp, { client });
            await waitFor(() =>
                expect(
                    view.container.querySelector('.seed-character-thumbnail img'),
                ).not.toBeNull(),
            );
            const image = view.container.querySelector('.seed-character-thumbnail img');
            if (!(image instanceof HTMLImageElement)) throw new Error('History image missing');
            await fireEvent.load(image);
            const originalUrl = image.src;
            const originalCalls = resolveAsset.mock.calls.length;
            const assertImageRetained = () => {
                expect(view.container.querySelector('.seed-character-thumbnail img')).toBe(image);
                expect(image).toHaveAttribute('src', originalUrl);
                expect(image.closest('.trusted-asset')).toHaveAttribute(
                    'data-asset-phase',
                    'ready',
                );
                expect(resolveAsset).toHaveBeenCalledTimes(originalCalls);
            };

            if (destination === 'chat') {
                const [conversation] = await client.listConversations(character.id);
                if (!conversation) throw new Error('Conversation fixture missing');
                await fireEvent.click(screen.getByRole('button', { name: t('navigation.chats') }));
                for (let attempt = 0; attempt < 3; attempt += 1) {
                    await fireEvent.click(
                        screen.getByRole('button', {
                            name: new RegExp('^' + conversation.title + ' ·'),
                        }),
                    );
                    await screen.findByRole('textbox', { name: t('uiPreview.message') });
                    assertImageRetained();
                    await fireEvent.keyDown(window, { key: 'Escape' });
                    await screen.findByRole('region', { name: t('uiPreview.history') });
                    assertImageRetained();
                }
                return;
            }

            await fireEvent.click(screen.getByRole('button', { name: t('navigation.settings') }));
            const settings = await screen.findByRole('region', {
                name: t('navigation.settings'),
            });
            assertImageRetained();
            await fireEvent.click(
                within(settings).getByRole('button', { name: t('settings.section.plugins.title') }),
            );
            const plugins = await screen.findByRole('dialog', { name: t('settingsUi.plugins') });
            await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([plugins]));
            expect(settings.isConnected).toBe(true);
            expect(settings).toHaveProperty('inert', true);
            assertImageRetained();

            await fireEvent.click(
                within(plugins).getByRole('button', { name: t('uiPreview.back') }),
            );
            await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
            expect(settings).toHaveProperty('inert', false);
            assertImageRetained();
            await fireEvent.click(screen.getByRole('button', { name: t('navigation.home') }));
            await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
            expect(
                screen.getByRole('button', {
                    name: t('uiPreview.cardSelect', { name: character.name }),
                }),
            ).toBeVisible();
            assertImageRetained();
        },
    );
});

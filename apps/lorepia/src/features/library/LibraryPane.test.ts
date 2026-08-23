import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
    INITIAL_APP_STATE,
    LorepiaAppController,
    type LorepiaAppState,
} from '../../app/app-controller';
import type { LorepiaClient } from '../../lib/ipc/contracts';

const tauriMocks = vi.hoisted(() => ({
    convertFileSrc: vi.fn<(filePath: string, protocol?: string) => string>(),
}));

vi.mock('@tauri-apps/api/core', () => tauriMocks);

import LibraryPane from './LibraryPane.svelte';

beforeEach(() => {
    tauriMocks.convertFileSrc.mockReset();
    tauriMocks.convertFileSrc.mockImplementation(
        (filePath: string, protocol = 'asset') =>
            `http://${protocol}.localhost/${encodeURIComponent(filePath)}`,
    );
});

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

function libraryState(): LorepiaAppState {
    const state: LorepiaAppState = structuredClone(INITIAL_APP_STATE);
    const character = {
        id: 'character-1',
        name: '라온',
        description: '합성 캐릭터',
        source_hash: 'synthetic',
        avatar_asset_id: null,
        created_at: '2026-08-03T00:00:00Z',
    };
    state.library = { phase: 'ready', error: null, characters: [character] };
    /* Export acts on the open character, so the list has one open. */
    state.selected_character = character;
    return state;
}

function deferred<Value>(): {
    promise: Promise<Value>;
    resolve: (value: Value) => void;
} {
    let resolvePromise!: (value: Value) => void;
    const promise = new Promise<Value>((resolve) => {
        resolvePromise = resolve;
    });
    return { promise, resolve: resolvePromise };
}

describe('LibraryPane safe local sources', () => {
    it('uses the Telegram Contacts action position on an empty home', () => {
        const state = structuredClone(INITIAL_APP_STATE);
        state.library = { phase: 'ready', error: null, characters: [] };
        const controller = new LorepiaAppController({} as LorepiaClient);

        const rendered = render(LibraryPane, {
            state,
            controller,
            client: {} as LorepiaClient,
            onOpenConversations: () => undefined,
            rootView: true,
        });

        expect(screen.queryByText('아직 캐릭터가 없습니다.')).not.toBeInTheDocument();
        expect(screen.getAllByRole('button', { name: '새 캐릭터 추가' })).toHaveLength(1);
        const action = screen.getByRole('button', { name: '새 캐릭터 추가' });
        expect(action).toHaveClass('mobile-root-contact-button');
        expect(action).not.toHaveClass('mobile-root-fab');
        expect(rendered.container.querySelector('.mobile-root-contact-action')).toContainElement(
            action,
        );
        expect(rendered.container.querySelector('.library-empty')).not.toBeInTheDocument();
        expect(
            screen.queryByText('캐릭터를 추가하면 바로 새로운 대화를 시작할 수 있어요.'),
        ).not.toBeInTheDocument();
        controller.destroy();
    });

    it('filters the local character list from the persistent home search field', async () => {
        const state = libraryState();
        state.library.characters.push({
            id: 'character-2',
            name: '세라',
            description: '별을 읽는 항해사',
            source_hash: 'synthetic-2',
            avatar_asset_id: null,
            created_at: '2026-08-03T00:00:00Z',
        });
        const controller = new LorepiaAppController({} as LorepiaClient);

        render(LibraryPane, {
            state,
            controller,
            client: {} as LorepiaClient,
            onOpenConversations: () => undefined,
        });

        const search = screen.getByRole('searchbox', { name: '캐릭터 검색' });
        await fireEvent.input(search, { target: { value: '항해' } });

        expect(screen.getByRole('button', { name: /세라/ })).toBeVisible();
        expect(screen.queryByRole('button', { name: /라온 합성 캐릭터/ })).not.toBeInTheDocument();
        expect(
            screen.queryByRole('button', { name: '라온 캐릭터 소스 내보내기' }),
        ).not.toBeInTheDocument();
        controller.destroy();
    });

    it('resolves a character avatar through the opaque asset command', async () => {
        const sha256 = 'ab'.repeat(32);
        const state: LorepiaAppState = structuredClone(INITIAL_APP_STATE);
        state.library = {
            phase: 'ready',
            error: null,
            characters: [
                {
                    id: 'character-1',
                    name: '라온',
                    description: '합성 캐릭터',
                    source_hash: 'synthetic',
                    avatar_asset_id: 'avatar-1',
                    created_at: '2026-08-03T00:00:00Z',
                },
            ],
        };
        const resolveAssetDelivery = vi.fn().mockResolvedValue({
            asset_id: 'avatar-1',
            sha256,
            media_type: 'image/png',
            kind: 'image',
            size_bytes: 1024,
            width: 64,
            height: 64,
            duration_ms: null,
            url: `lorepia-asset://sha256/${sha256}`,
        });
        const client = { resolveAssetDelivery } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);

        render(LibraryPane, {
            state,
            controller,
            client,
            onOpenConversations: () => undefined,
        });

        const image = await screen.findByRole('img', { name: '라온 캐릭터 이미지' });
        expect(resolveAssetDelivery).toHaveBeenCalledWith({
            selector: { kind: 'asset_id', asset_id: 'avatar-1' },
        });
        expect(tauriMocks.convertFileSrc).toHaveBeenCalledWith(sha256, 'lorepia-asset');
        expect(image).toHaveAttribute('src', `http://lorepia-asset.localhost/sha256/${sha256}`);
        expect(document.body.textContent).not.toContain('/Users/');
        expect(document.body.textContent).not.toContain('payload');

        controller.destroy();
    });

    it('exports a committed character by durable ID and shows only safe receipt evidence', async () => {
        const exportContentSource = vi.fn().mockResolvedValue({
            kind: 'character_card_v3',
            source_id: 'character-1',
            sha256: 'cd'.repeat(32),
            size_bytes: 2048,
            file_name: 'raon.card.json',
            path: '/private/should-not-cross',
        });
        const client = { exportContentSource } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);

        render(LibraryPane, {
            state: libraryState(),
            controller,
            client,
            onOpenConversations: () => undefined,
        });
        await fireEvent.click(screen.getByRole('button', { name: '라온 캐릭터 소스 내보내기' }));

        expect(exportContentSource).toHaveBeenCalledWith({
            kind: 'character_source',
            character_id: 'character-1',
        });
        expect(await screen.findByRole('heading', { name: '최근 캐릭터 내보내기' })).toBeVisible();
        expect(screen.getByText('파일명 raon.card.json')).toBeVisible();
        expect(screen.getByText('크기 2048바이트')).toBeVisible();
        expect(screen.getByText('cd'.repeat(32))).toBeVisible();
        expect(document.body.textContent).not.toContain('/private/');
        expect(document.body.textContent).not.toContain('bytes');

        controller.destroy();
    });

    it('blocks duplicate character exports and treats native picker cancellation as neutral', async () => {
        const pending = deferred<null>();
        const exportContentSource = vi.fn(() => pending.promise);
        const client = { exportContentSource } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);

        render(LibraryPane, {
            state: libraryState(),
            controller,
            client,
            onOpenConversations: () => undefined,
        });
        const exportButton = screen.getByRole('button', { name: '라온 캐릭터 소스 내보내기' });
        await fireEvent.click(exportButton);
        expect(exportButton).toBeDisabled();
        await fireEvent.click(exportButton);
        expect(exportContentSource).toHaveBeenCalledTimes(1);

        pending.resolve(null);
        await waitFor(() => expect(exportButton).toBeEnabled());
        expect(screen.queryByRole('alert')).not.toBeInTheDocument();
        expect(screen.queryByRole('heading', { name: '최근 캐릭터 내보내기' })).toBeNull();

        controller.destroy();
    });

    it('shows a safe visible error when character export fails', async () => {
        const client = {
            exportContentSource: vi.fn().mockRejectedValue(new Error('private host failure')),
        } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);

        render(LibraryPane, {
            state: libraryState(),
            controller,
            client,
            onOpenConversations: () => undefined,
        });
        await fireEvent.click(screen.getByRole('button', { name: '라온 캐릭터 소스 내보내기' }));

        expect(await screen.findByRole('alert')).toHaveTextContent(
            '캐릭터 소스를 내보내지 못했습니다.',
        );
        expect(document.body.textContent).not.toContain('private host failure');

        controller.destroy();
    });

    it('rejects a zero-byte success receipt instead of reporting delivery', async () => {
        const client = {
            exportContentSource: vi.fn().mockResolvedValue({
                kind: 'character_card_v3',
                source_id: 'character-1',
                sha256: 'ab'.repeat(32),
                size_bytes: 0,
                file_name: 'character.json',
            }),
        } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);

        render(LibraryPane, {
            state: libraryState(),
            controller,
            client,
            onOpenConversations: () => undefined,
        });
        await fireEvent.click(screen.getByRole('button', { name: '라온 캐릭터 소스 내보내기' }));

        expect(await screen.findByRole('alert')).toHaveTextContent(
            'Core 내보내기 영수증이 선택한 캐릭터와 일치하지 않습니다.',
        );
        expect(screen.queryByRole('heading', { name: '최근 캐릭터 내보내기' })).toBeNull();

        controller.destroy();
    });
});

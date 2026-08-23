import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { BootstrapDto, CharacterDto, LorepiaClient } from '../lib/ipc/contracts';
import App from './App.svelte';

const BOOTSTRAP: BootstrapDto = {
    shell_api_version: 2,
    core_api_version: 9,
    chat_event_version: 4,
    health: {
        core_version: '0.1.0',
        database_open: true,
        schema_version: 1,
        data_root_writable: true,
        staging_writable: true,
        recovery_pending: false,
        active_jobs: 0,
    },
};

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

function appClient(
    bootstrapSnapshot: LorepiaClient['bootstrapSnapshot'],
    listPendingContentPackageImports = vi.fn().mockResolvedValue([]),
    listCompletedContentPackageExports = vi.fn().mockResolvedValue([]),
    listCharacters: LorepiaClient['listCharacters'] = vi.fn().mockResolvedValue([]),
): LorepiaClient {
    return {
        bootstrapSnapshot,
        listCharacters,
        getProviderOverview: vi.fn().mockResolvedValue({
            templates: [],
            connections: [],
            legacy_profiles: [],
            settings: {
                preserve_partial_generations: true,
                selected_provider_profile_id: null,
                selected_model_route_id: null,
                selected_generation_preset_id: null,
            },
        }),
        listProviderDiscoveries: vi.fn().mockResolvedValue([]),
        providerCatalogStatus: vi.fn().mockResolvedValue({
            status_schema_version: 1,
            state_version: 1,
            active_revision: 1,
            active_snapshot_sha256: 'synthetic-active',
            bundled_baseline_sha256: 'synthetic-baseline',
            snapshot_count: 1,
            signed_update_count: 0,
            highest_accepted_revision: 1,
            latest_issued_at: null,
            active_signed_revisions: [],
        }),
        providerCatalogHistory: vi.fn().mockResolvedValue({
            history_schema_version: 1,
            active_revision: 1,
            revisions: [],
            activations: [],
            next_before_revision: null,
            next_before_state_version: null,
        }),
        subscribeMemorySupervisorStatus: vi.fn().mockResolvedValue(() => undefined),
        getMemorySupervisorStatus: vi.fn().mockResolvedValue({
            sequence: 1,
            phase: 'running',
            recovered_interrupted_jobs: 0,
            completed_jobs: 0,
        }),
        listPendingContentPackageImports,
        listCompletedContentPackageExports,
    } as unknown as LorepiaClient;
}

function setViewportWidth(width: number): (nextWidth: number) => void {
    let viewportWidth = width;
    vi.spyOn(window, 'matchMedia').mockImplementation((query: string) => {
        const minimum = /\(min-width:\s*(\d+)px\)/.exec(query);
        return {
            get matches() {
                return minimum === null || viewportWidth >= Number(minimum[1]);
            },
            media: query,
            onchange: null,
            addEventListener: () => undefined,
            removeEventListener: () => undefined,
            addListener: () => undefined,
            removeListener: () => undefined,
            dispatchEvent: () => false,
        };
    });
    return (nextWidth: number): void => {
        viewportWidth = nextWidth;
        window.dispatchEvent(new Event('resize'));
    };
}

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe('App responsive shell', () => {
    it.each([880, 393] as const)(
        'keeps the %s-point Tauri window on the mobile layout',
        async (width) => {
            setViewportWidth(width);
            const rendered = render(App, {
                client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
            });

            await screen.findByRole('heading', { name: '캐릭터' });
            expect(screen.queryByText('로컬 Core')).not.toBeInTheDocument();
            expect(rendered.container.querySelector('.app-shell')).toHaveAttribute(
                'data-layout',
                'mobile',
            );
            expect(rendered.container.querySelector('.sidebar')).not.toBeInTheDocument();
            expect(screen.getByRole('navigation', { name: '주요 화면' })).toHaveClass('tab-bar');
            expect(screen.getAllByRole('button', { name: '새 캐릭터 추가' })).toHaveLength(1);
        },
    );

    it('keeps the floating navigation on the mobile chat-list root', async () => {
        setViewportWidth(393);
        render(App, {
            client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
        });

        await screen.findByRole('heading', { name: '캐릭터' });
        await fireEvent.click(screen.getByRole('button', { name: '채팅' }));

        expect(screen.getByRole('heading', { name: '채팅' })).toBeVisible();
        expect(screen.getByRole('searchbox', { name: '대화 검색' })).toBeVisible();
        expect(screen.getByRole('navigation', { name: '주요 화면' })).toBeVisible();
        expect(screen.queryByRole('button', { name: '새 대화' })).not.toBeInTheDocument();
    });

    it('keeps an empty wide-window chat destination on the mobile chat-list root', async () => {
        setViewportWidth(880);
        const rendered = render(App, {
            client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
        });

        await screen.findByRole('heading', { name: '캐릭터' });
        expect(rendered.container.querySelector('.app-shell')).toHaveAttribute(
            'data-layout',
            'mobile',
        );

        await fireEvent.click(screen.getByRole('button', { name: '채팅' }));

        await screen.findByRole('searchbox', { name: '대화 검색' });
        expect(screen.getByRole('navigation', { name: '주요 화면' })).toBeVisible();
    });

    it('restores the desktop sidebar and workspace at desktop width', async () => {
        setViewportWidth(1180);
        const rendered = render(App, {
            client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
        });

        await waitFor(() => {
            expect(rendered.container.querySelector('.app-shell')).toHaveAttribute(
                'data-layout',
                'desktop',
            );
        });
        expect(rendered.container.querySelector('.sidebar')).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();
        expect(screen.getByRole('heading', { name: 'LorePia' })).toBeVisible();
        expect(screen.queryByText('로컬 Core')).not.toBeInTheDocument();
    });

    it('switches between phone and desktop structures when the Tauri window resizes', async () => {
        const resizeTo = setViewportWidth(393);
        const rendered = render(App, {
            client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
        });

        await screen.findByRole('heading', { name: '캐릭터' });
        resizeTo(1180);
        await waitFor(() => {
            expect(rendered.container.querySelector('.app-shell')).toHaveAttribute(
                'data-layout',
                'desktop',
            );
        });
        expect(rendered.container.querySelector('.sidebar')).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

        resizeTo(393);
        await waitFor(() => {
            expect(rendered.container.querySelector('.app-shell')).toHaveAttribute(
                'data-layout',
                'mobile',
            );
        });
        await waitFor(() => {
            expect(rendered.container.querySelector('.sidebar')).not.toBeInTheDocument();
        });
        expect(rendered.container.querySelector('.tab-bar')).toBeVisible();
    });

    it('uses the shared mobile root header for the create title', async () => {
        setViewportWidth(393);
        const rendered = render(App, {
            client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
        });

        await screen.findByRole('heading', { name: '캐릭터' });
        await fireEvent.click(screen.getByRole('button', { name: '생성' }));

        const title = await screen.findByRole('heading', { name: '창작 스튜디오' });
        expect(title.closest('.mobile-top-bar.mobile-root-header')).not.toBeNull();
        expect(rendered.container.querySelector('.studio-index-header')).toBeNull();
    });

    it('opens a grouped create destination as a full screen with the standard back action', async () => {
        setViewportWidth(393);
        const rendered = render(App, {
            client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
        });

        await screen.findByRole('heading', { name: '캐릭터' });
        await fireEvent.click(screen.getByRole('button', { name: '생성' }));
        await fireEvent.click(await screen.findByRole('button', { name: '프롬프트 설계' }));

        const back = screen.getByRole('button', { name: '뒤로' });
        const detailHeader = back.closest('.sub-header');
        expect(detailHeader).not.toBeNull();
        expect(
            within(detailHeader as HTMLElement).getByRole('heading', { name: '프롬프트' }),
        ).toBeVisible();
        const detailScroll = rendered.container.querySelector('.studio-detail-scroll');
        expect(detailScroll).not.toBeNull();
        expect(
            (detailScroll as HTMLElement).style.getPropertyValue('--mobile-top-mask-alpha'),
        ).toBe('1');
        expect(
            within(detailScroll as HTMLElement).queryByRole('heading', { name: '프롬프트' }),
        ).not.toBeInTheDocument();
        Object.defineProperty(detailScroll, 'scrollTop', {
            configurable: true,
            value: 24,
            writable: true,
        });
        await fireEvent.scroll(detailScroll as HTMLElement);
        expect(
            (detailScroll as HTMLElement).style.getPropertyValue('--mobile-top-mask-alpha'),
        ).toBe('0.5');
        (detailScroll as HTMLElement).scrollTop = 0;
        await fireEvent.scroll(detailScroll as HTMLElement);
        expect(
            (detailScroll as HTMLElement).style.getPropertyValue('--mobile-top-mask-alpha'),
        ).toBe('1');
        expect(screen.getByRole('region', { name: '프롬프트' })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

        await fireEvent.click(back);
        expect(screen.getByRole('button', { name: '프롬프트 설계' })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).toBeVisible();
    });

    it('enters a full settings screen with the standard back action and fixed tabs', async () => {
        setViewportWidth(393);
        const rendered = render(App, {
            client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
        });

        await screen.findByRole('heading', { name: '캐릭터' });
        await fireEvent.click(screen.getByRole('button', { name: '설정' }));
        await fireEvent.click(await screen.findByRole('button', { name: /화면 모드/ }));

        expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
        const back = screen.getByRole('button', { name: '뒤로' });
        const detailHeader = back.closest('.sub-header');
        expect(detailHeader).not.toBeNull();
        expect(
            within(detailHeader as HTMLElement).getByRole('heading', { name: '화면 모드' }),
        ).toBeVisible();
        const detailScroll = rendered.container.querySelector('.settings-detail-scroll');
        expect(detailScroll).not.toBeNull();
        expect(
            within(detailScroll as HTMLElement).queryByRole('heading', { name: '화면 모드' }),
        ).not.toBeInTheDocument();
        Object.defineProperty(detailScroll, 'scrollTop', {
            configurable: true,
            value: 48,
            writable: true,
        });
        await fireEvent.scroll(detailScroll as HTMLElement);
        expect(
            (detailScroll as HTMLElement).style.getPropertyValue('--mobile-top-mask-alpha'),
        ).toBe('0');
        (detailScroll as HTMLElement).scrollTop = 0;
        await fireEvent.scroll(detailScroll as HTMLElement);
        expect(
            (detailScroll as HTMLElement).style.getPropertyValue('--mobile-top-mask-alpha'),
        ).toBe('1');
        expect(screen.getByRole('region', { name: '화면 모드' })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).toBeVisible();

        await fireEvent.click(back);
        expect(screen.getByRole('button', { name: /화면 모드/ })).toBeVisible();
    });
});

describe('App bootstrap content-package recovery', () => {
    it('restores as soon as bootstrap succeeds while unrelated startup work is still pending', async () => {
        const bootstrap = deferred<BootstrapDto>();
        const library = deferred<CharacterDto[]>();
        const bootstrapSnapshot = vi.fn(() => bootstrap.promise);
        const listCharacters = vi.fn(() => library.promise);
        const listPendingContentPackageImports = vi.fn().mockResolvedValue([]);
        const listCompletedContentPackageExports = vi.fn().mockResolvedValue([]);
        render(App, {
            client: appClient(
                bootstrapSnapshot,
                listPendingContentPackageImports,
                listCompletedContentPackageExports,
                listCharacters,
            ),
        });

        await waitFor(() => expect(bootstrapSnapshot).toHaveBeenCalledOnce());
        bootstrap.resolve(BOOTSTRAP);

        await screen.findByRole('heading', { name: '캐릭터' });
        expect(listCharacters).toHaveBeenCalledOnce();
        await waitFor(() => {
            expect(listPendingContentPackageImports).toHaveBeenCalledOnce();
            expect(listCompletedContentPackageExports).toHaveBeenCalledOnce();
        });

        library.resolve([]);
    });

    it('waits for successful bootstrap before restoring pending imports and completed exports', async () => {
        const bootstrap = deferred<BootstrapDto>();
        const bootstrapSnapshot = vi.fn(() => bootstrap.promise);
        const listPendingContentPackageImports = vi.fn().mockResolvedValue([]);
        const listCompletedContentPackageExports = vi.fn().mockResolvedValue([]);
        render(App, {
            client: appClient(
                bootstrapSnapshot,
                listPendingContentPackageImports,
                listCompletedContentPackageExports,
            ),
        });

        await waitFor(() => expect(bootstrapSnapshot).toHaveBeenCalledOnce());
        expect(listPendingContentPackageImports).not.toHaveBeenCalled();
        expect(listCompletedContentPackageExports).not.toHaveBeenCalled();

        bootstrap.resolve(BOOTSTRAP);

        await waitFor(() => {
            expect(listPendingContentPackageImports).toHaveBeenCalledOnce();
            expect(listCompletedContentPackageExports).toHaveBeenCalledOnce();
        });
        expect(listPendingContentPackageImports).toHaveBeenCalledWith({ limit: 100 });
        expect(listCompletedContentPackageExports).toHaveBeenCalledWith({ limit: 100 });
    });

    it('restores only after a failed bootstrap is retried successfully', async () => {
        const bootstrapSnapshot = vi
            .fn<LorepiaClient['bootstrapSnapshot']>()
            .mockRejectedValueOnce(new Error('synthetic cold-start failure'))
            .mockResolvedValueOnce(BOOTSTRAP);
        const listPendingContentPackageImports = vi.fn().mockResolvedValue([]);
        const listCompletedContentPackageExports = vi.fn().mockResolvedValue([]);
        render(App, {
            client: appClient(
                bootstrapSnapshot,
                listPendingContentPackageImports,
                listCompletedContentPackageExports,
            ),
        });

        const retry = await screen.findByRole('button', { name: '다시 시도' });
        expect(listPendingContentPackageImports).not.toHaveBeenCalled();
        expect(listCompletedContentPackageExports).not.toHaveBeenCalled();

        await fireEvent.click(retry);

        await waitFor(() => {
            expect(bootstrapSnapshot).toHaveBeenCalledTimes(2);
            expect(listPendingContentPackageImports).toHaveBeenCalledOnce();
            expect(listCompletedContentPackageExports).toHaveBeenCalledOnce();
        });
    });
});

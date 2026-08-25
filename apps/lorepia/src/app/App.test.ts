import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    BootstrapDto,
    CharacterDto,
    LorepiaClient,
    ProviderTemplateDto,
} from '../lib/ipc/contracts';
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

const TEST_PERSONA = {
    value: {
        id: 'persona-1',
        name: '테스트 페르소나',
        description: '테스트 설명',
    },
    revision: 1,
    revision_id: 'persona-revision-1',
    created_at: '2026-08-23T00:00:00Z',
    updated_at: '2026-08-23T00:00:00Z',
};

const TEST_PROVIDER_TEMPLATE: ProviderTemplateDto = {
    id: 'synthetic-template',
    display_name: 'Synthetic API',
    manifest_version: 2,
    source: 'bundled',
    api_family: 'open_ai_responses',
    connection_fields: [],
    default_network_mode: 'public',
    default_api_origin: 'https://api.synthetic.invalid',
    credential_required: true,
    supports_model_listing: true,
    auth_binding: { kind: 'bearer_header' },
    parameters: [],
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
        listConversations: vi.fn().mockResolvedValue([]),
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
        createPersona: vi.fn().mockResolvedValue(TEST_PERSONA),
        updatePersona: vi.fn().mockResolvedValue(TEST_PERSONA),
        getPersona: vi.fn().mockResolvedValue(TEST_PERSONA),
        listPersonas: vi.fn().mockResolvedValue([TEST_PERSONA]),
        listPersonaPage: vi.fn().mockResolvedValue({
            kind: 'page',
            catalog_revision: 'a'.repeat(64),
            items: [TEST_PERSONA],
            next_cursor: null,
        }),
        deletePersona: vi.fn().mockResolvedValue({
            persona_id: TEST_PERSONA.value.id,
            revision: TEST_PERSONA.revision,
            deleted_at: '2026-08-23T00:01:00Z',
        }),
        getConversationPersonaSelection: vi.fn().mockResolvedValue({
            conversation_id: 'conversation-1',
            state_revision: null,
            selected_persona: null,
            updated_at: null,
            cleared_at: null,
        }),
        selectConversationPersona: vi.fn().mockResolvedValue({
            conversation_id: 'conversation-1',
            state_revision: 1,
            selected_persona: null,
            updated_at: '2026-08-23T00:01:00Z',
            cleared_at: null,
        }),
        clearConversationPersona: vi.fn().mockResolvedValue({
            conversation_id: 'conversation-1',
            state_revision: 2,
            selected_persona: null,
            updated_at: '2026-08-23T00:02:00Z',
            cleared_at: '2026-08-23T00:02:00Z',
        }),
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
        expect(screen.queryByRole('searchbox', { name: '대화 검색' })).not.toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '대화 검색' }));
        expect(screen.getByRole('searchbox', { name: '대화 검색' })).toHaveFocus();
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

        await fireEvent.click(screen.getByRole('button', { name: '대화 검색' }));
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
        const logo = rendered.container.querySelector<HTMLElement>(
            '.sidebar-logo .brand-logo-mark',
        );
        expect(logo).toBeInTheDocument();
        expect(logo?.style.getPropertyValue('--logo-mask')).toContain('lorepia-logo-mark');
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
        expect(title.closest('.mobile-top-frame.mobile-root-header')).not.toBeNull();
        expect(rendered.container.querySelector('.studio-index-header')).toBeNull();
    });

    it('pushes a Studio section index before a leaf and pops both levels with the shared back action', async () => {
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
        await waitFor(() =>
            expect(
                within(detailHeader as HTMLElement).getByRole('heading', { name: '프롬프트' }),
            ).toHaveFocus(),
        );
        expect(screen.getByRole('list', { name: '세부 도구' })).toBeVisible();
        expect(screen.getByRole('button', { name: /프롬프트 블록/ })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

        await fireEvent.click(screen.getByRole('button', { name: /프롬프트 블록/ }));

        expect(
            within(detailHeader as HTMLElement).getByRole('heading', { name: '프롬프트 블록' }),
        ).toBeVisible();
        const detailScroll = rendered.container.querySelector('.studio-detail-scroll');
        expect(detailScroll).not.toBeNull();
        expect(
            (detailHeader as HTMLElement).style.getPropertyValue('--mobile-top-fade-progress'),
        ).toBe('0');
        expect((detailScroll as HTMLElement).style.cssText).not.toContain('mask');
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
            (detailHeader as HTMLElement).style.getPropertyValue('--mobile-top-fade-progress'),
        ).toBe('0.5');
        (detailScroll as HTMLElement).scrollTop = 0;
        await fireEvent.scroll(detailScroll as HTMLElement);
        expect(
            (detailHeader as HTMLElement).style.getPropertyValue('--mobile-top-fade-progress'),
        ).toBe('0');
        expect(screen.getByRole('region', { name: '프롬프트' })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

        await fireEvent.click(back);
        expect(
            within(detailHeader as HTMLElement).getByRole('heading', { name: '프롬프트' }),
        ).toBeVisible();
        expect(screen.getByRole('button', { name: /프롬프트 블록/ })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

        await fireEvent.click(back);
        expect(screen.getByRole('button', { name: '프롬프트 설계' })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).toBeVisible();

        await fireEvent.click(screen.getByRole('button', { name: '프롬프트 설계' }));
        await waitFor(() =>
            expect(screen.getByRole('heading', { name: '프롬프트' })).toHaveFocus(),
        );
    });

    it('enters a full settings screen with the standard back action and no root tabs', async () => {
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
            (detailHeader as HTMLElement).style.getPropertyValue('--mobile-top-fade-progress'),
        ).toBe('1');
        (detailScroll as HTMLElement).scrollTop = 0;
        await fireEvent.scroll(detailScroll as HTMLElement);
        expect(
            (detailHeader as HTMLElement).style.getPropertyValue('--mobile-top-fade-progress'),
        ).toBe('0');
        expect(screen.getByRole('region', { name: '화면 모드' })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

        await fireEvent.click(back);
        expect(screen.getByRole('button', { name: /화면 모드/ })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).toBeVisible();
    });

    it.each([
        ['화면 모드', /^\ud654\uba74 \ubaa8\ub4dc /],
        ['페르소나', /^\ud398\ub974\uc18c\ub098 /],
        ['기본 생성 대상', /^\uae30\ubcf8 \uc0dd\uc131 \ub300\uc0c1 /],
        ['연결과 자격증명', /^\uc5f0\uacb0과 \uc790\uaca9\uc99d\uba85 /],
        ['사용 가능한 템플릿', /^\uc0ac\uc6a9 \uac00\ub2a5\ud55c \ud15c\ud50c\ub9bf /],
        ['검색과 동기화', /^\uac80\uc0c9\uacfc \ub3d9\uae30\ud654 /],
        ['제공자 카탈로그', /^\uc81c\uacf5\uc790 \uce74\ud0c8\ub85c\uadf8 /],
        ['고급', /^\uace0\uae09 /],
        ['오픈소스 라이선스', /^\uc624\ud508\uc18c\uc2a4 \ub77c\uc774\uc120\uc2a4 /],
    ] as const)(
        'hides the root tabs for the pushed %s settings destination',
        async (title, rowName) => {
            setViewportWidth(393);
            const rendered = render(App, {
                client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
            });

            await screen.findByRole('heading', { name: '캐릭터' });
            await fireEvent.click(screen.getByRole('button', { name: '설정' }));
            await fireEvent.click(await screen.findByRole('button', { name: rowName }));

            expect(screen.getByRole('heading', { name: title, level: 1 })).toBeVisible();
            expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

            await fireEvent.click(screen.getByRole('button', { name: '뒤로' }));
            expect(await screen.findByRole('button', { name: rowName })).toBeVisible();
            expect(rendered.container.querySelector('.tab-bar')).toBeVisible();
        },
    );

    it.each([
        {
            rootRow: /^\uac80\uc0c9\uacfc \ub3d9\uae30\ud654 /,
            sectionTitle: '검색과 동기화',
            leafRow: /프로바이더 탐색/,
            leafTitle: '프로바이더 탐색',
        },
        {
            rootRow: /^\uace0\uae09 /,
            sectionTitle: '고급',
            leafRow: /연결 관리/,
            leafTitle: '프로바이더 연결',
        },
        {
            rootRow: /^\uc81c\uacf5\uc790 \uce74\ud0c8\ub85c\uadf8 /,
            sectionTitle: '제공자 카탈로그',
            leafRow: /활성 카탈로그/,
            leafTitle: '활성 카탈로그',
        },
    ] as const)(
        'uses the $sectionTitle title and back stack for its leaf route',
        async ({ rootRow, sectionTitle, leafRow, leafTitle }) => {
            setViewportWidth(393);
            const rendered = render(App, {
                client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
            });

            await screen.findByRole('heading', { name: '캐릭터' });
            await fireEvent.click(screen.getByRole('button', { name: '설정' }));
            await fireEvent.click(await screen.findByRole('button', { name: rootRow }));
            expect(screen.getByRole('heading', { name: sectionTitle, level: 1 })).toBeVisible();

            await fireEvent.click(await screen.findByRole('button', { name: leafRow }));
            expect(screen.getByRole('heading', { name: leafTitle, level: 1 })).toBeVisible();
            expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

            await fireEvent.click(screen.getByRole('button', { name: '뒤로' }));
            expect(screen.getByRole('heading', { name: sectionTitle, level: 1 })).toBeVisible();
            expect(await screen.findByRole('button', { name: leafRow })).toBeVisible();
            expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

            await fireEvent.click(screen.getByRole('button', { name: '뒤로' }));
            expect(await screen.findByRole('button', { name: rootRow })).toBeVisible();
            expect(rendered.container.querySelector('.tab-bar')).toBeVisible();
        },
    );

    it('uses the template name as the pushed title and pops to the template list first', async () => {
        setViewportWidth(393);
        const client = appClient(vi.fn().mockResolvedValue(BOOTSTRAP));
        client.getProviderOverview = vi.fn().mockResolvedValue({
            templates: [TEST_PROVIDER_TEMPLATE],
            connections: [],
            legacy_profiles: [],
            settings: {
                preserve_partial_generations: true,
                selected_provider_profile_id: null,
                selected_model_route_id: null,
                selected_generation_preset_id: null,
            },
        });
        const rendered = render(App, { client });

        await screen.findByRole('heading', { name: '캐릭터' });
        await fireEvent.click(screen.getByRole('button', { name: '설정' }));
        await fireEvent.click(await screen.findByRole('button', { name: /^사용 가능한 템플릿 / }));
        expect(screen.getByRole('heading', { name: '사용 가능한 템플릿', level: 1 })).toBeVisible();

        await fireEvent.click(
            await screen.findByRole('button', { name: /Synthetic API open_ai_responses · v2/ }),
        );

        expect(screen.getByRole('heading', { name: 'Synthetic API', level: 1 })).toBeVisible();
        expect(screen.getByRole('region', { name: 'Synthetic API 템플릿 정보' })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

        await fireEvent.click(screen.getByRole('button', { name: '뒤로' }));
        expect(screen.getByRole('heading', { name: '사용 가능한 템플릿', level: 1 })).toBeVisible();
        expect(
            await screen.findByRole('button', { name: /Synthetic API open_ai_responses · v2/ }),
        ).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

        await fireEvent.click(screen.getByRole('button', { name: '뒤로' }));
        expect(await screen.findByRole('button', { name: /^사용 가능한 템플릿 / })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).toBeVisible();
    });

    it('titles a discovery create page and pops back to the discovery list first', async () => {
        setViewportWidth(393);
        const rendered = render(App, {
            client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
        });

        await screen.findByRole('heading', { name: '캐릭터' });
        await fireEvent.click(screen.getByRole('button', { name: '설정' }));
        await fireEvent.click(await screen.findByRole('button', { name: /^검색과 동기화 / }));
        await fireEvent.click(await screen.findByRole('button', { name: /프로바이더 탐색/ }));
        expect(screen.getByRole('heading', { name: '프로바이더 탐색', level: 1 })).toBeVisible();

        await fireEvent.click(screen.getByRole('button', { name: '새 탐색' }));
        expect(screen.getByRole('heading', { name: '새 프로바이더 탐색', level: 1 })).toBeVisible();
        expect(screen.getByRole('form', { name: '프로바이더 탐색 시작' })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();

        await fireEvent.click(screen.getByRole('button', { name: '뒤로' }));
        expect(screen.getByRole('heading', { name: '프로바이더 탐색', level: 1 })).toBeVisible();
        expect(screen.getByRole('button', { name: '새 탐색' })).toBeVisible();
        expect(rendered.container.querySelector('.tab-bar')).not.toBeInTheDocument();
    });

    it('opens the bundled open-source notices from the last settings row', async () => {
        setViewportWidth(393);
        render(App, {
            client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
        });

        await screen.findByRole('heading', { name: '캐릭터' });
        await fireEvent.click(screen.getByRole('button', { name: '설정' }));
        await fireEvent.click(
            await screen.findByRole('button', { name: /오픈소스 라이선스 ISC · MIT/ }),
        );

        const back = screen.getByRole('button', { name: '뒤로' });
        const detailHeader = back.closest('.sub-header');
        expect(detailHeader).not.toBeNull();
        expect(
            within(detailHeader as HTMLElement).getByRole('heading', {
                name: '오픈소스 라이선스',
            }),
        ).toBeVisible();
        expect(screen.getByRole('heading', { name: 'Lucide Icons' })).toBeVisible();
        expect(screen.getByText('상업적 사용')).toBeVisible();

        await fireEvent.click(back);
        expect(
            await screen.findByRole('button', { name: /오픈소스 라이선스 ISC · MIT/ }),
        ).toBeVisible();
    });

    it('returns from a persona editor to its list before leaving the settings section', async () => {
        setViewportWidth(393);
        render(App, {
            client: appClient(vi.fn().mockResolvedValue(BOOTSTRAP)),
        });

        await screen.findByRole('heading', { name: '캐릭터' });
        await fireEvent.click(screen.getByRole('button', { name: '설정' }));
        expect(screen.getByRole('navigation', { name: '주요 화면' })).toBeVisible();
        await fireEvent.click(await screen.findByRole('button', { name: /페르소나 1개/ }));
        expect(screen.queryByRole('navigation', { name: '주요 화면' })).not.toBeInTheDocument();
        const addPersona = within(screen.getByRole('toolbar', { name: '페르소나 작업' })).getByRole(
            'button',
            { name: '페르소나 추가하기' },
        );
        expect(addPersona).toBeVisible();

        await fireEvent.click(addPersona);
        expect(screen.getByRole('heading', { name: '새 페르소나', level: 1 })).toBeVisible();
        expect(screen.getByRole('form', { name: '새 페르소나' })).toBeVisible();
        await fireEvent.click(screen.getByRole('button', { name: '뒤로' }));
        expect(screen.getByRole('heading', { name: '페르소나', level: 1 })).toBeVisible();

        await fireEvent.click(
            await screen.findByRole('button', { name: /테스트 페르소나 테스트 설명/ }),
        );

        expect(screen.getByRole('heading', { name: '페르소나 편집', level: 1 })).toBeVisible();
        expect(screen.getByLabelText('이름')).toHaveValue('테스트 페르소나');
        expect(screen.queryByRole('heading', { name: '저장된 페르소나' })).not.toBeInTheDocument();
        const editBar = screen.getByRole('toolbar', { name: '페르소나 작업' });
        expect(within(editBar).getByRole('button', { name: '저장' })).toBeVisible();
        expect(within(editBar).getByRole('button', { name: '삭제' })).toBeVisible();

        await fireEvent.click(screen.getByRole('button', { name: '뒤로' }));

        expect(screen.queryByRole('heading', { name: '페르소나 편집' })).not.toBeInTheDocument();
        expect(screen.getByRole('heading', { name: '페르소나', level: 1 })).toBeVisible();
        expect(
            await screen.findByRole('button', { name: /테스트 페르소나 테스트 설명/ }),
        ).toBeVisible();
        expect(screen.queryByRole('navigation', { name: '주요 화면' })).not.toBeInTheDocument();
        expect(
            within(screen.getByRole('toolbar', { name: '페르소나 작업' })).getByRole('button', {
                name: '페르소나 추가하기',
            }),
        ).toBeVisible();

        await fireEvent.click(screen.getByRole('button', { name: '뒤로' }));

        expect(await screen.findByRole('button', { name: /페르소나 1개/ })).toBeVisible();
        expect(screen.queryByRole('toolbar', { name: '페르소나 작업' })).not.toBeInTheDocument();
        expect(screen.getByRole('navigation', { name: '주요 화면' })).toBeVisible();
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

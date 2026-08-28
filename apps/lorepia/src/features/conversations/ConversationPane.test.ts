import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
    INITIAL_APP_STATE,
    type LorepiaAppController,
    type LorepiaAppState,
} from '../../app/app-controller';
import type {
    CharacterDto,
    CharacterGreetingCatalogDto,
    ConversationDto,
} from '../../lib/ipc/contracts';
import ConversationPane from './ConversationPane.svelte';

afterEach(cleanup);

const character: CharacterDto = {
    id: 'character-1',
    name: '라온',
    description: '',
    source_hash: 'synthetic',
    avatar_asset_id: null,
    created_at: '2026-08-03T00:00:00Z',
};

const conversation: ConversationDto = {
    id: 'conversation-1',
    character_id: character.id,
    title: '첫 대화',
    created_at: '2026-08-03T00:00:00Z',
    updated_at: '2026-08-03T00:00:00Z',
};

const otherCharacter: CharacterDto = {
    id: 'character-2',
    name: '미오',
    description: '',
    source_hash: 'synthetic-2',
    avatar_asset_id: null,
    created_at: '2026-08-04T00:00:00Z',
};

const otherConversation: ConversationDto = {
    id: 'conversation-2',
    character_id: otherCharacter.id,
    title: '별빛 산책',
    created_at: '2026-08-04T00:00:00Z',
    updated_at: '2026-08-04T00:00:00Z',
};

const catalog: CharacterGreetingCatalogDto = {
    character_id: character.id,
    character_content_revision_id: 'character-revision-7',
    greetings: [
        { id: 'default-enabled', kind: 'default', enabled: true },
        { id: 'alternate-enabled', kind: 'alternate', enabled: true },
        { id: 'alternate-disabled', kind: 'alternate', enabled: false },
    ],
};

function readyState(): LorepiaAppState {
    const state = structuredClone(INITIAL_APP_STATE);
    state.selected_character = character;
    state.conversations = { phase: 'ready', error: null, items: [conversation] };
    state.greeting_catalog = {
        phase: 'ready',
        error: null,
        value: catalog,
        selected_greeting_id: 'default-enabled',
    };
    return state;
}

describe('ConversationPane greeting selector', () => {
    it('keeps an empty chat root visually quiet until a character is selected', () => {
        const state = structuredClone(INITIAL_APP_STATE);
        const controller = {
            selectConversation: vi.fn(() => Promise.resolve(false)),
            openNewConversation: vi.fn(() => Promise.resolve(false)),
        } as unknown as LorepiaAppController;

        const rendered = render(ConversationPane, {
            state,
            controller,
            onOpenChat: vi.fn(),
            rootView: true,
        });

        expect(screen.queryByText('새 대화를 시작해 보세요.')).not.toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '캐릭터 보기' })).not.toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '새 대화' })).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.conversation-empty')).not.toBeInTheDocument();
        expect(
            screen.queryByText(
                '홈에서 캐릭터를 추가하거나 선택하면 대화 목록이 여기에 표시됩니다.',
            ),
        ).not.toBeInTheDocument();
    });

    it('keeps chat creation out of the root header and reserves it for characters', () => {
        const state = readyState();
        state.conversations.items = [];
        const controller = {
            selectConversation: vi.fn(() => Promise.resolve(false)),
            openNewConversation: vi.fn(() => Promise.resolve(false)),
        } as unknown as LorepiaAppController;

        const rendered = render(ConversationPane, {
            state,
            controller,
            onOpenChat: vi.fn(),
            rootView: true,
        });

        const searchAction = screen.getByRole('button', { name: '대화 검색' });
        expect(searchAction).toHaveClass('mobile-top-action');
        expect(rendered.container.querySelector('.mobile-root-actions')).toContainElement(
            searchAction,
        );
        expect(screen.queryByRole('button', { name: '새 대화' })).not.toBeInTheDocument();
        expect(screen.queryByText('저장된 대화가 없습니다.')).not.toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '대화 시작' })).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.conversation-empty')).not.toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '설정' })).not.toBeInTheDocument();
    });

    it('focuses the Telegram-style search shortcut and previews the latest selected message', async () => {
        const state = readyState();
        state.selected_conversation = conversation;
        state.messages = {
            phase: 'ready',
            error: null,
            items: [
                {
                    id: 'message-1',
                    conversation_id: conversation.id,
                    parent_id: null,
                    role: 'assistant',
                    content: '마지막 메시지\n미리보기',
                    status: 'complete',
                    generation_id: 'generation-1',
                    created_at: '2026-08-03T00:00:00Z',
                },
            ],
        };
        const controller = {
            selectConversation: vi.fn(() => Promise.resolve(false)),
            openNewConversation: vi.fn(() => Promise.resolve(false)),
        } as unknown as LorepiaAppController;

        const rendered = render(ConversationPane, {
            state,
            controller,
            onOpenChat: vi.fn(),
            rootView: true,
        });

        expect(screen.queryByRole('searchbox', { name: '대화 검색' })).not.toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '대화 검색' }));
        const search = screen.getByRole('searchbox', { name: '대화 검색' });
        expect(search).toHaveFocus();
        const rootHeader = rendered.container.querySelector('.conversation-root-header');
        const filterStrip = rendered.container.querySelector('.conversation-filter-strip');
        expect(rootHeader).toContainElement(search);
        expect(rootHeader?.nextElementSibling).toBe(filterStrip);
        expect(rendered.container.querySelector('.mobile-root-search')).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.mobile-root-actions')).toHaveAttribute(
            'aria-hidden',
            'true',
        );
        expect(screen.getByRole('button', { name: '검색 닫기' })).toHaveClass(
            'conversation-search-close',
        );
        expect(screen.getByText('마지막 메시지 미리보기')).toBeVisible();

        await fireEvent.keyDown(search, { key: 'Escape' });
        expect(screen.queryByRole('searchbox', { name: '대화 검색' })).not.toBeInTheDocument();
        expect(screen.getByRole('heading', { name: '채팅' })).toBeVisible();
        expect(rendered.container.querySelector('.mobile-root-actions')).toHaveAttribute(
            'aria-hidden',
            'false',
        );
    });

    it('finishes the animated search collapse before restoring the header controls', async () => {
        vi.useFakeTimers();
        const getAnimationsDescriptor = Object.getOwnPropertyDescriptor(
            Element.prototype,
            'getAnimations',
        );
        Object.defineProperty(Element.prototype, 'getAnimations', {
            configurable: true,
            value: vi.fn(() => []),
        });

        try {
            const state = readyState();
            const controller = {
                selectConversation: vi.fn(() => Promise.resolve(false)),
                openNewConversation: vi.fn(() => Promise.resolve(false)),
            } as unknown as LorepiaAppController;
            const rendered = render(ConversationPane, {
                state,
                controller,
                onOpenChat: vi.fn(),
                rootView: true,
            });

            await fireEvent.click(screen.getByRole('button', { name: '대화 검색' }));
            await fireEvent.click(screen.getByRole('button', { name: '검색 닫기' }));

            expect(rendered.container.querySelector('.conversation-top-search')).toHaveClass(
                'closing',
            );
            expect(screen.getByRole('searchbox', { name: '대화 검색' })).toBeInTheDocument();

            await vi.advanceTimersByTimeAsync(380);

            expect(screen.queryByRole('searchbox', { name: '대화 검색' })).not.toBeInTheDocument();
            expect(rendered.container.querySelector('.mobile-root-actions')).toHaveAttribute(
                'aria-hidden',
                'false',
            );
        } finally {
            if (getAnimationsDescriptor === undefined) {
                Reflect.deleteProperty(Element.prototype, 'getAnimations');
            } else {
                Object.defineProperty(Element.prototype, 'getAnimations', getAnimationsDescriptor);
            }
            vi.useRealTimers();
        }
    });

    it('loads the global conversation index and filters it with character pills', async () => {
        const state = readyState();
        state.library = {
            phase: 'ready',
            error: null,
            characters: [character, otherCharacter],
        };
        const listConversations = vi.fn().mockResolvedValue([conversation, otherConversation]);
        const selectCharacter = vi.fn().mockResolvedValue(undefined);
        const selectConversation = vi.fn().mockResolvedValue(true);
        const onOpenChat = vi.fn();
        const controller = {
            selectCharacter,
            selectConversation,
            openNewConversation: vi.fn(() => Promise.resolve(false)),
        } as unknown as LorepiaAppController;

        render(ConversationPane, {
            state,
            controller,
            client: { listConversations },
            onOpenChat,
            rootView: true,
        });

        await screen.findByRole('tab', { name: '미오' });
        expect(listConversations).toHaveBeenCalledWith(null);
        expect(screen.getByRole('button', { name: /첫 대화/ })).toBeVisible();
        expect(screen.getByRole('button', { name: /별빛 산책/ })).toBeVisible();
        expect(document.querySelector('.conversation-filter-count')).not.toBeInTheDocument();

        await fireEvent.click(screen.getByRole('tab', { name: '미오' }));
        expect(screen.queryByRole('button', { name: /첫 대화/ })).not.toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: /별빛 산책/ }));

        await waitFor(() => {
            expect(selectCharacter).toHaveBeenCalledWith(otherCharacter);
            expect(selectConversation).toHaveBeenCalledWith(otherConversation);
            expect(onOpenChat).toHaveBeenCalledOnce();
        });
    });

    it('exposes new conversation as the mobile primary action without changing its gate', () => {
        const controller = {
            selectGreeting: vi.fn(() => true),
            openNewConversation: vi.fn(() => Promise.resolve(true)),
            selectConversation: vi.fn(() => Promise.resolve(true)),
        } as unknown as LorepiaAppController;

        render(ConversationPane, {
            state: readyState(),
            controller,
            onOpenChat: vi.fn(),
        });

        const action = screen.getByRole('button', { name: '새 대화' });
        expect(action).toHaveClass('new-conversation-button');
        expect(action.querySelector('svg.new-conversation-icon')).toBeInTheDocument();
    });

    it('renders only greeting ID/kind metadata and navigates only after successful opens', async () => {
        const selectGreeting = vi.fn(() => true);
        const openNewConversation = vi.fn(() => Promise.resolve(true));
        const selectConversation = vi.fn(() => Promise.resolve(true));
        const onOpenChat = vi.fn();
        const controller = {
            selectGreeting,
            openNewConversation,
            selectConversation,
        } as unknown as LorepiaAppController;

        render(ConversationPane, {
            state: readyState(),
            controller,
            onOpenChat,
        });

        const selector = screen.getByLabelText('시작 인사');
        expect(selector).toHaveAttribute('role', 'combobox');
        expect(selector).toHaveTextContent('default-enabled · 기본');
        await fireEvent.click(selector);
        expect(screen.getByRole('option', { name: 'default-enabled · 기본' })).toBeEnabled();
        expect(
            screen.getByRole('option', {
                name: 'alternate-disabled · 대체 · 비활성',
            }),
        ).toBeDisabled();
        expect(
            screen.getByText('인사 본문은 UI로 전달하지 않으며 ID와 종류만 선택합니다.'),
        ).toBeVisible();

        await fireEvent.click(screen.getByRole('option', { name: 'alternate-enabled · 대체' }));
        expect(selectGreeting).toHaveBeenCalledWith('alternate-enabled');

        await fireEvent.click(screen.getByRole('button', { name: '새 대화' }));
        expect(openNewConversation).toHaveBeenCalledOnce();
        expect(onOpenChat).toHaveBeenCalledOnce();

        const conversationButton = screen.getByRole('button', { name: /첫 대화/ });
        expect(conversationButton.querySelector('[aria-hidden="true"]')).not.toBeInTheDocument();
        await fireEvent.click(conversationButton);
        expect(selectConversation).toHaveBeenCalledWith(conversation);
        expect(onOpenChat).toHaveBeenCalledTimes(2);
    });

    it('disables new-room entry until the exact greeting catalog is ready', () => {
        const state = readyState();
        state.greeting_catalog = {
            phase: 'loading',
            error: null,
            value: null,
            selected_greeting_id: null,
        };
        const controller = {
            selectGreeting: vi.fn(() => false),
            openNewConversation: vi.fn(() => Promise.resolve(false)),
            selectConversation: vi.fn(() => Promise.resolve(false)),
        } as unknown as LorepiaAppController;

        render(ConversationPane, {
            state,
            controller,
            onOpenChat: vi.fn(),
        });

        expect(screen.getByRole('button', { name: '새 대화' })).toBeDisabled();
        expect(screen.getByRole('status')).toHaveTextContent('시작 인사 ID를 불러오는 중입니다.');
        expect(screen.queryByLabelText('시작 인사')).not.toBeInTheDocument();
    });
});

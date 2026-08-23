import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
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

    it('keeps new conversation in its floating position without an empty-state prompt', () => {
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

        const action = screen.getByRole('button', { name: '새 대화' });
        expect(action).toHaveClass('mobile-root-fab');
        expect(screen.queryByText('저장된 대화가 없습니다.')).not.toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '대화 시작' })).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.conversation-empty')).not.toBeInTheDocument();
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
        expect(action.querySelector('[aria-hidden="true"]')).toHaveTextContent('+');
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
        expect(selector).toHaveValue('default-enabled');
        expect(
            within(selector).getByRole('option', { name: 'default-enabled · 기본' }),
        ).toBeEnabled();
        expect(
            within(selector).getByRole('option', {
                name: 'alternate-disabled · 대체 · 비활성',
            }),
        ).toBeDisabled();
        expect(
            screen.getByText('인사 본문은 UI로 전달하지 않으며 ID와 종류만 선택합니다.'),
        ).toBeVisible();

        await fireEvent.change(selector, { target: { value: 'alternate-enabled' } });
        expect(selectGreeting).toHaveBeenCalledWith('alternate-enabled');

        await fireEvent.click(screen.getByRole('button', { name: '새 대화' }));
        expect(openNewConversation).toHaveBeenCalledOnce();
        expect(onOpenChat).toHaveBeenCalledOnce();

        await fireEvent.click(screen.getByRole('button', { name: /첫 대화/ }));
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

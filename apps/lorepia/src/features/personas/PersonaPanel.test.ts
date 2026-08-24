import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import PersonaPanel from './PersonaPanel.svelte';
import { PersonaController, type PersonaState } from './persona-controller';
import type { PersonaDto } from './persona-contracts';

afterEach(cleanup);

function persona(): PersonaDto {
    return {
        value: {
            id: 'persona-1',
            name: '현재 Persona',
            description: '현재 편집 가능한 설명',
        },
        revision: 5,
        revision_id: 'revision-5',
        created_at: '2026-08-03T00:00:00Z',
        updated_at: '2026-08-03T00:05:00Z',
    };
}

function state(): PersonaState {
    return {
        phase: 'ready',
        personas: [persona()],
        next_cursor: null,
        conversation_id: 'conversation-1',
        selection: {
            conversation_id: 'conversation-1',
            state_revision: 8,
            selected_persona: {
                value: {
                    id: 'persona-1',
                    name: '선택 당시 Persona',
                    description: '선택 당시 설명',
                },
                revision: 3,
                revision_id: 'revision-3',
                snapshot_created_at: '2026-08-03T00:03:00Z',
            },
            updated_at: '2026-08-03T00:04:00Z',
            cleared_at: null,
        },
        error: null,
        announcement: '',
    };
}

describe('PersonaPanel', () => {
    it('uses one localized settings surface without a duplicate page title', () => {
        const controller = new PersonaController({});
        const rendered = render(PersonaPanel, {
            personaState: state(),
            controller,
        });

        const panel = screen.getByRole('region', { name: '페르소나' });
        expect(panel).toHaveClass('persona-panel');
        expect(within(panel).queryByRole('heading', { name: '현재 대화' })).not.toBeInTheDocument();
        expect(within(panel).queryByText('테스트 대화')).not.toBeInTheDocument();
        expect(
            within(panel).queryByRole('heading', { name: '새 페르소나' }),
        ).not.toBeInTheDocument();
        expect(
            within(panel).queryByRole('heading', { name: '저장된 페르소나' }),
        ).not.toBeInTheDocument();
        expect(within(panel).queryByText('1개')).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.persona-count')).not.toBeInTheDocument();
        const actionBar = within(panel).getByRole('toolbar', { name: '페르소나 작업' });
        expect(within(actionBar).getByRole('button', { name: '페르소나 추가하기' })).toBeVisible();
        expect(
            within(panel).queryByRole('heading', { name: '내 Persona' }),
        ).not.toBeInTheDocument();
        expect(within(panel).queryByText('Local persona')).not.toBeInTheDocument();
        expect(
            rendered.container.querySelector('.section-heading.compact'),
        ).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.persona-selection')).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.persona-form')).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.persona-create-action')).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.persona-catalog')?.parentElement).toHaveClass(
            'persona-scroll',
        );
        const personaButton = within(panel).getByRole('button', {
            name: /현재 Persona 현재 편집 가능한 설명/,
        });
        expect(personaButton).toHaveClass('setting-row', 'persona-row');
        expect(personaButton.querySelector('.setting-chevron')).not.toBeInTheDocument();
        expect(personaButton.querySelector('.persona-row-name')).toHaveTextContent('현재 Persona');
        expect(personaButton.querySelector('.persona-row-description')).toHaveTextContent(
            '현재 편집 가능한 설명',
        );
        expect(within(panel).queryByRole('button', { name: '편집' })).not.toBeInTheDocument();
        expect(within(panel).queryByRole('button', { name: '삭제' })).not.toBeInTheDocument();
        expect(
            within(panel).queryByRole('button', { name: /이 대화에서 사용 중/ }),
        ).not.toBeInTheDocument();
    });

    it('uses the bottom action bar to create a persona', async () => {
        const controller = new PersonaController({});
        const create = vi.spyOn(controller, 'create').mockResolvedValue(true);
        const rendered = render(PersonaPanel, {
            personaState: state(),
            controller,
        });

        await fireEvent.click(screen.getByRole('button', { name: '페르소나 추가하기' }));

        const form = screen.getByRole('form', { name: '새 페르소나' });
        expect(form).toBeInTheDocument();
        expect(form).not.toHaveClass('settings-section');
        expect(screen.getByLabelText('이름')).toBeInTheDocument();
        expect(screen.getByLabelText('설명')).toBeInTheDocument();
        expect(rendered.container.querySelector('.persona-form')).toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '페르소나 추가하기' })).not.toBeInTheDocument();
        expect(screen.queryByRole('heading', { name: '저장된 페르소나' })).not.toBeInTheDocument();
        expect(screen.getByRole('button', { name: '페르소나 만들기' })).toBeDisabled();
        expect(screen.queryByRole('button', { name: '삭제' })).not.toBeInTheDocument();
        expect(
            screen.queryByRole('button', { name: /현재 Persona 현재 편집 가능한 설명/ }),
        ).not.toBeInTheDocument();

        await fireEvent.input(screen.getByLabelText('이름'), {
            target: { value: '새 페르소나 이름' },
        });
        await fireEvent.input(screen.getByLabelText('설명'), {
            target: { value: '새 페르소나 설명' },
        });
        await fireEvent.click(screen.getByRole('button', { name: '페르소나 만들기' }));

        expect(create).toHaveBeenCalledWith('새 페르소나 이름', '새 페르소나 설명');
        expect(screen.getByRole('button', { name: '페르소나 추가하기' })).toBeVisible();
    });

    it('opens a persona row as a dedicated prefilled edit screen', async () => {
        const controller = new PersonaController({});
        render(PersonaPanel, { personaState: state(), controller });

        await fireEvent.click(
            screen.getByRole('button', { name: /현재 Persona 현재 편집 가능한 설명/ }),
        );

        expect(screen.getByRole('form', { name: '페르소나 편집' })).toBeInTheDocument();
        expect(screen.getByLabelText('이름')).toHaveValue('현재 Persona');
        expect(screen.getByLabelText('설명')).toHaveValue('현재 편집 가능한 설명');
        expect(screen.queryByRole('button', { name: '페르소나 추가하기' })).not.toBeInTheDocument();
        expect(screen.queryByRole('heading', { name: '저장된 페르소나' })).not.toBeInTheDocument();
        const actionBar = screen.getByRole('toolbar', { name: '페르소나 작업' });
        expect(
            within(actionBar)
                .getAllByRole('button')
                .map((button) => button.textContent.trim()),
        ).toEqual(['삭제', '저장']);
    });

    it('offers recovery only when loading the persona state failed', async () => {
        const controller = new PersonaController({});
        const loadContext = vi.spyOn(controller, 'loadContext').mockResolvedValue(true);
        render(PersonaPanel, {
            personaState: {
                ...state(),
                phase: 'error',
                error: '페르소나를 불러오지 못했습니다.',
            },
            controller,
        });

        expect(screen.queryByRole('button', { name: '새로고침' })).not.toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '다시 불러오기' }));
        expect(loadContext).toHaveBeenCalledWith('conversation-1');
    });

    it('offers an explicit load-more action while another persona page exists', async () => {
        const controller = new PersonaController({});
        const loadMore = vi.fn().mockResolvedValue(true);
        Object.assign(controller, { loadMore });
        render(PersonaPanel, {
            personaState: {
                ...state(),
                next_cursor: {
                    catalog_revision: 'a'.repeat(64),
                    updated_at: '2026-08-03T00:05:00Z',
                    persona_id: 'persona-1',
                },
            },
            controller,
        });

        await fireEvent.click(screen.getByRole('button', { name: '더 불러오기' }));
        expect(loadMore).toHaveBeenCalledOnce();
    });

    it('keeps conversation selection details out of the persona manager', () => {
        const controller = new PersonaController({});
        render(PersonaPanel, { personaState: state(), controller });

        expect(screen.queryByText('선택 당시 Persona')).not.toBeInTheDocument();
        expect(screen.getByText('현재 Persona')).toBeInTheDocument();
        expect(screen.queryByText(/선택 리비전 3/)).not.toBeInTheDocument();
        expect(screen.queryByText(/현재 페르소나의 r3 스냅샷/)).not.toBeInTheDocument();
    });

    it('requires an explicit second click before deleting a persona', async () => {
        const controller = new PersonaController({});
        const remove = vi.spyOn(controller, 'deletePersona').mockResolvedValue(true);
        render(PersonaPanel, {
            personaState: state(),
            controller,
        });

        await fireEvent.click(
            screen.getByRole('button', { name: /현재 Persona 현재 편집 가능한 설명/ }),
        );
        await fireEvent.click(screen.getByRole('button', { name: '삭제' }));
        expect(remove).not.toHaveBeenCalled();
        const actionBar = screen.getByRole('toolbar', { name: '페르소나 작업' });
        expect(
            within(actionBar)
                .getAllByRole('button')
                .map((button) => button.textContent.trim()),
        ).toEqual(['삭제 확인', '취소']);

        await fireEvent.click(screen.getByRole('button', { name: '삭제 확인' }));
        expect(remove).toHaveBeenCalledWith(persona());
    });

    it.each([
        ['is refreshed to a newer revision', [persona()]],
        ['is removed from the authoritative catalog', []],
    ])(
        'updates the cloned edit-start revision when the catalog %s',
        async (_caseName, refreshedPersonas) => {
            const initialState = state();
            const sourcePersona = initialState.personas[0];
            if (sourcePersona === undefined) throw new Error('expected an editable persona');
            const editStart = structuredClone(sourcePersona);
            if (refreshedPersonas[0] !== undefined) {
                refreshedPersonas[0] = {
                    ...refreshedPersonas[0],
                    revision: editStart.revision + 1,
                    revision_id: 'revision-6',
                };
            }
            const controller = new PersonaController({});
            const create = vi.spyOn(controller, 'create').mockResolvedValue(true);
            const update = vi.spyOn(controller, 'updatePersona').mockResolvedValue(true);
            const rendered = render(PersonaPanel, {
                personaState: initialState,
                controller,
            });

            await fireEvent.click(
                screen.getByRole('button', { name: /현재 Persona 현재 편집 가능한 설명/ }),
            );
            sourcePersona.revision = 99;
            sourcePersona.revision_id = 'mutated-after-edit-start';
            await rendered.rerender({
                personaState: { ...initialState, personas: refreshedPersonas },
                controller,
            });
            await fireEvent.input(screen.getByLabelText('이름'), {
                target: { value: '편집 완료 Persona' },
            });
            await fireEvent.input(screen.getByLabelText('설명'), {
                target: { value: '편집 완료 설명' },
            });
            await fireEvent.click(screen.getByRole('button', { name: '저장' }));

            expect(create).not.toHaveBeenCalled();
            expect(update).toHaveBeenCalledWith(editStart, '편집 완료 Persona', '편집 완료 설명');
        },
    );
});

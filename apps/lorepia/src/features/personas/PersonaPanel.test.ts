import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
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
            conversationTitle: '테스트 대화',
        });

        await fireEvent.click(screen.getByRole('button', { name: '더 불러오기' }));
        expect(loadMore).toHaveBeenCalledOnce();
    });

    it('distinguishes the selected immutable snapshot from the current editable revision', () => {
        const controller = new PersonaController({});
        render(PersonaPanel, {
            personaState: state(),
            controller,
            conversationTitle: '테스트 대화',
        });

        expect(screen.getByText('선택 당시 Persona')).toBeInTheDocument();
        expect(screen.getByText('현재 Persona')).toBeInTheDocument();
        expect(screen.getByText(/선택 리비전 3/)).toBeInTheDocument();
        expect(screen.getByText(/현재 Persona의 r3 스냅샷/)).toBeInTheDocument();
        expect(screen.getByRole('button', { name: '이 대화에서 사용 중' })).toBeDisabled();
    });

    it('requires an explicit second click before deleting a persona', async () => {
        const controller = new PersonaController({});
        const remove = vi.spyOn(controller, 'deletePersona').mockResolvedValue(true);
        render(PersonaPanel, {
            personaState: state(),
            controller,
            conversationTitle: '테스트 대화',
        });

        await fireEvent.click(screen.getByRole('button', { name: '삭제' }));
        expect(remove).not.toHaveBeenCalled();

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
                conversationTitle: '테스트 대화',
            });

            await fireEvent.click(screen.getByRole('button', { name: '편집' }));
            sourcePersona.revision = 99;
            sourcePersona.revision_id = 'mutated-after-edit-start';
            await rendered.rerender({
                personaState: { ...initialState, personas: refreshedPersonas },
                controller,
                conversationTitle: '테스트 대화',
            });
            await fireEvent.input(screen.getByLabelText('이름'), {
                target: { value: '편집 완료 Persona' },
            });
            await fireEvent.input(screen.getByLabelText('설명'), {
                target: { value: '편집 완료 설명' },
            });
            await fireEvent.click(screen.getByRole('button', { name: '변경 저장' }));

            expect(create).not.toHaveBeenCalled();
            expect(update).toHaveBeenCalledWith(editStart, '편집 완료 Persona', '편집 완료 설명');
        },
    );
});

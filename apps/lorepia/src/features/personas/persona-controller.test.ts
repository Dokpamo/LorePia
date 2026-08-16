import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import { PersonaController } from './persona-controller';
import type {
    ClearConversationPersonaInput,
    ConversationPersonaSelectionDto,
    CreatePersonaInput,
    DeletePersonaInput,
    GetConversationPersonaSelectionInput,
    PersonaClientApi,
    PersonaDto,
    PersonaListPageDto,
    PersonaPageCursorDto,
    SelectConversationPersonaInput,
    UpdatePersonaInput,
} from './persona-contracts';

const CATALOG_REVISION = 'a'.repeat(64);

function page(
    items: PersonaDto[],
    nextCursor: Omit<PersonaPageCursorDto, 'catalog_revision'> | null = null,
    catalogRevision = CATALOG_REVISION,
): Extract<PersonaListPageDto, { kind: 'page' }> {
    return {
        kind: 'page',
        catalog_revision: catalogRevision,
        items,
        next_cursor:
            nextCursor === null ? null : { catalog_revision: catalogRevision, ...nextCursor },
    };
}

function persona(id: string, name: string, revision = 1): PersonaDto {
    return {
        value: { id, name, description: `${name} description` },
        revision,
        revision_id: `${id}-revision-${String(revision)}`,
        created_at: '2026-08-03T00:00:00Z',
        updated_at: '2026-08-03T00:00:00Z',
    };
}

function selection(
    conversationId: string,
    selected: PersonaDto | null,
    stateRevision: number | null,
): ConversationPersonaSelectionDto {
    return {
        conversation_id: conversationId,
        state_revision: stateRevision,
        selected_persona:
            selected === null
                ? null
                : {
                      value: structuredClone(selected.value),
                      revision: selected.revision,
                      revision_id: selected.revision_id,
                      snapshot_created_at: selected.updated_at,
                  },
        updated_at: stateRevision === null ? null : '2026-08-03T00:00:00Z',
        cleared_at: selected === null && stateRevision !== null ? '2026-08-03T00:00:00Z' : null,
    };
}

function client(overrides: Partial<PersonaClientApi> = {}): PersonaClientApi {
    const first = persona('persona-1', 'Narrator');
    return {
        createPersona: vi.fn((input: CreatePersonaInput) =>
            Promise.resolve(persona('persona-created', input.name)),
        ),
        updatePersona: vi.fn((input: UpdatePersonaInput) =>
            Promise.resolve({
                ...persona(input.persona_id, input.name, input.expected_revision + 1),
                value: {
                    id: input.persona_id,
                    name: input.name,
                    description: input.description,
                },
            }),
        ),
        getPersona: vi.fn().mockResolvedValue(first),
        listPersonas: vi.fn().mockResolvedValue([first]),
        listPersonaPage: vi.fn().mockResolvedValue(page([first])),
        deletePersona: vi.fn((input: DeletePersonaInput) =>
            Promise.resolve({
                persona_id: input.persona_id,
                revision: input.expected_revision + 1,
                deleted_at: '2026-08-03T00:00:00Z',
            }),
        ),
        getConversationPersonaSelection: vi.fn((input: GetConversationPersonaSelectionInput) =>
            Promise.resolve(selection(input.conversation_id, null, null)),
        ),
        selectConversationPersona: vi.fn((input: SelectConversationPersonaInput) =>
            Promise.resolve(
                selection(input.conversation_id, first, (input.expected_state_revision ?? 0) + 1),
            ),
        ),
        clearConversationPersona: vi.fn((input: ClearConversationPersonaInput) =>
            Promise.resolve(
                selection(input.conversation_id, null, input.expected_state_revision + 1),
            ),
        ),
        ...overrides,
    };
}

describe('PersonaController', () => {
    it('loads all 101 personas across keyset pages without duplicates', async () => {
        const firstPage = Array.from({ length: 100 }, (_, index) =>
            persona(`persona-${String(index).padStart(3, '0')}`, `Persona ${String(index)}`),
        );
        const last = firstPage.at(-1);
        if (last === undefined) throw new Error('expected a full first page');
        const finalPersona = persona('persona-100', 'Persona 100');
        const listPersonaPage = vi
            .fn()
            .mockResolvedValueOnce(
                page(firstPage, {
                    updated_at: last.updated_at,
                    persona_id: last.value.id,
                }),
            )
            .mockResolvedValueOnce(page([last, finalPersona]));
        const api = client();
        Object.assign(api, { listPersonaPage });
        const controller = new PersonaController(api);

        expect(await controller.loadContext(null)).toBe(true);
        expect(get(controller.state).personas).toHaveLength(100);
        expect(
            await (
                controller as PersonaController & {
                    loadMore(): Promise<boolean>;
                }
            ).loadMore(),
        ).toBe(true);

        const loaded = get(controller.state);
        expect(loaded.personas).toHaveLength(101);
        expect(new Set(loaded.personas.map((item) => item.value.id)).size).toBe(101);
        expect(loaded.personas.at(-1)?.value.id).toBe('persona-100');
        expect(listPersonaPage).toHaveBeenNthCalledWith(1, { limit: 100, after: null });
        expect(listPersonaPage).toHaveBeenNthCalledWith(2, {
            limit: 100,
            after: {
                catalog_revision: CATALOG_REVISION,
                updated_at: last.updated_at,
                persona_id: last.value.id,
            },
        });
    });

    it('restarts from an authoritative first page when the catalog changes mid-pagination', async () => {
        const first = persona('persona-001', 'First');
        const boundary = persona('persona-099', 'Boundary');
        const moved = persona('persona-100', 'Moved before the cursor', 2);
        const currentRevision = 'b'.repeat(64);
        const listPersonaPage = vi
            .fn()
            .mockResolvedValueOnce(
                page([first, boundary], {
                    updated_at: boundary.updated_at,
                    persona_id: boundary.value.id,
                }),
            )
            .mockResolvedValueOnce({
                kind: 'restart_required',
                current_catalog_revision: currentRevision,
            } satisfies PersonaListPageDto)
            .mockResolvedValueOnce(page([moved, first], null, currentRevision));
        const api = client({ listPersonaPage });
        const controller = new PersonaController(api);

        expect(await controller.loadContext(null)).toBe(true);
        expect(await controller.loadMore()).toBe(true);

        const loaded = get(controller.state);
        expect(loaded.personas).toEqual([moved, first]);
        expect(loaded.next_cursor).toBeNull();
        expect(loaded.announcement).toContain('최신 첫 페이지부터 다시');
        expect(listPersonaPage).toHaveBeenNthCalledWith(2, {
            limit: 100,
            after: {
                catalog_revision: CATALOG_REVISION,
                updated_at: boundary.updated_at,
                persona_id: boundary.value.id,
            },
        });
        expect(listPersonaPage).toHaveBeenNthCalledWith(3, { limit: 100, after: null });
    });

    it('reloads the conversation selection when a stale page restarts after external deletion', async () => {
        const selected = persona('persona-001', 'Selected');
        const boundary = persona('persona-099', 'Boundary');
        const currentRevision = 'b'.repeat(64);
        const listPersonaPage = vi
            .fn()
            .mockResolvedValueOnce(
                page([selected, boundary], {
                    updated_at: boundary.updated_at,
                    persona_id: boundary.value.id,
                }),
            )
            .mockResolvedValueOnce({
                kind: 'restart_required',
                current_catalog_revision: currentRevision,
            } satisfies PersonaListPageDto)
            .mockResolvedValueOnce(page([], null, currentRevision));
        const getSelection = vi
            .fn()
            .mockResolvedValueOnce(selection('conversation-1', selected, 6))
            .mockResolvedValueOnce(selection('conversation-1', null, 7));
        const controller = new PersonaController(
            client({
                listPersonaPage,
                getConversationPersonaSelection: getSelection,
            }),
        );

        expect(await controller.loadContext('conversation-1')).toBe(true);
        expect(await controller.loadMore()).toBe(true);

        const loaded = get(controller.state);
        expect(loaded.personas).toEqual([]);
        expect(loaded.selection?.selected_persona).toBeNull();
        expect(loaded.selection?.state_revision).toBe(7);
        expect(getSelection).toHaveBeenNthCalledWith(2, {
            conversation_id: 'conversation-1',
        });
    });

    it('replaces the catalog with the authoritative first page after create', async () => {
        const existing = persona('persona-001', 'Existing');
        const created = persona('persona-created', 'Created');
        const listPersonaPage = vi
            .fn()
            .mockResolvedValueOnce(page([existing]))
            .mockResolvedValueOnce(page([existing, created]));
        const api = client({
            listPersonaPage,
            createPersona: vi.fn().mockResolvedValue(created),
        });
        const controller = new PersonaController(api);
        await controller.loadContext(null);

        expect(await controller.create('Created', 'Created description')).toBe(true);

        expect(get(controller.state).personas).toEqual([existing, created]);
        expect(listPersonaPage).toHaveBeenLastCalledWith({ limit: 100, after: null });
        expect(listPersonaPage).toHaveBeenCalledTimes(2);
    });

    it('loads global personas and the selected immutable conversation snapshot together', async () => {
        const selected = persona('persona-1', 'Pinned narrator', 3);
        const api = client({
            listPersonaPage: vi
                .fn()
                .mockResolvedValue(page([persona('persona-1', 'Current narrator', 5)])),
            getConversationPersonaSelection: vi
                .fn()
                .mockResolvedValue(selection('conversation-1', selected, 8)),
        });
        const controller = new PersonaController(api);

        expect(await controller.loadContext('conversation-1')).toBe(true);

        const state = get(controller.state);
        expect(state.personas[0]?.revision).toBe(5);
        expect(state.selection?.selected_persona?.revision).toBe(3);
        expect(state.selection?.selected_persona?.value.name).toBe('Pinned narrator');
    });

    it('uses the selection tombstone revision for select and clear CAS', async () => {
        const first = persona('persona-1', 'Narrator');
        const selectConversationPersona = vi
            .fn()
            .mockResolvedValue(selection('conversation-1', first, 12));
        const clearConversationPersona = vi
            .fn()
            .mockResolvedValue(selection('conversation-1', null, 13));
        const api = client({
            listPersonaPage: vi.fn().mockResolvedValue(page([first])),
            getConversationPersonaSelection: vi
                .fn()
                .mockResolvedValue(selection('conversation-1', null, 11)),
            selectConversationPersona,
            clearConversationPersona,
        });
        const controller = new PersonaController(api);
        await controller.loadContext('conversation-1');

        expect(await controller.selectPersona(first)).toBe(true);
        expect(selectConversationPersona).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            persona_id: 'persona-1',
            expected_state_revision: 11,
        });

        expect(await controller.clearSelection()).toBe(true);
        expect(clearConversationPersona).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            expected_state_revision: 12,
        });
        expect(get(controller.state).selection?.state_revision).toBe(13);
    });

    it('keeps the selected snapshot pinned when the editable persona changes', async () => {
        const current = persona('persona-1', 'Narrator', 2);
        const other = persona('persona-2', 'Other');
        const updated = {
            ...persona('persona-1', 'Updated narrator', 3),
            value: {
                id: 'persona-1',
                name: 'Updated narrator',
                description: 'Updated description',
            },
        };
        const listPersonaPage = vi
            .fn()
            .mockResolvedValueOnce(page([other, current]))
            .mockResolvedValueOnce(page([updated, other]));
        const api = client({
            listPersonaPage,
            getConversationPersonaSelection: vi
                .fn()
                .mockResolvedValue(selection('conversation-1', current, 4)),
        });
        const controller = new PersonaController(api);
        await controller.loadContext('conversation-1');

        expect(
            await controller.updatePersona(current, 'Updated narrator', 'Updated description'),
        ).toBe(true);

        const state = get(controller.state);
        expect(state.personas[0]?.revision).toBe(3);
        expect(state.personas[0]?.value.name).toBe('Updated narrator');
        expect(state.personas[1]?.value.id).toBe('persona-2');
        expect(state.selection?.selected_persona?.revision).toBe(2);
        expect(state.selection?.selected_persona?.value.name).toBe('Narrator');
        expect(state.announcement).toContain('선택 당시 리비전');
        expect(listPersonaPage).toHaveBeenCalledTimes(2);
    });

    it('refreshes the conversation tombstone after deleting a selected persona', async () => {
        const selected = persona('persona-1', 'Narrator');
        const getSelection = vi
            .fn()
            .mockResolvedValueOnce(selection('conversation-1', selected, 6))
            .mockResolvedValueOnce(selection('conversation-1', null, 7));
        const listPersonaPage = vi
            .fn()
            .mockResolvedValueOnce(page([selected]))
            .mockResolvedValueOnce(page([]));
        const api = client({
            listPersonaPage,
            getConversationPersonaSelection: getSelection,
        });
        const controller = new PersonaController(api);
        await controller.loadContext('conversation-1');

        expect(await controller.deletePersona(selected)).toBe(true);

        const state = get(controller.state);
        expect(state.personas).toEqual([]);
        expect(state.selection?.selected_persona).toBeNull();
        expect(state.selection?.state_revision).toBe(7);
        expect(listPersonaPage).toHaveBeenCalledTimes(2);
    });
});

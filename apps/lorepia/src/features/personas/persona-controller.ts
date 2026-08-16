import { get, writable, type Readable } from 'svelte/store';

import { normalizeClientError } from '../../lib/ipc/errors';
import type {
    ConversationPersonaSelectionDto,
    PersonaClientApi,
    PersonaDto,
    PersonaListPageDto,
    PersonaPageCursorDto,
} from './persona-contracts';

type PersonaCatalogPage = Extract<PersonaListPageDto, { kind: 'page' }>;

export type PersonaPhase = 'idle' | 'loading' | 'ready' | 'saving' | 'unavailable' | 'error';

export interface PersonaState {
    phase: PersonaPhase;
    personas: PersonaDto[];
    next_cursor: PersonaPageCursorDto | null;
    conversation_id: string | null;
    selection: ConversationPersonaSelectionDto | null;
    error: string | null;
    announcement: string;
}

export const INITIAL_PERSONA_STATE: PersonaState = {
    phase: 'idle',
    personas: [],
    next_cursor: null,
    conversation_id: null,
    selection: null,
    error: null,
    announcement: '',
};

function errorLabel(error: unknown): string {
    const normalized = normalizeClientError(error);
    switch (normalized.code) {
        case 'invalid_input':
            return '다른 화면에서 Persona가 변경되었습니다. 최신 상태를 다시 불러와 주세요.';
        case 'not_found':
            return 'Persona 또는 대화를 찾을 수 없습니다.';
        case 'permission_denied':
            return '현재 로컬 사용자가 소유한 Persona만 수정할 수 있습니다.';
        default:
            return normalized.messageKey === 'error.unexpected'
                ? 'Persona 작업을 완료하지 못했습니다.'
                : normalized.messageKey;
    }
}

function hasPersonaApi(client: Partial<PersonaClientApi>): client is PersonaClientApi {
    return (
        client.createPersona !== undefined &&
        client.updatePersona !== undefined &&
        client.getPersona !== undefined &&
        client.listPersonas !== undefined &&
        client.listPersonaPage !== undefined &&
        client.deletePersona !== undefined &&
        client.getConversationPersonaSelection !== undefined &&
        client.selectConversationPersona !== undefined &&
        client.clearConversationPersona !== undefined
    );
}

export class PersonaController {
    private readonly mutable = writable<PersonaState>(structuredClone(INITIAL_PERSONA_STATE));
    readonly state: Readable<PersonaState> = this.mutable;

    private operationEpoch = 0;
    private readonly available: boolean;

    constructor(private readonly client: Partial<PersonaClientApi>) {
        this.available = hasPersonaApi(client);
        if (!this.available) {
            const message = '현재 Core가 안전한 Persona 관리 API를 제공하지 않습니다.';
            this.mutable.set({
                ...structuredClone(INITIAL_PERSONA_STATE),
                phase: 'unavailable',
                error: message,
                announcement: message,
            });
        }
    }

    private update(updater: (state: PersonaState) => PersonaState): void {
        this.mutable.update(updater);
    }

    private markError(epoch: number, error: unknown): false {
        if (epoch !== this.operationEpoch) return false;
        const message = errorLabel(error);
        this.update((state) => ({
            ...state,
            phase: 'error',
            error: message,
            announcement: message,
        }));
        return false;
    }

    private async loadFirstPage(): Promise<PersonaCatalogPage> {
        if (!hasPersonaApi(this.client)) {
            throw new Error('The persona catalog API is unavailable.');
        }
        const result = await this.client.listPersonaPage({ limit: 100, after: null });
        if (result.kind !== 'page') {
            throw new Error('An initial persona catalog page unexpectedly required restart.');
        }
        return result;
    }

    async loadContext(conversationId: string | null): Promise<boolean> {
        if (!this.available || !hasPersonaApi(this.client)) return false;
        const epoch = ++this.operationEpoch;
        this.update((state) => ({
            ...state,
            phase: 'loading',
            conversation_id: conversationId,
            selection: null,
            error: null,
        }));
        try {
            const [page, selection] = await Promise.all([
                this.loadFirstPage(),
                conversationId === null
                    ? Promise.resolve(null)
                    : this.client.getConversationPersonaSelection({
                          conversation_id: conversationId,
                      }),
            ]);
            if (epoch !== this.operationEpoch) return false;
            this.update((state) => ({
                ...state,
                phase: 'ready',
                personas: page.items,
                next_cursor: page.next_cursor,
                selection,
                error: null,
            }));
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    async loadMore(): Promise<boolean> {
        if (!this.available || !hasPersonaApi(this.client)) return false;
        const current = get(this.mutable);
        if (
            current.next_cursor === null ||
            current.phase === 'loading' ||
            current.phase === 'saving'
        ) {
            return false;
        }
        const epoch = ++this.operationEpoch;
        const after = structuredClone(current.next_cursor);
        this.update((state) => ({ ...state, phase: 'loading', error: null }));
        try {
            const result = await this.client.listPersonaPage({ limit: 100, after });
            if (epoch !== this.operationEpoch) return false;
            if (result.kind === 'restart_required') {
                const [page, selection] = await Promise.all([
                    this.loadFirstPage(),
                    current.conversation_id === null
                        ? Promise.resolve(null)
                        : this.client.getConversationPersonaSelection({
                              conversation_id: current.conversation_id,
                          }),
                ]);
                if (epoch !== this.operationEpoch) return false;
                this.update((state) => ({
                    ...state,
                    phase: 'ready',
                    personas: page.items,
                    next_cursor: page.next_cursor,
                    selection,
                    error: null,
                    announcement: 'Persona 목록이 변경되어 최신 첫 페이지부터 다시 불러왔습니다.',
                }));
                return true;
            }
            const page = result;
            this.update((state) => {
                const byId = new Map(page.items.map((persona) => [persona.value.id, persona]));
                const personas = state.personas.map(
                    (persona) => byId.get(persona.value.id) ?? persona,
                );
                const knownIds = new Set(personas.map((persona) => persona.value.id));
                for (const persona of page.items) {
                    if (knownIds.has(persona.value.id)) continue;
                    knownIds.add(persona.value.id);
                    personas.push(persona);
                }
                return {
                    ...state,
                    phase: 'ready',
                    personas,
                    next_cursor: page.next_cursor,
                    error: null,
                };
            });
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    async create(name: string, description: string): Promise<boolean> {
        if (!this.available || !hasPersonaApi(this.client)) return false;
        const epoch = ++this.operationEpoch;
        this.update((state) => ({ ...state, phase: 'saving', error: null }));
        try {
            const created = await this.client.createPersona({ name, description });
            const refreshed = await this.loadFirstPage();
            if (epoch !== this.operationEpoch) return false;
            this.update((state) => ({
                ...state,
                phase: 'ready',
                personas: refreshed.items,
                next_cursor: refreshed.next_cursor,
                error: null,
                announcement: `${created.value.name} Persona를 만들었습니다.`,
            }));
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    async updatePersona(persona: PersonaDto, name: string, description: string): Promise<boolean> {
        if (!this.available || !hasPersonaApi(this.client)) return false;
        const epoch = ++this.operationEpoch;
        this.update((state) => ({ ...state, phase: 'saving', error: null }));
        try {
            const updated = await this.client.updatePersona({
                persona_id: persona.value.id,
                expected_revision: persona.revision,
                name,
                description,
            });
            const refreshed = await this.loadFirstPage();
            if (epoch !== this.operationEpoch) return false;
            this.update((state) => ({
                ...state,
                phase: 'ready',
                personas: refreshed.items,
                next_cursor: refreshed.next_cursor,
                error: null,
                announcement:
                    state.selection?.selected_persona?.value.id === updated.value.id
                        ? `${updated.value.name} Persona를 수정했습니다. 현재 대화는 선택 당시 리비전을 계속 사용합니다.`
                        : `${updated.value.name} Persona를 수정했습니다.`,
            }));
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    async deletePersona(persona: PersonaDto): Promise<boolean> {
        if (!this.available || !hasPersonaApi(this.client)) return false;
        const epoch = ++this.operationEpoch;
        const conversationId = get(this.mutable).conversation_id;
        this.update((state) => ({ ...state, phase: 'saving', error: null }));
        try {
            await this.client.deletePersona({
                persona_id: persona.value.id,
                expected_revision: persona.revision,
            });
            const [selection, refreshed] = await Promise.all([
                conversationId === null
                    ? Promise.resolve(null)
                    : this.client.getConversationPersonaSelection({
                          conversation_id: conversationId,
                      }),
                this.loadFirstPage(),
            ]);
            if (epoch !== this.operationEpoch) return false;
            this.update((state) => ({
                ...state,
                phase: 'ready',
                personas: refreshed.items,
                next_cursor: refreshed.next_cursor,
                selection,
                error: null,
                announcement: `${persona.value.name} Persona를 삭제했습니다.`,
            }));
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    async selectPersona(persona: PersonaDto): Promise<boolean> {
        if (!this.available || !hasPersonaApi(this.client)) return false;
        const current = get(this.mutable);
        if (current.conversation_id === null) return false;
        const epoch = ++this.operationEpoch;
        this.update((state) => ({ ...state, phase: 'saving', error: null }));
        try {
            const selection = await this.client.selectConversationPersona({
                conversation_id: current.conversation_id,
                persona_id: persona.value.id,
                expected_state_revision: current.selection?.state_revision ?? null,
            });
            if (epoch !== this.operationEpoch) return false;
            this.update((state) => ({
                ...state,
                phase: 'ready',
                selection,
                error: null,
                announcement: `${selection.selected_persona?.value.name ?? persona.value.name} Persona를 이 대화에 선택했습니다.`,
            }));
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    async clearSelection(): Promise<boolean> {
        if (!this.available || !hasPersonaApi(this.client)) return false;
        const current = get(this.mutable);
        const stateRevision = current.selection?.state_revision;
        if (
            current.conversation_id === null ||
            stateRevision === null ||
            stateRevision === undefined
        ) {
            return false;
        }
        const epoch = ++this.operationEpoch;
        this.update((state) => ({ ...state, phase: 'saving', error: null }));
        try {
            const selection = await this.client.clearConversationPersona({
                conversation_id: current.conversation_id,
                expected_state_revision: stateRevision,
            });
            if (epoch !== this.operationEpoch) return false;
            this.update((state) => ({
                ...state,
                phase: 'ready',
                selection,
                error: null,
                announcement: '이 대화의 Persona 선택을 해제했습니다.',
            }));
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    destroy(): void {
        this.operationEpoch += 1;
    }
}

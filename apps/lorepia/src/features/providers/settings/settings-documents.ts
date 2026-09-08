import { get, writable } from 'svelte/store';
import { t } from '../../../lib/i18n';
import type {
    CreatorMemoryProfileDocumentDto,
    CreatorPromptPresetDocumentDto,
    OrchestrationDocumentClientApi,
    PromptPresetSummaryDto,
    RevisionedDto,
    TaskProfileDocumentDto,
} from '../../../lib/ipc/contracts';
import { errorLabel } from '../../orchestration/controllers/orchestration-state';

export interface SettingsDocumentsState {
    phase: 'loading' | 'ready' | 'error';
    busy: boolean;
    error: string | null;
    announcement: string;
    prompts: RevisionedDto<PromptPresetSummaryDto>[];
    memories: RevisionedDto<CreatorMemoryProfileDocumentDto>[];
    tasks: RevisionedDto<TaskProfileDocumentDto>[];
}
const initialState = (): SettingsDocumentsState => ({
    phase: 'loading',
    busy: false,
    error: null,
    announcement: '',
    prompts: [],
    memories: [],
    tasks: [],
});

/** Global settings documents do not require an open conversation. Writes keep Rust revision authority. */
export class SettingsDocumentsController {
    readonly state = writable<SettingsDocumentsState>(initialState());
    private disposed = false;
    private loading = 0;
    private selecting = 0;
    private busy = false;
    constructor(private readonly client: Partial<OrchestrationDocumentClientApi>) {}

    async load(): Promise<void> {
        if (this.isDisposed() || this.busy) return;
        const epoch = ++this.loading;
        this.state.update((s) => ({ ...s, phase: 'loading', error: null }));
        try {
            if (
                !this.client.listPromptPresets ||
                !this.client.listMemoryProfiles ||
                !this.client.listTaskProfiles
            )
                throw new Error(t('settingsLive.unsupported'));
            const [prompts, memories, tasks] = await Promise.all([
                this.client.listPromptPresets(),
                this.client.listMemoryProfiles(),
                this.client.listTaskProfiles(),
            ]);
            if (this.isDisposed() || epoch !== this.loading) return;
            this.state.set({ ...initialState(), phase: 'ready', prompts, memories, tasks });
        } catch (error) {
            if (!this.isDisposed() && epoch === this.loading)
                this.state.update((s) => ({ ...s, phase: 'error', error: errorLabel(error) }));
        }
    }

    async openPrompt(id: string): Promise<RevisionedDto<CreatorPromptPresetDocumentDto> | null> {
        const epoch = ++this.selecting;
        try {
            if (!this.client.getEditablePromptPreset)
                throw new Error(t('settingsLive.unsupported'));
            const document = await this.client.getEditablePromptPreset({ prompt_preset_id: id });
            return !this.isDisposed() && epoch === this.selecting ? document : null;
        } catch (error) {
            if (!this.isDisposed() && epoch === this.selecting)
                this.state.update((s) => ({ ...s, error: errorLabel(error) }));
            return null;
        }
    }
    private isDisposed(): boolean {
        return this.disposed;
    }
    cancelSelection(): void {
        this.selecting++;
    }

    private async write<T>(
        operation: () => Promise<T>,
        commit: (state: SettingsDocumentsState, value: T) => SettingsDocumentsState,
    ): Promise<T | null> {
        if (this.isDisposed() || this.busy || get(this.state).phase !== 'ready') return null;
        this.busy = true;
        this.loading++;
        this.state.update((s) => ({ ...s, busy: true, error: null, announcement: '' }));
        try {
            const value = await operation();
            if (this.isDisposed()) return null;
            this.state.update((s) => ({
                ...commit(s, value),
                announcement: t('settingsLive.saved'),
            }));
            return value;
        } catch (error) {
            if (!this.isDisposed()) this.state.update((s) => ({ ...s, error: errorLabel(error) }));
            return null;
        } finally {
            this.busy = false;
            if (!this.isDisposed()) this.state.update((s) => ({ ...s, busy: false }));
        }
    }
    saveTask(value: TaskProfileDocumentDto, revision: number | null) {
        return this.write(
            async () => {
                if (!this.client.upsertTaskProfile) throw new Error(t('settingsLive.unsupported'));
                return this.client.upsertTaskProfile({
                    value: structuredClone(value),
                    expected_revision: revision,
                });
            },
            (s, saved) => ({ ...s, tasks: replace(s.tasks, saved) }),
        );
    }
    saveMemory(value: CreatorMemoryProfileDocumentDto, revision: number | null) {
        return this.write(
            async () => {
                if (!this.client.upsertMemoryProfile)
                    throw new Error(t('settingsLive.unsupported'));
                return this.client.upsertMemoryProfile({
                    value: structuredClone(value),
                    expected_revision: revision,
                });
            },
            (s, saved) => ({ ...s, memories: replace(s.memories, saved) }),
        );
    }
    savePrompt(value: CreatorPromptPresetDocumentDto, revision: number | null) {
        return this.write(
            async () => {
                if (!this.client.upsertPromptPreset) throw new Error(t('settingsLive.unsupported'));
                return this.client.upsertPromptPreset({
                    value: structuredClone(value),
                    expected_revision: revision,
                });
            },
            (s, saved) => ({ ...s, prompts: replace(s.prompts, saved) }),
        );
    }
    deleteMemory(document: RevisionedDto<CreatorMemoryProfileDocumentDto>) {
        return this.write(
            async () => {
                if (!this.client.deleteMemoryProfile)
                    throw new Error(t('settingsLive.unsupported'));
                return this.client.deleteMemoryProfile({
                    memory_profile_id: document.value.id,
                    expected_revision: document.revision,
                });
            },
            (s) => ({
                ...s,
                memories: s.memories.filter((item) => item.value.id !== document.value.id),
            }),
        );
    }
    deletePrompt(document: RevisionedDto<PromptPresetSummaryDto>) {
        return this.write(
            async () => {
                if (!this.client.deletePromptPreset) throw new Error(t('settingsLive.unsupported'));
                return this.client.deletePromptPreset({
                    prompt_preset_id: document.value.id,
                    expected_revision: document.revision,
                });
            },
            (s) => ({
                ...s,
                prompts: s.prompts.filter((item) => item.value.id !== document.value.id),
            }),
        );
    }
    destroy(): void {
        this.disposed = true;
        this.loading++;
        this.selecting++;
    }
}
function replace<T extends { id: string }>(
    items: RevisionedDto<T>[],
    value: RevisionedDto<T>,
): RevisionedDto<T>[] {
    return items.some((item) => item.value.id === value.value.id)
        ? items.map((item) => (item.value.id === value.value.id ? value : item))
        : [...items, value];
}

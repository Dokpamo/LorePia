import { get, writable } from 'svelte/store';
import { t } from '../../../lib/i18n';
import type {
    CreatorPromptPresetDocumentDto,
    PromptPresetSummaryDto,
    RevisionedDto,
} from '../../../lib/ipc/contracts';
import {
    buildImportedGenerationPreset,
    buildImportedMemoryProfile,
    buildImportedSummaryTask,
    hintNumber,
    readImportedCompatibilityHints,
    type ImportedCompatibilityKind,
} from '../../orchestration/imported-compatibility';
import { errorLabel } from '../../orchestration/controllers/orchestration-state';
import { isBuiltInPrompt } from './settings-defaults';
import type { SettingsServices } from './settings-services';

type Source = RevisionedDto<CreatorPromptPresetDocumentDto>;
interface SetupState {
    loading: boolean;
    saving: boolean;
    error: string | null;
    sources: Source[];
}
export interface ImportedSetupSelection {
    kind: ImportedCompatibilityKind;
    sourceId: string;
    routeId: string;
    targetPromptId: string;
    embeddingTaskId: string;
    applyToRoom: boolean;
}

/** Owns source loading and revision-checked writes for imported settings. */
export class ImportedSetupController {
    readonly state = writable<SetupState>({
        loading: false,
        saving: false,
        error: null,
        sources: [],
    });
    private epoch = 0;
    private disposed = false;
    constructor(private readonly services: () => SettingsServices) {}

    async load(catalog: RevisionedDto<PromptPresetSummaryDto>[]): Promise<void> {
        if (this.disposed || get(this.state).saving) return;
        const epoch = ++this.epoch;
        this.state.update((s) => ({ ...s, loading: true, error: null }));
        try {
            const client = this.services().client;
            const loader = client.getEditablePromptPreset;
            if (!loader) throw new SetupError(t('settingsLive.unsupported'));
            const sources: Source[] = [];
            const editable = catalog.filter((item) => !isBuiltInPrompt(item.value.id));
            for (let index = 0; index < editable.length; index += 4) {
                const batch = await Promise.all(
                    editable
                        .slice(index, index + 4)
                        .map((item) => loader.call(client, { prompt_preset_id: item.value.id })),
                );
                if (!this.current(epoch)) return;
                sources.push(...batch);
            }
            this.state.update((s) => ({ ...s, sources }));
        } catch (error) {
            if (this.current(epoch)) this.fail(error);
        } finally {
            if (this.current(epoch)) this.state.update((s) => ({ ...s, loading: false }));
        }
    }

    async save(selection: ImportedSetupSelection): Promise<boolean> {
        if (this.disposed || get(this.state).saving || get(this.state).loading) return false;
        const services = this.services();
        const client = services.client;
        const context = roomContext(services);
        const epoch = ++this.epoch;
        this.state.update((s) => ({ ...s, saving: true, error: null }));
        try {
            if (!client.getEditablePromptPreset || !client.upsertPromptPreset)
                throw new SetupError(t('settingsLive.unsupported'));
            const source = await client.getEditablePromptPreset({
                prompt_preset_id: selection.sourceId,
            });
            if (!this.current(epoch)) return false;
            const hints = readImportedCompatibilityHints(source.value);
            if (hints?.kind !== selection.kind || !selection.routeId)
                throw new SetupError(
                    t('orchestration.imported_compatibility.error.route_required'),
                );
            const workspace = services.appState.providers.workspace;
            if (!workspace.routes.some((item) => item.id === selection.routeId))
                throw new SetupError(
                    t('orchestration.imported_compatibility.error.route_required'),
                );
            if (selection.applyToRoom && !context)
                throw new SetupError(t('settingsLive.chooseRoom'));
            const presetId = await importedSetupId(
                source.value.id,
                selection.kind === 'memory' ? 'summary-provider' : 'provider',
            );
            if (!this.current(epoch)) return false;
            const generation = buildImportedGenerationPreset(
                workspace,
                hints,
                selection.routeId,
                presetId,
                source.value.name,
            );
            let promptId = source.value.id;
            let generationId: string | null = presetId;
            if (selection.kind === 'memory') {
                if (
                    !client.listTaskProfiles ||
                    !client.upsertTaskProfile ||
                    !client.listMemoryProfiles ||
                    !client.upsertMemoryProfile
                )
                    throw new SetupError(t('settingsLive.unsupported'));
                const [tasks, memories, target] = await Promise.all([
                    client.listTaskProfiles(),
                    client.listMemoryProfiles(),
                    selection.targetPromptId
                        ? client.getEditablePromptPreset({
                              prompt_preset_id: selection.targetPromptId,
                          })
                        : Promise.resolve(null),
                ]);
                if (!this.current(epoch)) return false;
                if (target && readImportedCompatibilityHints(target.value)?.kind === 'memory')
                    throw new SetupError(
                        t('orchestration.imported_compatibility.error.memory_selection'),
                    );
                if (selection.applyToRoom && !target)
                    throw new SetupError(
                        t('orchestration.imported_compatibility.error.memory_selection'),
                    );
                const task = buildImportedSummaryTask(hints, selection.routeId, presetId);
                const memory =
                    task &&
                    buildImportedMemoryProfile(
                        source.value,
                        hints,
                        task.id,
                        t('orchestration.imported_compatibility.memory_name', {
                            name: source.value.name,
                        }),
                    );
                if (!task || !memory)
                    throw new SetupError(
                        t('orchestration.imported_compatibility.error.memory_metadata'),
                    );
                const embedding = tasks.find(
                    (item) => item.value.id === selection.embeddingTaskId,
                )?.value;
                if (selection.embeddingTaskId) {
                    if (embedding?.kind !== 'memory_embedding' || !embedding.embedding_dimensions)
                        throw new SetupError(t('importSetup.embeddingRequired'));
                    memory.embedding_task = embedding.id;
                    memory.similarity_weight = Math.max(
                        0,
                        1 -
                            (hintNumber(hints, 'memory_recent_memory_ratio') ?? 0.6) -
                            (hintNumber(hints, 'memory_extra_summarization_ratio') ?? 0),
                    );
                }
                if (!(await services.appController.upsertProviderGenerationPreset(generation)))
                    throw new SetupError(
                        t('orchestration.imported_compatibility.error.summary_preset'),
                    );
                if (!this.current(epoch)) return false;
                await client.upsertTaskProfile({
                    value: task,
                    expected_revision:
                        tasks.find((item) => item.value.id === task.id)?.revision ?? null,
                });
                if (!this.current(epoch)) return false;
                await client.upsertMemoryProfile({
                    value: memory,
                    expected_revision:
                        memories.find((item) => item.value.id === memory.id)?.revision ?? null,
                });
                if (!this.current(epoch)) return false;
                if (target) {
                    await client.upsertPromptPreset({
                        value: { ...target.value, memory_profile_id: memory.id },
                        expected_revision: target.revision,
                    });
                    promptId = target.value.id;
                    generationId = target.value.default_generation_preset_id;
                }
            } else {
                if (!(await services.appController.upsertProviderGenerationPreset(generation)))
                    throw new SetupError(
                        t('orchestration.imported_compatibility.error.parameters'),
                    );
                if (!this.current(epoch)) return false;
                await client.upsertPromptPreset({
                    value: { ...source.value, default_generation_preset_id: presetId },
                    expected_revision: source.revision,
                });
            }
            if (!this.current(epoch)) return false;
            if (selection.applyToRoom && context) {
                if (roomContext(this.services()) !== context)
                    throw new SetupError(t('importSetup.roomChanged'));
                const app = this.services().appState;
                const conversationId = app.selected_conversation?.id;
                const branchId = app.conversation_state?.active_branch_id;
                if (!conversationId || !branchId)
                    throw new SetupError(t('importSetup.roomChanged'));
                const orchestration = services.orchestrationController;
                await orchestration.loadContext(conversationId, branchId);
                if (!this.current(epoch) || roomContext(this.services()) !== context)
                    throw new SetupError(t('importSetup.roomChanged'));
                orchestration.stageRoomConfig({
                    prompt_preset_id: promptId,
                    ...(generationId ? { generation_preset_id: generationId } : {}),
                    ...(selection.kind === 'memory' ? { memory_enabled: true } : {}),
                });
                if (!(await orchestration.saveRoomConfig()))
                    throw new SetupError(
                        t('orchestration.imported_compatibility.error.room_binding'),
                    );
            }
            return this.current(epoch);
        } catch (error) {
            if (this.current(epoch)) this.fail(error);
            return false;
        } finally {
            if (this.current(epoch)) this.state.update((s) => ({ ...s, saving: false }));
        }
    }
    private fail(error: unknown): void {
        this.state.update((s) => ({
            ...s,
            error: error instanceof SetupError ? error.message : errorLabel(error),
        }));
    }
    private current(epoch: number): boolean {
        return !this.disposed && this.epoch === epoch;
    }
    destroy(): void {
        this.disposed = true;
        this.epoch++;
    }
}
function roomContext(services: SettingsServices): string | null {
    const conversation = services.appState.selected_conversation?.id;
    const branch = services.appState.conversation_state?.active_branch_id;
    return conversation && branch ? JSON.stringify([conversation, branch]) : null;
}
async function importedSetupId(source: string, suffix: string): Promise<string> {
    const digest = await globalThis.crypto.subtle.digest(
        'SHA-256',
        new TextEncoder().encode(source),
    );
    const hash = [...new Uint8Array(digest)]
        .map((byte) => byte.toString(16).padStart(2, '0'))
        .join('');
    return `imported-${hash}-${suffix}`;
}

class SetupError extends Error {}

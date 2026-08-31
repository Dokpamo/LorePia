import { t } from '../../../lib/i18n';
import type {
    CreatorPromptPresetDocumentDto,
    TaskProfileDocumentDto,
} from '../../../lib/ipc/contracts';

import {
    errorLabel,
    taskProfileValidationError,
    type EditablePromptBlockPatch,
    type OrchestrationCapableClient,
} from './orchestration-state';
import type { OrchestrationStateController } from './orchestration-state-controller';

export class PromptTaskController {
    constructor(
        private readonly client: OrchestrationCapableClient,
        private readonly state: OrchestrationStateController,
    ) {}

    async loadEditablePromptPresetForContext(
        contextKey: string,
        promptPresetId: string | null,
    ): Promise<void> {
        const loader = this.client.getEditablePromptPreset;
        if (promptPresetId === null || loader === undefined) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                editable_prompt_preset: null,
                editable_prompt_preset_dirty: false,
                editable_prompt_preset_loading: false,
                editable_prompt_preset_error:
                    promptPresetId !== null && loader === undefined
                        ? t('orchestration.error.unsupported_block_edit')
                        : null,
            }));
            return;
        }
        this.state.updateForContext(contextKey, (state) => ({
            ...state,
            editable_prompt_preset_loading: true,
            editable_prompt_preset_error: null,
        }));
        try {
            const document = await loader.call(this.client, {
                prompt_preset_id: promptPresetId,
            });
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                editable_prompt_preset: document,
                editable_prompt_preset_dirty: false,
                editable_prompt_preset_loading: false,
                editable_prompt_preset_error: null,
            }));
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                editable_prompt_preset: null,
                editable_prompt_preset_dirty: false,
                editable_prompt_preset_loading: false,
                editable_prompt_preset_error: errorLabel(error),
            }));
        }
    }

    async loadEditableTaskProfilesForContext(contextKey: string): Promise<void> {
        const loader = this.client.listTaskProfiles;
        if (loader === undefined) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                editable_task_profiles: [],
                editable_task_profiles_loading: false,
                editable_task_profiles_error: t('orchestration.error.unsupported_task_edit'),
            }));
            return;
        }
        this.state.updateForContext(contextKey, (state) => ({
            ...state,
            editable_task_profiles_loading: true,
            editable_task_profiles_error: null,
        }));
        try {
            const profiles = await loader.call(this.client);
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                editable_task_profiles: profiles.map(({ value, revision }) => ({
                    value,
                    expected_revision: revision,
                    dirty: false,
                })),
                editable_task_profiles_loading: false,
                editable_task_profiles_error: null,
            }));
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                editable_task_profiles: [],
                editable_task_profiles_loading: false,
                editable_task_profiles_error: errorLabel(error),
            }));
        }
    }
    stageEditablePromptBlock(blockId: string, patch: EditablePromptBlockPatch): boolean {
        const state = this.state.snapshot();
        const document = state.editable_prompt_preset;
        if (state.phase !== 'ready' || document === null) return false;
        const index = document.value.blocks.findIndex((block) => block.id === blockId);
        if (index < 0) return false;
        this.state.invalidatePlanPreviewForContext(state.context_key);
        this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_prompt_preset: {
                ...document,
                value: {
                    ...document.value,
                    blocks: document.value.blocks.map((block) =>
                        block.id === blockId ? { ...block, ...patch } : block,
                    ),
                },
            },
            editable_prompt_preset_dirty: true,
            editable_prompt_preset_error: null,
            workspace: {
                ...current.workspace,
                plan_preview: null,
            },
        }));
        return true;
    }

    setEditablePromptCacheBoundary(blockId: string, enabled: boolean): boolean {
        const state = this.state.snapshot();
        const document = state.editable_prompt_preset;
        if (state.phase !== 'ready' || document === null) return false;
        if (!document.value.blocks.some((block) => block.id === blockId)) return false;
        const existing = document.value.cache_boundaries.filter(
            (boundary) => boundary.after_block_id === blockId,
        );
        if ((enabled && existing.length > 0) || (!enabled && existing.length === 0)) return true;
        this.state.invalidatePlanPreviewForContext(state.context_key);
        let cacheBoundaries = document.value.cache_boundaries.filter(
            (boundary) => boundary.after_block_id !== blockId,
        );
        if (enabled) {
            const usedIds = new Set(cacheBoundaries.map(({ id }) => id));
            const baseId = `cache-${blockId}`.slice(0, 240);
            let candidate = baseId;
            let suffix = 2;
            while (usedIds.has(candidate)) {
                candidate = `${baseId}-${String(suffix)}`;
                suffix += 1;
            }
            cacheBoundaries = [
                ...cacheBoundaries,
                {
                    id: candidate,
                    after_block_id: blockId,
                    role_filter: { kind: 'all' },
                    ttl: 'provider_default',
                    mode: 'automatic',
                },
            ];
        }
        this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_prompt_preset: {
                ...document,
                value: {
                    ...document.value,
                    cache_boundaries: cacheBoundaries,
                },
            },
            editable_prompt_preset_dirty: true,
            editable_prompt_preset_error: null,
            workspace: {
                ...current.workspace,
                plan_preview: null,
            },
        }));
        return true;
    }

    stageEditablePromptCacheBoundary(
        blockId: string,
        patch: Partial<
            Pick<
                CreatorPromptPresetDocumentDto['cache_boundaries'][number],
                'role_filter' | 'ttl' | 'mode'
            >
        >,
    ): boolean {
        const state = this.state.snapshot();
        const document = state.editable_prompt_preset;
        if (document === null) return false;
        const boundary = document.value.cache_boundaries.find(
            (candidate) => candidate.after_block_id === blockId,
        );
        if (boundary === undefined) return false;
        this.state.invalidatePlanPreviewForContext(state.context_key);
        this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_prompt_preset: {
                ...document,
                value: {
                    ...document.value,
                    cache_boundaries: document.value.cache_boundaries.map((candidate) =>
                        candidate.id === boundary.id ? { ...candidate, ...patch } : candidate,
                    ),
                },
            },
            editable_prompt_preset_dirty: true,
            editable_prompt_preset_error: null,
            workspace: {
                ...current.workspace,
                plan_preview: null,
            },
        }));
        return true;
    }

    async reloadEditablePromptPreset(): Promise<void> {
        const state = this.state.snapshot();
        await this.loadEditablePromptPresetForContext(
            state.context_key,
            state.workspace.room_config.prompt_preset_id,
        );
    }

    async saveEditablePromptPreset(): Promise<boolean> {
        const state = this.state.snapshot();
        const document = state.editable_prompt_preset;
        const save = this.client.upsertPromptPreset;
        const reload = this.client.getEditablePromptPreset;
        if (document === null || !state.editable_prompt_preset_dirty) return false;
        if (save === undefined || reload === undefined) {
            this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_prompt_preset_error: t('orchestration.error.unsupported_block_save'),
            }));
            return false;
        }
        const contextKey = state.context_key;
        this.state.updateForContext(contextKey, (current) => ({
            ...current,
            editable_prompt_preset_loading: true,
            editable_prompt_preset_error: null,
        }));
        try {
            const summary = await save.call(this.client, {
                value: document.value,
                expected_revision: document.revision,
            });
            const refreshed = await reload.call(this.client, {
                prompt_preset_id: document.value.id,
            });
            if (!this.state.invalidatePlanPreviewForContext(contextKey)) return false;
            return this.state.updateForContext(contextKey, (current) => {
                const currentDocument = current.editable_prompt_preset;
                const hasNewerDraft =
                    current.editable_prompt_preset_dirty &&
                    currentDocument !== null &&
                    currentDocument !== document;
                return {
                    ...current,
                    editable_prompt_preset: hasNewerDraft
                        ? {
                              ...refreshed,
                              value: currentDocument.value,
                          }
                        : refreshed,
                    editable_prompt_preset_dirty: hasNewerDraft,
                    editable_prompt_preset_loading: false,
                    editable_prompt_preset_error: null,
                    announcement: hasNewerDraft
                        ? t('orchestration.notice.preset_saved_partial', {
                              name: summary.value.name,
                          })
                        : t('orchestration.notice.preset_saved', { name: summary.value.name }),
                    workspace: {
                        ...current.workspace,
                        prompt_preset_revision: refreshed.revision,
                        prompt_presets: current.workspace.prompt_presets.map((preset) =>
                            preset.id === summary.value.id ? summary.value : preset,
                        ),
                        plan_preview: null,
                    },
                };
            });
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (current) => ({
                ...current,
                editable_prompt_preset_loading: false,
                editable_prompt_preset_error: errorLabel(error),
            }));
            return false;
        }
    }

    addTaskProfileDraft(taskProfileId: string): boolean {
        const state = this.state.snapshot();
        const id = taskProfileId.trim();
        if (
            state.phase !== 'ready' ||
            id === '' ||
            id.length > 256 ||
            state.editable_task_profiles.some((profile) => profile.value.id === id)
        ) {
            return false;
        }
        this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_task_profiles: [
                ...current.editable_task_profiles,
                {
                    value: {
                        id,
                        kind: 'memory_summary',
                        route_id: '',
                        generation_preset_id: '',
                        fallback_route_ids: [],
                        embedding_dimensions: null,
                        timeout_ms: 30_000,
                        rate_limit: { requests: 1, per_seconds: 60 },
                        concurrency_limit: 1,
                    },
                    expected_revision: null,
                    dirty: true,
                },
            ],
            editable_task_profiles_error: null,
        }));
        return true;
    }

    stageTaskProfile(taskProfileId: string, patch: Partial<TaskProfileDocumentDto>): boolean {
        const state = this.state.snapshot();
        if (!state.editable_task_profiles.some((profile) => profile.value.id === taskProfileId)) {
            return false;
        }
        this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_task_profiles: current.editable_task_profiles.map((profile) =>
                profile.value.id === taskProfileId
                    ? (() => {
                          const nextValue = {
                              ...profile.value,
                              ...patch,
                              id: taskProfileId,
                          };
                          if (nextValue.kind === 'memory_embedding') {
                              nextValue.fallback_route_ids = [];
                          } else {
                              nextValue.embedding_dimensions = null;
                          }
                          return {
                              ...profile,
                              value: nextValue,
                              dirty: true,
                          };
                      })()
                    : profile,
            ),
            editable_task_profiles_error: null,
        }));
        return true;
    }

    async saveTaskProfile(taskProfileId: string): Promise<boolean> {
        const state = this.state.snapshot();
        const profile = state.editable_task_profiles.find(
            (candidate) => candidate.value.id === taskProfileId,
        );
        const save = this.client.upsertTaskProfile;
        if (!profile?.dirty) return false;
        const validationError = taskProfileValidationError(profile.value);
        if (validationError !== null) {
            this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_task_profiles_error: validationError,
            }));
            return false;
        }
        if (save === undefined) {
            this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_task_profiles_error: t('orchestration.error.unsupported_task_save'),
            }));
            return false;
        }
        const contextKey = state.context_key;
        this.state.updateForContext(contextKey, (current) => ({
            ...current,
            editable_task_profiles_loading: true,
            editable_task_profiles_error: null,
        }));
        try {
            const saved = await save.call(this.client, {
                value: profile.value,
                expected_revision: profile.expected_revision,
            });
            return this.state.updateForContext(contextKey, (current) => {
                const currentProfile = current.editable_task_profiles.find(
                    (candidate) => candidate.value.id === taskProfileId,
                );
                const hasNewerDraft =
                    currentProfile !== undefined &&
                    currentProfile !== profile &&
                    currentProfile.dirty;
                return {
                    ...current,
                    editable_task_profiles: current.editable_task_profiles.map((candidate) =>
                        candidate.value.id === taskProfileId
                            ? hasNewerDraft
                                ? {
                                      value: candidate.value,
                                      expected_revision: saved.revision,
                                      dirty: true,
                                  }
                                : {
                                      value: saved.value,
                                      expected_revision: saved.revision,
                                      dirty: false,
                                  }
                            : candidate,
                    ),
                    editable_task_profiles_loading: false,
                    editable_task_profiles_error: null,
                    announcement: hasNewerDraft
                        ? t('orchestration.notice.task_saved_partial', { name: saved.value.id })
                        : t('orchestration.notice.task_saved', { name: saved.value.id }),
                };
            });
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (current) => ({
                ...current,
                editable_task_profiles_loading: false,
                editable_task_profiles_error: errorLabel(error),
            }));
            return false;
        }
    }

    async deleteTaskProfile(taskProfileId: string): Promise<boolean> {
        const state = this.state.snapshot();
        const profile = state.editable_task_profiles.find(
            (candidate) => candidate.value.id === taskProfileId,
        );
        if (profile === undefined) return false;
        if (profile.expected_revision === null) {
            return this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_task_profiles: current.editable_task_profiles.filter(
                    (candidate) => candidate.value.id !== taskProfileId,
                ),
            }));
        }
        const remove = this.client.deleteTaskProfile;
        if (remove === undefined) {
            this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_task_profiles_error: t('orchestration.error.unsupported_task_delete'),
            }));
            return false;
        }
        const contextKey = state.context_key;
        this.state.updateForContext(contextKey, (current) => ({
            ...current,
            editable_task_profiles_loading: true,
            editable_task_profiles_error: null,
        }));
        try {
            await remove.call(this.client, {
                task_profile_id: taskProfileId,
                expected_revision: profile.expected_revision,
            });
            return this.state.updateForContext(contextKey, (current) => ({
                ...current,
                editable_task_profiles: current.editable_task_profiles.filter(
                    (candidate) => candidate.value.id !== taskProfileId,
                ),
                editable_task_profiles_loading: false,
                editable_task_profiles_error: null,
                announcement: t('orchestration.notice.task_deleted', { name: taskProfileId }),
            }));
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (current) => ({
                ...current,
                editable_task_profiles_loading: false,
                editable_task_profiles_error: errorLabel(error),
            }));
            return false;
        }
    }
}

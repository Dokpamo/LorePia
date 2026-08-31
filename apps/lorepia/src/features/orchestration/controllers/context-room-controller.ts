import { t } from '../../../lib/i18n';
import type { CreatorControlValue, OrchestrationWorkspaceDto } from '../../../lib/ipc/contracts';

import {
    INITIAL_ORCHESTRATION_STATE,
    boundedWorkspace,
    emptyOrchestrationWorkspace,
    errorLabel,
    roomPromptSourceValidationError,
    validateInteractionProposalPage,
    type OrchestrationCapableClient,
    type RoomOrchestrationConfigPatch,
} from './orchestration-state';
import type { CreatorDocumentController } from './creator-document-controller';
import type { OrchestrationStateController } from './orchestration-state-controller';
import type { PromptTaskController } from './prompt-task-controller';

export class ContextRoomController {
    constructor(
        private readonly client: OrchestrationCapableClient,
        private readonly state: OrchestrationStateController,
        private readonly promptTasks: PromptTaskController,
        private readonly creatorDocuments: CreatorDocumentController,
    ) {}

    async loadContext(conversationId: string | null, branchId: string | null): Promise<void> {
        const epoch = this.state.beginContextLoad();
        if (conversationId === null || branchId === null) {
            this.state.set(structuredClone(INITIAL_ORCHESTRATION_STATE));
            return;
        }

        const contextKey = `${conversationId}:${branchId}`;
        const loader = this.client.getOrchestrationWorkspace;
        if (loader === undefined) {
            this.state.set({
                ...structuredClone(INITIAL_ORCHESTRATION_STATE),
                phase: 'unavailable',
                context_key: contextKey,
                error: t('orchestration.error.unsupported'),
                workspace: emptyOrchestrationWorkspace(conversationId, branchId),
            });
            return;
        }

        this.state.update((state) => ({
            ...state,
            phase: 'loading',
            saving: false,
            busy_interaction_proposal_id: null,
            error: null,
            announcement: '',
            context_key: contextKey,
            dirty_room_config: false,
            workspace: emptyOrchestrationWorkspace(conversationId, branchId),
            plan_operation_nonce: null,
            plan_generation_attempt_id: null,
            plan_preview_request: null,
            knowledge_simulation: null,
            transform_preview: null,
            editable_prompt_preset: null,
            editable_prompt_preset_dirty: false,
            editable_prompt_preset_loading: false,
            editable_prompt_preset_error: null,
            editable_task_profiles: [],
            editable_task_profiles_loading: false,
            editable_task_profiles_error: null,
            editable_memory_profiles: [],
            editable_knowledge_books: [],
            editable_transform_sets: [],
            editable_interaction_rule_sets: [],
            editable_content_modules: [],
            editable_creator_documents_loading: false,
            editable_creator_documents_error: null,
            list_truncation: {
                prompt_blocks: false,
                memory_records: false,
                selection_evidence: false,
                content_modules: false,
            },
        }));
        try {
            const snapshot = await loader.call(this.client, conversationId, branchId);
            if (!this.state.isContextEpoch(epoch)) return;
            const response: OrchestrationWorkspaceDto = {
                ...emptyOrchestrationWorkspace(conversationId, branchId),
                ...snapshot,
            };
            const expireProposals = this.client.expireInteractionProposals;
            const listProposals = this.client.listInteractionProposals;
            if (expireProposals !== undefined && listProposals !== undefined) {
                const expiry = await expireProposals.call(this.client, {
                    conversation_id: conversationId,
                    branch_id: branchId,
                    limit: 100,
                });
                if (
                    expiry.conversation_id !== conversationId ||
                    expiry.branch_id !== branchId ||
                    !Number.isSafeInteger(expiry.current_state_revision) ||
                    expiry.current_state_revision < 0 ||
                    expiry.has_more_expired
                ) {
                    throw new Error('interaction proposal expiry authority validation failed');
                }
                validateInteractionProposalPage(
                    expiry.expired_proposals,
                    conversationId,
                    branchId,
                    'expired',
                );
                const pending = await listProposals.call(this.client, {
                    conversation_id: conversationId,
                    branch_id: branchId,
                    status: 'pending',
                    limit: 100,
                });
                validateInteractionProposalPage(pending, conversationId, branchId, 'pending');
                if (pending.some((item) => item.state_revision !== expiry.current_state_revision)) {
                    throw new Error('interaction proposal state revision changed during refresh');
                }
                response.interaction_state_revision = expiry.current_state_revision;
                response.interaction_proposals = pending;
            }
            if (!this.state.isContextEpoch(epoch)) return;
            const promptSourceError = roomPromptSourceValidationError(response.room_config);
            if (promptSourceError !== null) {
                throw new Error(
                    t('orchestration.error.prompt_source_limit', { detail: promptSourceError }),
                );
            }
            const bounded = boundedWorkspace(response);
            this.state.update((state) => ({
                ...state,
                phase: 'ready',
                error: null,
                workspace: bounded.workspace,
                list_truncation: bounded.truncation,
            }));
            await Promise.all([
                this.promptTasks.loadEditablePromptPresetForContext(
                    contextKey,
                    response.room_config.prompt_preset_id,
                ),
                this.promptTasks.loadEditableTaskProfilesForContext(contextKey),
                this.creatorDocuments.loadEditableCreatorDocumentsForContext(contextKey),
            ]);
        } catch (error: unknown) {
            if (!this.state.isContextEpoch(epoch)) return;
            this.state.update((state) => ({
                ...state,
                phase: 'error',
                error: errorLabel(error),
            }));
        }
    }

    stageRoomConfig(patch: RoomOrchestrationConfigPatch): void {
        const state = this.state.snapshot();
        if (state.phase !== 'ready') return;
        const supported = state.workspace.room_config.supported_fields;
        const accepted: RoomOrchestrationConfigPatch = {};
        if (patch.prompt_preset_id !== undefined && supported.prompt_preset_id) {
            accepted.prompt_preset_id = patch.prompt_preset_id;
        }
        if (patch.generation_preset_id !== undefined && supported.generation_preset_id) {
            accepted.generation_preset_id = patch.generation_preset_id;
        }
        if (patch.creator_values !== undefined && supported.creator_values) {
            accepted.creator_values = patch.creator_values;
        }
        if (patch.variable_overrides !== undefined && supported.variable_overrides) {
            accepted.variable_overrides = patch.variable_overrides;
        }
        if (patch.response_length !== undefined && supported.response_length) {
            accepted.response_length = patch.response_length;
        }
        if (patch.creativity !== undefined && supported.creativity) {
            accepted.creativity = patch.creativity;
        }
        if (patch.reasoning_effort !== undefined && supported.reasoning_effort) {
            accepted.reasoning_effort = patch.reasoning_effort;
        }
        if (patch.memory_enabled !== undefined && supported.memory_enabled) {
            accepted.memory_enabled = patch.memory_enabled;
        }
        if (patch.knowledge_enabled !== undefined && supported.knowledge_enabled) {
            accepted.knowledge_enabled = patch.knowledge_enabled;
        }
        if (patch.user_name_override !== undefined && supported.user_name_override) {
            accepted.user_name_override = patch.user_name_override;
        }
        if (patch.author_note !== undefined && supported.author_note) {
            accepted.author_note = patch.author_note;
        }
        if (patch.group_context !== undefined && supported.group_context) {
            accepted.group_context = patch.group_context;
        }
        if (patch.template_slots !== undefined && supported.template_slots) {
            accepted.template_slots = structuredClone(patch.template_slots);
        }
        if (Object.keys(accepted).length === 0) return;
        this.state.bumpRoomDraftEpoch();
        this.state.invalidatePlanPreviewForContext(state.context_key);
        this.state.update((current) => ({
            ...current,
            dirty_room_config: true,
            workspace: {
                ...current.workspace,
                room_config: {
                    ...current.workspace.room_config,
                    ...accepted,
                },
                plan_preview: null,
            },
        }));
        if (accepted.prompt_preset_id !== undefined) {
            void this.promptTasks.loadEditablePromptPresetForContext(
                state.context_key,
                accepted.prompt_preset_id,
            );
        }
    }

    stageCreatorControl(controlId: string, value: CreatorControlValue): void {
        const state = this.state.snapshot();
        this.stageRoomConfig({
            creator_values: {
                ...state.workspace.room_config.creator_values,
                [controlId]: value,
            },
        });
    }

    async saveRoomConfig(): Promise<boolean> {
        const saver = this.client.saveRoomOrchestrationConfig;
        if (saver === undefined) {
            this.state.update((state) => ({
                ...state,
                phase: 'unavailable',
                error: t('orchestration.error.unsupported_room_save'),
            }));
            return false;
        }
        const state = this.state.snapshot();
        if (state.phase !== 'ready' || state.saving) return false;
        const contextKey = state.context_key;
        const contextEpoch = this.state.currentContextEpoch();
        const draftEpoch = this.state.currentRoomDraftEpoch();
        const config = structuredClone(state.workspace.room_config);
        const promptSourceError = roomPromptSourceValidationError(config);
        if (promptSourceError !== null) {
            this.state.updateForContext(contextKey, (current) => ({
                ...current,
                error: promptSourceError,
            }));
            return false;
        }
        const input = {
            conversation_id: config.conversation_id,
            branch_id: config.branch_id,
            prompt_preset_id: config.prompt_preset_id,
            generation_preset_id: config.generation_preset_id,
            creator_values: structuredClone(config.creator_values),
            variable_overrides: structuredClone(config.variable_overrides),
            response_length: config.response_length,
            creativity: config.creativity,
            reasoning_effort: config.reasoning_effort,
            memory_enabled: config.memory_enabled,
            knowledge_enabled: config.knowledge_enabled,
            user_name_override: config.user_name_override,
            author_note: config.author_note,
            group_context: config.group_context,
            template_slots: structuredClone(config.template_slots),
            expected_revision: state.workspace.room_config_revision,
        };
        this.state.update((state) => ({ ...state, saving: true, error: null }));
        try {
            const saved = await saver.call(this.client, input);
            if (!this.state.isContextEpoch(contextEpoch)) return false;
            this.state.invalidatePlanPreviewForContext(contextKey);
            return this.state.updateForContext(contextKey, (current) => {
                if (!this.state.isRoomDraftEpoch(draftEpoch)) {
                    return {
                        ...current,
                        saving: false,
                        dirty_room_config: true,
                        announcement: t('orchestration.notice.unsaved_changes'),
                        workspace: {
                            ...current.workspace,
                            room_config: {
                                ...current.workspace.room_config,
                                conversation_id: saved.room_config.conversation_id,
                                branch_id: saved.room_config.branch_id,
                                supported_fields: saved.room_config.supported_fields,
                            },
                            room_config_revision: saved.revision,
                            generation_target: saved.generation_target,
                            plan_preview: null,
                        },
                    };
                }
                return {
                    ...current,
                    saving: false,
                    dirty_room_config: false,
                    announcement: t('orchestration.notice.room_saved'),
                    workspace: {
                        ...current.workspace,
                        room_config: saved.room_config,
                        room_config_revision: saved.revision,
                        generation_target: saved.generation_target,
                        plan_preview: null,
                    },
                };
            });
        } catch (error: unknown) {
            if (!this.state.isContextEpoch(contextEpoch)) return false;
            this.state.updateForContext(contextKey, (current) => ({
                ...current,
                saving: false,
                error: errorLabel(error),
            }));
            return false;
        }
    }
    async reloadContextIfCurrent(contextKey: string): Promise<void> {
        if (!this.state.isCurrentContext(contextKey)) return;
        const { conversation_id: conversationId, branch_id: branchId } =
            this.state.snapshot().workspace.room_config;
        if (conversationId !== '' && branchId !== '') {
            await this.loadContext(conversationId, branchId);
        }
    }
}

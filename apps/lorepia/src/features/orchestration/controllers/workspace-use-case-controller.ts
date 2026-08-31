import { t } from '../../../lib/i18n';
import type {
    MemoryRecordExclusionScope,
    MemoryRecordPatchInput,
    OrchestrationWorkspaceDto,
    PromptBlockDto,
} from '../../../lib/ipc/contracts';

import {
    MAX_VISIBLE_PROMPT_BLOCKS,
    MAX_VISIBLE_SELECTION_EVIDENCE,
    MEMORY_RECORD_RESPONSE_AUTHORITY_ERROR,
    errorLabel,
    moveBlockByDrop,
    type OrchestrationCapableClient,
} from './orchestration-state';
import type { ContextRoomController } from './context-room-controller';
import type { OrchestrationStateController } from './orchestration-state-controller';

export class WorkspaceUseCaseController {
    constructor(
        private readonly client: OrchestrationCapableClient,
        private readonly state: OrchestrationStateController,
        private readonly context: ContextRoomController,
    ) {}

    async movePromptBlock(blockId: string, direction: -1 | 1): Promise<boolean> {
        const state = this.state.snapshot();
        const currentIndex = state.workspace.prompt_blocks.findIndex(
            (block) => block.id === blockId,
        );
        if (currentIndex < 0) return false;
        const current = state.workspace.prompt_blocks[currentIndex];
        if (!current?.order_editable) return false;
        const zoneIndexes = state.workspace.prompt_blocks.flatMap((block, index) =>
            block.placement_zone === current.placement_zone ? [index] : [],
        );
        const positionInZone = zoneIndexes.indexOf(currentIndex);
        const nextIndex = zoneIndexes[positionInZone + direction];
        if (nextIndex === undefined) return false;
        const reordered = [...state.workspace.prompt_blocks];
        const target = reordered[nextIndex];
        if (!target?.order_editable) return false;
        reordered[currentIndex] = target;
        reordered[nextIndex] = current;
        return this.persistPromptOrder(reordered, current.name);
    }

    async movePromptBlockTo(blockId: string, targetId: string): Promise<boolean> {
        const state = this.state.snapshot();
        const reordered = moveBlockByDrop(state.workspace.prompt_blocks, blockId, targetId);
        if (reordered === state.workspace.prompt_blocks) return false;
        const moved = reordered.find((block) => block.id === blockId);
        return this.persistPromptOrder(reordered, moved?.name ?? t('orchestration.label.prompt'));
    }

    private async persistPromptOrder(
        reordered: PromptBlockDto[],
        movedName: string,
    ): Promise<boolean> {
        const state = this.state.snapshot();
        const contextKey = state.context_key;
        const presetId = state.workspace.room_config.prompt_preset_id;
        const expectedRevision = state.workspace.prompt_preset_revision;
        const persist = this.client.reorderPromptBlocks;
        if (presetId === null || expectedRevision === null || persist === undefined) {
            this.state.updateForContext(contextKey, (current) => ({
                ...current,
                error: t('orchestration.error.unsupported_block_order'),
                announcement: t('orchestration.notice.order_failed'),
            }));
            return false;
        }
        try {
            const saved = await persist.call(this.client, {
                prompt_preset_id: presetId,
                ordered_block_ids: reordered.map((block) => block.id),
                expected_revision: expectedRevision,
            });
            if (!this.state.invalidatePlanPreviewForContext(contextKey)) return false;
            return this.state.updateForContext(contextKey, (current) => ({
                ...current,
                announcement: t('orchestration.notice.order_saved', { name: movedName }),
                error: null,
                workspace: {
                    ...current.workspace,
                    prompt_blocks: saved.blocks.slice(0, MAX_VISIBLE_PROMPT_BLOCKS),
                    prompt_preset_revision: saved.revision,
                    plan_preview: null,
                },
            }));
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (current) => ({
                ...current,
                error: errorLabel(error),
            }));
            await this.context.reloadContextIfCurrent(contextKey);
            return false;
        }
    }

    async simulateKnowledge(sampleText: string): Promise<boolean> {
        const state = this.state.snapshot();
        const contextKey = state.context_key;
        const simulate = this.client.simulateKnowledge;
        const knowledgeBookId = state.workspace.knowledge_book_ids[0] ?? null;
        if (simulate === undefined || knowledgeBookId === null || sampleText.trim() === '') {
            if (simulate === undefined) {
                this.state.updateForContext(contextKey, (current) => ({
                    ...current,
                    error: t('orchestration.error.unsupported_knowledge_sim'),
                }));
            } else if (knowledgeBookId === null) {
                this.state.updateForContext(contextKey, (current) => ({
                    ...current,
                    error: t('orchestration.error.no_knowledge_book'),
                }));
            }
            return false;
        }
        try {
            const simulation = await simulate.call(this.client, {
                knowledge_book_id: knowledgeBookId,
                sample_text: sampleText,
                variables: structuredClone(state.workspace.room_config.variable_overrides),
            });
            return this.state.updateForContext(contextKey, (current) => ({
                ...current,
                knowledge_simulation: {
                    ...simulation,
                    entries: simulation.entries.slice(0, MAX_VISIBLE_SELECTION_EVIDENCE),
                    truncated:
                        simulation.truncated ||
                        simulation.entries.length > MAX_VISIBLE_SELECTION_EVIDENCE,
                },
                error: null,
            }));
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (current) => ({
                ...current,
                error: errorLabel(error),
            }));
            return false;
        }
    }

    async previewTransform(ruleId: string, sampleText: string): Promise<boolean> {
        const snapshot = this.state.snapshot();
        const contextKey = snapshot.context_key;
        const preview = this.client.previewTransform;
        const matchingTransformSets = snapshot.editable_transform_sets.filter((document) =>
            document.value.rules.some((rule) => rule.id === ruleId),
        );
        if (preview === undefined || ruleId === '' || sampleText === '') {
            if (preview === undefined) {
                this.state.updateForContext(contextKey, (state) => ({
                    ...state,
                    error: t('orchestration.error.unsupported_transform_preview'),
                }));
            }
            return false;
        }
        if (matchingTransformSets.length !== 1) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error:
                    matchingTransformSets.length === 0
                        ? t('orchestration.error.rule_not_found')
                        : t('orchestration.error.rule_ambiguous'),
            }));
            return false;
        }
        const transformSet = matchingTransformSets[0];
        if (transformSet === undefined) return false;
        try {
            const result = await preview.call(this.client, {
                transform_set_id: transformSet.value.id,
                rule_id: ruleId,
                sample_text: sampleText,
                variables: structuredClone(snapshot.workspace.room_config.variable_overrides),
            });
            if (
                result.transform_set_id !== transformSet.value.id ||
                result.rule_id !== ruleId ||
                result.reports.some((report) => report.trace.rule_id !== ruleId)
            ) {
                throw new Error(
                    'Core transform preview authority did not match the requested rule.',
                );
            }
            return this.state.updateForContext(contextKey, (state) => ({
                ...state,
                transform_preview: result,
                error: null,
            }));
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error: errorLabel(error),
            }));
            return false;
        }
    }

    async updateMemoryRecord(recordId: string, patch: MemoryRecordPatchInput): Promise<boolean> {
        const snapshot = this.state.snapshot();
        const contextKey = snapshot.context_key;
        const record = snapshot.workspace.memory_records.find(
            (candidate) => candidate.id === recordId,
        );
        const update = this.client.patchMemoryRecord;
        if (record === undefined) return false;
        if (update === undefined) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error: t('orchestration.error.unsupported_memory_edit'),
            }));
            return false;
        }
        try {
            const saved = await update.call(this.client, {
                conversation_id: record.conversation_id,
                branch_id: record.branch_id,
                memory_record_id: recordId,
                patch,
                expected_revision: record.revision,
            });
            return this.replaceMemoryRecord(
                contextKey,
                recordId,
                record.revision,
                saved,
                t('orchestration.notice.memory_updated'),
            );
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error: errorLabel(error),
                announcement: '',
            }));
            return false;
        }
    }

    async deleteMemoryRecord(recordId: string): Promise<boolean> {
        const snapshot = this.state.snapshot();
        const contextKey = snapshot.context_key;
        const record = snapshot.workspace.memory_records.find(
            (candidate) => candidate.id === recordId,
        );
        const remove = this.client.deleteMemoryRecord;
        if (record === undefined) return false;
        if (remove === undefined) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error: t('orchestration.error.unsupported_memory_delete'),
            }));
            return false;
        }
        try {
            await remove.call(this.client, {
                conversation_id: record.conversation_id,
                branch_id: record.branch_id,
                memory_record_id: recordId,
                expected_revision: record.revision,
            });
            if (!this.state.invalidatePlanPreviewForContext(contextKey)) return false;
            return this.state.updateForContext(contextKey, (state) => ({
                ...state,
                announcement: t('orchestration.notice.memory_deleted'),
                workspace: {
                    ...state.workspace,
                    memory_records: state.workspace.memory_records.filter(
                        (record) => record.id !== recordId,
                    ),
                    plan_preview: null,
                },
                error: null,
            }));
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error: errorLabel(error),
                announcement: '',
            }));
            return false;
        }
    }

    async setMemoryRecordPinned(recordId: string, pinned: boolean): Promise<boolean> {
        const snapshot = this.state.snapshot();
        const contextKey = snapshot.context_key;
        const record = snapshot.workspace.memory_records.find(
            (candidate) => candidate.id === recordId,
        );
        const update = this.client.patchMemoryRecord;
        if (record === undefined) return false;
        if (update === undefined) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error: t('orchestration.error.unsupported_memory_pin'),
            }));
            return false;
        }
        try {
            const saved = await update.call(this.client, {
                conversation_id: record.conversation_id,
                branch_id: record.branch_id,
                memory_record_id: recordId,
                patch: { pinned },
                expected_revision: record.revision,
            });
            return this.replaceMemoryRecord(
                contextKey,
                recordId,
                record.revision,
                saved,
                t('orchestration.notice.memory_pinned'),
            );
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error: errorLabel(error),
                announcement: '',
            }));
            return false;
        }
    }

    async setMemoryRecordExclusion(
        recordId: string,
        scope: MemoryRecordExclusionScope,
        excluded: boolean,
    ): Promise<boolean> {
        const snapshot = this.state.snapshot();
        const contextKey = snapshot.context_key;
        const record = snapshot.workspace.memory_records.find(
            (candidate) => candidate.id === recordId,
        );
        const update = this.client.setMemoryRecordExclusion;
        if (record === undefined) return false;
        if (update === undefined) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error: t('orchestration.error.unsupported_memory_exclusion'),
            }));
            return false;
        }
        try {
            const saved = await update.call(this.client, {
                conversation_id: record.conversation_id,
                branch_id: record.branch_id,
                memory_record_id: recordId,
                scope,
                excluded,
                expected_revision: record.revision,
            });
            const label =
                scope === 'conversation'
                    ? t('orchestration.label.conversation_scope')
                    : t('orchestration.label.character_scope');
            return this.replaceMemoryRecord(
                contextKey,
                recordId,
                record.revision,
                saved,
                t('orchestration.notice.exclusion_changed', { scope: label }),
            );
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error: errorLabel(error),
                announcement: '',
            }));
            return false;
        }
    }

    private replaceMemoryRecord(
        contextKey: string,
        requestedRecordId: string,
        expectedRevision: number,
        saved: OrchestrationWorkspaceDto['memory_records'][number],
        announcement: string,
    ): boolean {
        let accepted = false;
        if (!this.state.invalidatePlanPreviewForContext(contextKey)) return false;
        const contextApplied = this.state.updateForContext(contextKey, (state) => {
            const currentRecord = state.workspace.memory_records.find(
                (record) => record.id === requestedRecordId,
            );
            if (
                currentRecord === undefined ||
                saved.id !== requestedRecordId ||
                !Number.isSafeInteger(saved.revision) ||
                saved.revision <= expectedRevision ||
                saved.revision <= currentRecord.revision
            ) {
                return {
                    ...state,
                    error: MEMORY_RECORD_RESPONSE_AUTHORITY_ERROR,
                    announcement: '',
                };
            }
            accepted = true;
            return {
                ...state,
                announcement,
                workspace: {
                    ...state.workspace,
                    memory_records: state.workspace.memory_records.map((record) =>
                        record.id === requestedRecordId ? saved : record,
                    ),
                    plan_preview: null,
                },
                error: null,
            };
        });
        return contextApplied && accepted;
    }
}

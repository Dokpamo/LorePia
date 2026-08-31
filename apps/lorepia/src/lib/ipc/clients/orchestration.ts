import type {
    ApplyPromptPresetRollbackInput,
    ContentModuleRevisionDiffDocumentDto,
    ContentModuleRevisionListResultDto,
    ContentShareGateDto,
    CreatorContentModuleDocumentDto,
    CreatorInteractionRuleSetDocumentDto,
    CreatorKnowledgeBookDocumentDto,
    CreatorMemoryProfileDocumentDto,
    CreatorPromptPresetDocumentDto,
    CreatorTransformSetDocumentDto,
    DeleteContentModuleInput,
    DeleteInteractionRuleSetInput,
    DeleteKnowledgeBookInput,
    DeleteMemoryProfileInput,
    DeleteMemoryRecordRequest,
    DeletePromptPresetInput,
    DeleteTaskProfileInput,
    DeleteTransformSetInput,
    DiffContentModuleRevisionsInput,
    DiffPromptPresetRevisionsInput,
    EvaluateContentModuleShareInput,
    GetMemoryRecordInput,
    GetContentModuleInput,
    GetInteractionRuleSetInput,
    GetKnowledgeBookInput,
    GetMemoryProfileInput,
    GetPromptPresetInput,
    GetTransformSetInput,
    KnowledgeActivationResultDto,
    KnowledgeSimulationDto,
    ListContentModuleBindingsInput,
    ListContentModuleRevisionsInput,
    ListMemoryRecordsInput,
    InterruptedMemoryJobDto,
    ListInterruptedMemoryJobsInput,
    MemoryJobRetryReceiptDto,
    RetryInterruptedMemoryJobInput,
    ListRetryableMemoryQueryEmbeddingsInput,
    ListPromptPresetBindingsInput,
    ListPromptPresetRevisionsInput,
    MemoryRecordDto,
    MemoryRecordListResultDto,
    MemoryQueryEmbeddingRetryCandidateDto,
    ModuleBindingDocumentDto,
    OrchestrationWorkspaceSnapshotDto,
    ExplainPromptPlanInput,
    PatchMemoryRecordRequest,
    PromptPlanPreviewDto,
    PromptPlanRequestInput,
    PromptPresetRevisionDiffDto,
    PromptPresetRevisionListDto,
    PromptPresetRollbackReceiptDto,
    PromptPresetRollbackReviewDto,
    PromptPresetSummaryDto,
    PromptResolutionTraceDto,
    PreviewTransformRequest,
    PreviewTransformRuleInput,
    ReorderPromptBlocksInput,
    ReorderPromptBlocksResult,
    RetryMemoryQueryEmbeddingInput,
    ReviewPromptPresetRollbackInput,
    PromptPresetBindingDocumentDto,
    RevisionedDto,
    SetMemoryRecordExclusionRequest,
    SaveRoomOrchestrationConfigInput,
    SaveRoomOrchestrationConfigResult,
    SimulateKnowledgeActivationInput,
    SimulateKnowledgeRequest,
    TaskProfileDocumentDto,
    TransformPreviewDto,
    TransformRulePreviewDto,
    UpsertContentModuleInput,
    UpsertInteractionRuleSetInput,
    UpsertKnowledgeBookInput,
    UpsertMemoryProfileInput,
    UpsertTaskProfileInput,
    UpsertPromptPresetInput,
    UpsertTransformSetInput,
} from '../contracts';

import { LOREPIA_COMMANDS } from '../commands';

import { DiscoveryClient } from './discovery';

export abstract class OrchestrationClient extends DiscoveryClient {
    getOrchestrationWorkspace(
        conversationId: string,
        branchId: string,
    ): Promise<OrchestrationWorkspaceSnapshotDto> {
        return this.call(LOREPIA_COMMANDS.getOrchestrationWorkspace, {
            request: {
                conversation_id: conversationId,
                branch_id: branchId,
            },
        });
    }

    saveRoomOrchestrationConfig(
        input: SaveRoomOrchestrationConfigInput,
    ): Promise<SaveRoomOrchestrationConfigResult> {
        return this.call(LOREPIA_COMMANDS.saveRoomOrchestrationConfig, { request: input });
    }

    resolvePromptPreview(input: PromptPlanRequestInput): Promise<PromptPlanPreviewDto> {
        return this.call(LOREPIA_COMMANDS.resolvePromptPreview, { request: input });
    }

    explainPromptPlan(input: ExplainPromptPlanInput): Promise<PromptResolutionTraceDto> {
        return this.call(LOREPIA_COMMANDS.explainPromptPlan, { request: input });
    }

    upsertPromptPreset(
        input: UpsertPromptPresetInput,
    ): Promise<RevisionedDto<PromptPresetSummaryDto>> {
        return this.call(LOREPIA_COMMANDS.upsertPromptPreset, { request: input });
    }

    getPromptPreset(input: GetPromptPresetInput): Promise<RevisionedDto<PromptPresetSummaryDto>> {
        return this.call(LOREPIA_COMMANDS.getPromptPreset, { request: input });
    }

    getEditablePromptPreset(
        input: GetPromptPresetInput,
    ): Promise<RevisionedDto<CreatorPromptPresetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getEditablePromptPreset, { request: input });
    }

    listPromptPresets(): Promise<RevisionedDto<PromptPresetSummaryDto>[]> {
        return this.call(LOREPIA_COMMANDS.listPromptPresets);
    }

    listPromptPresetRevisions(
        input: ListPromptPresetRevisionsInput,
    ): Promise<PromptPresetRevisionListDto> {
        return this.call(LOREPIA_COMMANDS.listPromptPresetRevisions, { request: input });
    }

    diffPromptPresetRevisions(
        input: DiffPromptPresetRevisionsInput,
    ): Promise<PromptPresetRevisionDiffDto> {
        return this.call(LOREPIA_COMMANDS.diffPromptPresetRevisions, { request: input });
    }

    reviewPromptPresetRollback(
        input: ReviewPromptPresetRollbackInput,
    ): Promise<PromptPresetRollbackReviewDto> {
        return this.call(LOREPIA_COMMANDS.reviewPromptPresetRollback, { request: input });
    }

    applyPromptPresetRollback(
        input: ApplyPromptPresetRollbackInput,
    ): Promise<PromptPresetRollbackReceiptDto> {
        return this.call(LOREPIA_COMMANDS.applyPromptPresetRollback, { request: input });
    }

    deletePromptPreset(
        input: DeletePromptPresetInput,
    ): Promise<RevisionedDto<PromptPresetSummaryDto>> {
        return this.call(LOREPIA_COMMANDS.deletePromptPreset, { request: input });
    }

    reorderPromptBlocks(input: ReorderPromptBlocksInput): Promise<ReorderPromptBlocksResult> {
        return this.call(LOREPIA_COMMANDS.reorderPromptBlocks, { request: input });
    }

    listTaskProfiles(): Promise<RevisionedDto<TaskProfileDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listTaskProfiles);
    }

    upsertTaskProfile(
        input: UpsertTaskProfileInput,
    ): Promise<RevisionedDto<TaskProfileDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertTaskProfile, { request: input });
    }

    deleteTaskProfile(
        input: DeleteTaskProfileInput,
    ): Promise<RevisionedDto<TaskProfileDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteTaskProfile, { request: input });
    }

    listMemoryProfiles(): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listMemoryProfiles);
    }

    getMemoryProfile(
        input: GetMemoryProfileInput,
    ): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getMemoryProfile, { request: input });
    }

    upsertMemoryProfile(
        input: UpsertMemoryProfileInput,
    ): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertMemoryProfile, { request: input });
    }

    deleteMemoryProfile(
        input: DeleteMemoryProfileInput,
    ): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteMemoryProfile, { request: input });
    }

    listKnowledgeBooks(): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listKnowledgeBooks);
    }

    getKnowledgeBook(
        input: GetKnowledgeBookInput,
    ): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getKnowledgeBook, { request: input });
    }

    upsertKnowledgeBook(
        input: UpsertKnowledgeBookInput,
    ): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertKnowledgeBook, { request: input });
    }

    deleteKnowledgeBook(
        input: DeleteKnowledgeBookInput,
    ): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteKnowledgeBook, { request: input });
    }

    listTransformSets(): Promise<RevisionedDto<CreatorTransformSetDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listTransformSets);
    }

    getTransformSet(
        input: GetTransformSetInput,
    ): Promise<RevisionedDto<CreatorTransformSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getTransformSet, { request: input });
    }

    upsertTransformSet(
        input: UpsertTransformSetInput,
    ): Promise<RevisionedDto<CreatorTransformSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertTransformSet, { request: input });
    }

    deleteTransformSet(
        input: DeleteTransformSetInput,
    ): Promise<RevisionedDto<CreatorTransformSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteTransformSet, { request: input });
    }

    listInteractionRuleSets(): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listInteractionRuleSets);
    }

    getInteractionRuleSet(
        input: GetInteractionRuleSetInput,
    ): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getInteractionRuleSet, { request: input });
    }

    upsertInteractionRuleSet(
        input: UpsertInteractionRuleSetInput,
    ): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertInteractionRuleSet, { request: input });
    }

    deleteInteractionRuleSet(
        input: DeleteInteractionRuleSetInput,
    ): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteInteractionRuleSet, { request: input });
    }

    listContentModules(): Promise<RevisionedDto<CreatorContentModuleDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listContentModules);
    }

    getContentModule(
        input: GetContentModuleInput,
    ): Promise<RevisionedDto<CreatorContentModuleDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getContentModule, { request: input });
    }

    upsertContentModule(
        input: UpsertContentModuleInput,
    ): Promise<RevisionedDto<CreatorContentModuleDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertContentModule, { request: input });
    }

    deleteContentModule(
        input: DeleteContentModuleInput,
    ): Promise<RevisionedDto<CreatorContentModuleDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteContentModule, { request: input });
    }

    deleteMemoryRecord(input: DeleteMemoryRecordRequest): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteMemoryRecord, { request: input });
    }

    getMemoryRecord(input: GetMemoryRecordInput): Promise<MemoryRecordDto> {
        return this.call(LOREPIA_COMMANDS.getMemoryRecord, { request: input });
    }

    patchMemoryRecord(input: PatchMemoryRecordRequest): Promise<MemoryRecordDto> {
        return this.call(LOREPIA_COMMANDS.patchMemoryRecord, { request: input });
    }

    setMemoryRecordExclusion(input: SetMemoryRecordExclusionRequest): Promise<MemoryRecordDto> {
        return this.call(LOREPIA_COMMANDS.setMemoryRecordExclusion, { request: input });
    }

    listPromptPresetBindings(
        input: ListPromptPresetBindingsInput,
    ): Promise<RevisionedDto<PromptPresetBindingDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listPromptPresetBindings, { request: input });
    }

    listMemoryRecords(input: ListMemoryRecordsInput): Promise<MemoryRecordListResultDto> {
        return this.call(LOREPIA_COMMANDS.listMemoryRecords, { request: input });
    }

    async simulateKnowledge(input: SimulateKnowledgeRequest): Promise<KnowledgeSimulationDto> {
        const result = await this.simulateKnowledgeActivation({
            knowledge_book_id: input.knowledge_book_id,
            sample_texts: [input.sample_text],
            manual_entry_ids: [],
            semantic_scores: [],
            variables: input.variables,
            supported_capabilities: [],
            token_estimates: [],
            activation_seed: 0,
        });
        const selectedById = new Map(result.selected.map((entry) => [entry.entry_id, entry]));
        return {
            sample_text: input.sample_text,
            entries: result.evidence.map((evidence) => {
                const selected = selectedById.get(evidence.entry_id);
                const semantic = evidence.reasons.find((reason) => reason.kind === 'semantic');
                return {
                    id: evidence.entry_id,
                    source_kind: 'knowledge' as const,
                    title: evidence.entry_id,
                    selected: evidence.selected,
                    reason:
                        evidence.exclusion_reason ??
                        (evidence.reasons.map((reason) => reason.kind).join(', ') ||
                            'not_selected'),
                    score:
                        semantic?.kind === 'semantic'
                            ? semantic.score_millionths / 1_000_000
                            : null,
                    estimated_tokens: evidence.estimated_tokens,
                    placement: selected?.placement ?? null,
                };
            }),
            total_estimated_tokens: result.used_tokens,
            truncated: result.truncated,
        };
    }

    previewTransformRule(input: PreviewTransformRuleInput): Promise<TransformRulePreviewDto> {
        return this.call(LOREPIA_COMMANDS.previewTransformRule, { request: input });
    }

    async previewTransform(input: PreviewTransformRequest): Promise<TransformPreviewDto> {
        const result = await this.previewTransformRule({
            transform_set_id: input.transform_set_id,
            transform_rule_id: input.rule_id,
            sample_text: input.sample_text,
            variables: input.variables,
            supported_capabilities: [],
            approved_import_source_ids: [],
            allow_resolved_prompt: false,
        });
        return {
            transform_set_id: input.transform_set_id,
            rule_id: input.rule_id,
            phase: result.phase,
            input: result.original,
            output: result.output,
            changed: result.changed,
            rendering: result.rendering,
            used_original:
                result.error !== null ||
                result.reports.some((report) => report.status === 'failed'),
            diagnostics: [
                ...(result.error === null ? [] : [result.error.message]),
                ...result.reports.flatMap((report) =>
                    report.trace.error === null ? [] : [report.trace.error],
                ),
            ],
            reports: result.reports,
            diff: result.diff,
            error: result.error,
            truncated: result.truncated,
        };
    }

    listInterruptedMemoryJobs(
        input: ListInterruptedMemoryJobsInput,
    ): Promise<InterruptedMemoryJobDto[]> {
        return this.call(LOREPIA_COMMANDS.listInterruptedMemoryJobs, { request: input });
    }

    retryInterruptedMemoryJob(
        input: RetryInterruptedMemoryJobInput,
    ): Promise<MemoryJobRetryReceiptDto> {
        return this.call(LOREPIA_COMMANDS.retryInterruptedMemoryJob, { request: input });
    }

    listRetryableMemoryQueryEmbeddings(
        input: ListRetryableMemoryQueryEmbeddingsInput,
    ): Promise<MemoryQueryEmbeddingRetryCandidateDto[]> {
        return this.call(LOREPIA_COMMANDS.listRetryableMemoryQueryEmbeddings, { request: input });
    }

    retryMemoryQueryEmbedding(
        input: RetryMemoryQueryEmbeddingInput,
    ): Promise<MemoryQueryEmbeddingRetryCandidateDto> {
        return this.call(LOREPIA_COMMANDS.retryMemoryQueryEmbedding, { request: input });
    }

    simulateKnowledgeActivation(
        input: SimulateKnowledgeActivationInput,
    ): Promise<KnowledgeActivationResultDto> {
        return this.call(LOREPIA_COMMANDS.simulateKnowledgeActivation, { request: input });
    }

    listContentModuleBindings(
        input: ListContentModuleBindingsInput,
    ): Promise<RevisionedDto<ModuleBindingDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listContentModuleBindings, { request: input });
    }

    listContentModuleRevisions(
        input: ListContentModuleRevisionsInput,
    ): Promise<ContentModuleRevisionListResultDto> {
        return this.call(LOREPIA_COMMANDS.listContentModuleRevisions, { request: input });
    }

    diffContentModuleRevisionDocuments(
        input: DiffContentModuleRevisionsInput,
    ): Promise<ContentModuleRevisionDiffDocumentDto> {
        return this.call(LOREPIA_COMMANDS.diffContentModuleRevisions, { request: input });
    }

    evaluateContentModuleShare(
        input: EvaluateContentModuleShareInput,
    ): Promise<ContentShareGateDto> {
        return this.call(LOREPIA_COMMANDS.evaluateContentModuleShare, { request: input });
    }
}

import type { Readable } from 'svelte/store';

import type {
    CreatorContentModuleDocumentDto,
    CreatorInteractionRuleSetDocumentDto,
    CreatorKnowledgeBookDocumentDto,
    CreatorMemoryProfileDocumentDto,
    CreatorPromptPresetDocumentDto,
    CreatorTransformSetDocumentDto,
    CreatorControlValue,
    MemoryRecordExclusionScope,
    MemoryRecordPatchInput,
    PromptPlanPreviewDto,
    ReviewedPromptSendInput,
    TaskProfileDocumentDto,
} from '../../lib/ipc/contracts';
import { ContextRoomController } from './controllers/context-room-controller';
import { CreatorDocumentController } from './controllers/creator-document-controller';
import {
    type CreatorDocumentKind,
    type CreatorDocumentValue,
    type EditablePromptBlockPatch,
    type OrchestrationCapableClient,
    type OrchestrationState,
    type RoomOrchestrationConfigPatch,
} from './controllers/orchestration-state';
import { OrchestrationStateController } from './controllers/orchestration-state-controller';
import { PlanInteractionController } from './controllers/plan-interaction-controller';
import { PromptTaskController } from './controllers/prompt-task-controller';
import { WorkspaceUseCaseController } from './controllers/workspace-use-case-controller';

export {
    INITIAL_ORCHESTRATION_STATE,
    MAX_ROOM_PROMPT_NAME_CHARS,
    MAX_ROOM_PROMPT_TEMPLATE_SLOTS,
    MAX_ROOM_PROMPT_TEXT_CHARS,
    MAX_VISIBLE_CONTENT_MODULES,
    MAX_VISIBLE_MEMORY_RECORDS,
    MAX_VISIBLE_PLAN_OPERATION_NONCE_CHARS,
    MAX_VISIBLE_PROMPT_BLOCKS,
    MAX_VISIBLE_SELECTION_EVIDENCE,
    emptyOrchestrationWorkspace,
    moveBlockByDrop,
    roomPromptSourceValidationError,
    taskProfileValidationError,
} from './controllers/orchestration-state';
export type {
    CreatorDocumentKind,
    CreatorDocumentValue,
    EditableCreatorDocumentState,
    EditablePromptBlockPatch,
    EditableTaskProfileState,
    OrchestrationCapableClient,
    OrchestrationPhase,
    OrchestrationState,
    RoomOrchestrationConfigPatch,
} from './controllers/orchestration-state';

export class OrchestrationController {
    private readonly stateController: OrchestrationStateController;
    private readonly contextRoom: ContextRoomController;
    private readonly promptTasks: PromptTaskController;
    private readonly creatorDocuments: CreatorDocumentController;
    private readonly workspaceUseCases: WorkspaceUseCaseController;
    private readonly planInteraction: PlanInteractionController;

    readonly state: Readable<OrchestrationState>;

    constructor(client: OrchestrationCapableClient) {
        this.stateController = new OrchestrationStateController();
        this.state = this.stateController.state;
        this.promptTasks = new PromptTaskController(client, this.stateController);
        this.creatorDocuments = new CreatorDocumentController(client, this.stateController);
        this.contextRoom = new ContextRoomController(
            client,
            this.stateController,
            this.promptTasks,
            this.creatorDocuments,
        );
        this.workspaceUseCases = new WorkspaceUseCaseController(
            client,
            this.stateController,
            this.contextRoom,
        );
        this.planInteraction = new PlanInteractionController(client, this.stateController);
    }

    async loadContext(conversationId: string | null, branchId: string | null): Promise<void> {
        return this.contextRoom.loadContext(conversationId, branchId);
    }

    stageRoomConfig(patch: RoomOrchestrationConfigPatch): void {
        this.contextRoom.stageRoomConfig(patch);
    }

    stageCreatorControl(controlId: string, value: CreatorControlValue): void {
        this.contextRoom.stageCreatorControl(controlId, value);
    }

    async saveRoomConfig(): Promise<boolean> {
        return this.contextRoom.saveRoomConfig();
    }

    stageEditablePromptBlock(blockId: string, patch: EditablePromptBlockPatch): boolean {
        return this.promptTasks.stageEditablePromptBlock(blockId, patch);
    }

    setEditablePromptCacheBoundary(blockId: string, enabled: boolean): boolean {
        return this.promptTasks.setEditablePromptCacheBoundary(blockId, enabled);
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
        return this.promptTasks.stageEditablePromptCacheBoundary(blockId, patch);
    }

    async reloadEditablePromptPreset(): Promise<void> {
        return this.promptTasks.reloadEditablePromptPreset();
    }

    async saveEditablePromptPreset(): Promise<boolean> {
        return this.promptTasks.saveEditablePromptPreset();
    }

    addTaskProfileDraft(taskProfileId: string): boolean {
        return this.promptTasks.addTaskProfileDraft(taskProfileId);
    }

    stageTaskProfile(taskProfileId: string, patch: Partial<TaskProfileDocumentDto>): boolean {
        return this.promptTasks.stageTaskProfile(taskProfileId, patch);
    }

    async saveTaskProfile(taskProfileId: string): Promise<boolean> {
        return this.promptTasks.saveTaskProfile(taskProfileId);
    }

    async deleteTaskProfile(taskProfileId: string): Promise<boolean> {
        return this.promptTasks.deleteTaskProfile(taskProfileId);
    }

    addCreatorDocumentDraft(kind: CreatorDocumentKind, requestedId: string): boolean {
        return this.creatorDocuments.addCreatorDocumentDraft(kind, requestedId);
    }

    replaceCreatorDocument(
        kind: CreatorDocumentKind,
        documentId: string,
        value: CreatorDocumentValue,
    ): boolean {
        return this.creatorDocuments.replaceCreatorDocument(kind, documentId, value);
    }

    stageMemoryProfile(
        documentId: string,
        patch: Partial<CreatorMemoryProfileDocumentDto>,
    ): boolean {
        return this.creatorDocuments.stageMemoryProfile(documentId, patch);
    }

    stageKnowledgeBook(
        documentId: string,
        patch: Partial<CreatorKnowledgeBookDocumentDto>,
    ): boolean {
        return this.creatorDocuments.stageKnowledgeBook(documentId, patch);
    }

    stageTransformSet(documentId: string, patch: Partial<CreatorTransformSetDocumentDto>): boolean {
        return this.creatorDocuments.stageTransformSet(documentId, patch);
    }

    stageInteractionRuleSet(
        documentId: string,
        patch: Partial<CreatorInteractionRuleSetDocumentDto>,
    ): boolean {
        return this.creatorDocuments.stageInteractionRuleSet(documentId, patch);
    }

    stageContentModule(
        documentId: string,
        patch: Partial<CreatorContentModuleDocumentDto>,
    ): boolean {
        return this.creatorDocuments.stageContentModule(documentId, patch);
    }

    async saveCreatorDocument(kind: CreatorDocumentKind, documentId: string): Promise<boolean> {
        return this.creatorDocuments.saveCreatorDocument(kind, documentId);
    }

    deleteCreatorDocument(kind: CreatorDocumentKind, documentId: string): Promise<boolean> {
        return this.creatorDocuments.deleteCreatorDocument(kind, documentId);
    }

    async movePromptBlock(blockId: string, direction: -1 | 1): Promise<boolean> {
        return this.workspaceUseCases.movePromptBlock(blockId, direction);
    }

    async movePromptBlockTo(blockId: string, targetId: string): Promise<boolean> {
        return this.workspaceUseCases.movePromptBlockTo(blockId, targetId);
    }

    async simulateKnowledge(sampleText: string): Promise<boolean> {
        return this.workspaceUseCases.simulateKnowledge(sampleText);
    }

    async previewTransform(ruleId: string, sampleText: string): Promise<boolean> {
        return this.workspaceUseCases.previewTransform(ruleId, sampleText);
    }

    async updateMemoryRecord(recordId: string, patch: MemoryRecordPatchInput): Promise<boolean> {
        return this.workspaceUseCases.updateMemoryRecord(recordId, patch);
    }

    async deleteMemoryRecord(recordId: string): Promise<boolean> {
        return this.workspaceUseCases.deleteMemoryRecord(recordId);
    }

    async setMemoryRecordPinned(recordId: string, pinned: boolean): Promise<boolean> {
        return this.workspaceUseCases.setMemoryRecordPinned(recordId, pinned);
    }

    async setMemoryRecordExclusion(
        recordId: string,
        scope: MemoryRecordExclusionScope,
        excluded: boolean,
    ): Promise<boolean> {
        return this.workspaceUseCases.setMemoryRecordExclusion(recordId, scope, excluded);
    }

    async resolvePlanPreview(userText: string): Promise<PromptPlanPreviewDto | null> {
        return this.planInteraction.resolvePlanPreview(userText);
    }

    async resolveNewPlanPreview(userText: string): Promise<PromptPlanPreviewDto | null> {
        return this.planInteraction.resolveNewPlanPreview(userText);
    }

    async resumePlanPreview(
        generationAttemptId: string,
        userText: string,
    ): Promise<PromptPlanPreviewDto | null> {
        return this.planInteraction.resumePlanPreview(generationAttemptId, userText);
    }

    clearPlanPreview(): void {
        this.planInteraction.clearPlanPreview();
    }

    completePlanOperation(): void {
        this.planInteraction.completePlanOperation();
    }

    reviewedPromptSendInput(): ReviewedPromptSendInput | null {
        return this.planInteraction.reviewedPromptSendInput();
    }

    async decideProposal(proposalId: string, approved: boolean): Promise<boolean> {
        return this.planInteraction.decideProposal(proposalId, approved);
    }

    destroy(): void {
        this.stateController.destroy();
    }
}

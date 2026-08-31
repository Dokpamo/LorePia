import type {
    CapabilityKeyInput,
    CapabilityObservationDto,
    CreateProviderConnectionInput,
    CredentialStatusDto,
    CredentialTargetDto,
    GenerationPresetDto,
    GenerationPresetInput,
    GenerationTargetDto,
    ModelRouteDto,
    ModelSyncEventDto,
    ModelSyncJobDto,
    ModelSyncStartedDto,
    NativeCaptureStatusDto,
    PromptCacheControlDto,
    ProviderConnectionDto,
    ProviderProfileDto,
    ProviderTemplateDto,
    ProviderOverviewDto,
    ReasoningControlDto,
    RequestPreviewDto,
    AppSettingsDto,
    EffectiveCapabilityDto,
    ParameterSpecDto,
    UpdateProviderConnectionInput,
    UpsertCapabilityOverrideInput,
    UpsertModelRouteInput,
} from '../contracts';

import { LOREPIA_COMMANDS } from '../commands';

import { ConversationClient } from './conversation';

export abstract class ProviderClient extends ConversationClient {
    getProviderOverview(): Promise<ProviderOverviewDto> {
        return this.call(LOREPIA_COMMANDS.getProviderOverview);
    }

    getSettings(): Promise<AppSettingsDto> {
        return this.call(LOREPIA_COMMANDS.getSettings);
    }

    updateSettings(settings: AppSettingsDto): Promise<AppSettingsDto> {
        return this.call(LOREPIA_COMMANDS.updateSettings, { request: { settings } });
    }

    selectGenerationTarget(target: GenerationTargetDto | null): Promise<AppSettingsDto> {
        return this.call(LOREPIA_COMMANDS.selectGenerationTarget, { request: { target } });
    }

    listProviderTemplates(): Promise<ProviderTemplateDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderTemplates);
    }

    listProviderConnections(): Promise<ProviderConnectionDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderConnections);
    }

    createProviderConnection(input: CreateProviderConnectionInput): Promise<ProviderConnectionDto> {
        return this.call(LOREPIA_COMMANDS.createProviderConnection, {
            request: { input },
        });
    }

    upsertProviderConnection(input: UpdateProviderConnectionInput): Promise<ProviderConnectionDto> {
        return this.call(LOREPIA_COMMANDS.upsertProviderConnection, {
            request: { input },
        });
    }

    deleteProviderConnection(connectionId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteProviderConnection, {
            request: { connection_id: connectionId },
        });
    }

    listProviderProfiles(): Promise<ProviderProfileDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderProfiles);
    }

    listModelRoutes(connectionId: string): Promise<ModelRouteDto[]> {
        return this.call(LOREPIA_COMMANDS.listModelRoutes, {
            request: { connection_id: connectionId },
        });
    }

    upsertModelRoute(input: UpsertModelRouteInput): Promise<ModelRouteDto> {
        return this.call(LOREPIA_COMMANDS.upsertModelRoute, { request: { input } });
    }

    deleteModelRoute(routeId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteModelRoute, {
            request: { model_route_id: routeId },
        });
    }

    listCapabilityObservations(modelRouteId: string): Promise<CapabilityObservationDto[]> {
        return this.call(LOREPIA_COMMANDS.listCapabilityObservations, {
            request: { model_route_id: modelRouteId },
        });
    }

    effectiveCapability(
        modelRouteId: string,
        key: CapabilityKeyInput,
    ): Promise<EffectiveCapabilityDto | null> {
        return this.call(LOREPIA_COMMANDS.effectiveCapability, {
            request: { model_route_id: modelRouteId, key },
        });
    }

    effectiveParameterSpecs(modelRouteId: string): Promise<ParameterSpecDto[]> {
        return this.call(LOREPIA_COMMANDS.effectiveParameterSpecs, {
            request: { model_route_id: modelRouteId },
        });
    }

    upsertUserCapabilityOverride(
        input: UpsertCapabilityOverrideInput,
    ): Promise<CapabilityObservationDto> {
        return this.call(LOREPIA_COMMANDS.upsertUserCapabilityOverride, {
            request: { input },
        });
    }

    deleteUserCapabilityOverride(modelRouteId: string, observationId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteUserCapabilityOverride, {
            request: {
                model_route_id: modelRouteId,
                observation_id: observationId,
            },
        });
    }

    listGenerationPresets(routeId: string): Promise<GenerationPresetDto[]> {
        return this.call(LOREPIA_COMMANDS.listGenerationPresets, {
            request: { model_route_id: routeId },
        });
    }

    upsertGenerationPreset(input: GenerationPresetInput): Promise<GenerationPresetDto> {
        return this.call(LOREPIA_COMMANDS.upsertGenerationPreset, {
            request: { input },
        });
    }

    deleteGenerationPreset(presetId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteGenerationPreset, {
            request: { generation_preset_id: presetId },
        });
    }

    validateGenerationPresetCandidate(input: GenerationPresetInput): Promise<void> {
        return this.call(LOREPIA_COMMANDS.validateGenerationPresetCandidate, {
            request: { input },
        });
    }

    renderReasoningControlForPreset(input: GenerationPresetInput): Promise<ReasoningControlDto> {
        return this.call(LOREPIA_COMMANDS.renderReasoningControlForPreset, {
            request: { input },
        });
    }

    renderPromptCacheControlForPreset(
        input: GenerationPresetInput,
    ): Promise<PromptCacheControlDto> {
        return this.call(LOREPIA_COMMANDS.renderPromptCacheControlForPreset, {
            request: { input },
        });
    }

    previewProviderRequestCandidate(input: GenerationPresetInput): Promise<RequestPreviewDto> {
        return this.call(LOREPIA_COMMANDS.previewProviderRequestCandidate, {
            request: { input },
        });
    }

    credentialStatus(target: CredentialTargetDto): Promise<CredentialStatusDto> {
        return this.call(LOREPIA_COMMANDS.credentialStatus, { request: { target } });
    }

    captureCredential(target: CredentialTargetDto): Promise<NativeCaptureStatusDto> {
        return this.call(LOREPIA_COMMANDS.captureCredential, { request: { target } });
    }

    deleteCredential(target: CredentialTargetDto): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteCredential, { request: { target } });
    }

    previewProviderRequest(target: GenerationTargetDto): Promise<RequestPreviewDto> {
        return this.call(LOREPIA_COMMANDS.previewProviderRequest, {
            request: { target },
        });
    }

    startProviderModelSync(connectionId: string): Promise<ModelSyncStartedDto> {
        return this.call(LOREPIA_COMMANDS.startProviderModelSync, {
            request: { connection_id: connectionId },
        });
    }

    getProviderModelSync(jobId: string): Promise<ModelSyncJobDto> {
        return this.call(LOREPIA_COMMANDS.getProviderModelSync, {
            request: { job_id: jobId },
        });
    }

    listProviderModelSyncs(connectionId: string, limit: number): Promise<ModelSyncJobDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderModelSyncs, {
            request: { connection_id: connectionId, limit },
        });
    }

    approveProviderModelSync(jobId: string, reviewSha256: string): Promise<ModelSyncJobDto> {
        return this.call(LOREPIA_COMMANDS.approveProviderModelSync, {
            request: { job_id: jobId, review_sha256: reviewSha256 },
        });
    }

    cancelProviderModelSync(jobId: string): Promise<ModelSyncJobDto> {
        return this.call(LOREPIA_COMMANDS.cancelProviderModelSync, {
            request: { job_id: jobId },
        });
    }

    pollProviderModelSyncEvents(jobId: string, limit: number): Promise<ModelSyncEventDto[]> {
        return this.call(LOREPIA_COMMANDS.pollProviderModelSyncEvents, {
            request: { job_id: jobId, limit },
        });
    }

    ackProviderModelSyncEvent(jobId: string, sequence: number): Promise<boolean> {
        return this.call(LOREPIA_COMMANDS.ackProviderModelSyncEvent, {
            request: { job_id: jobId, sequence },
        });
    }
}

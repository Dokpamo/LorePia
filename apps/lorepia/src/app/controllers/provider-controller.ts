import type {
    AppSettingsDto,
    CapabilityKeyInput,
    CreateProviderConnectionInput,
    CredentialTargetDto,
    GenerationPresetInput,
    ModelSyncJobDto,
    NativeCaptureStatusDto,
    ProviderCatalogRollbackPlanDto,
    ProviderWorkspaceDto,
    UpdateProviderConnectionInput,
    UpsertCapabilityOverrideInput,
    UpsertModelRouteInput,
} from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { EpochGuard } from '../operations/epoch-guard';
import { SerializedMutation } from '../operations/serialized-mutation';
import { credentialKey } from '../provider-credential';
import type { AppControllerContext } from './controller-context';
import { ProviderWorkspaceLoader, type ProviderDiagnosticsKind } from './provider-workspace-loader';

interface ProviderControllerHooks {
    captureAnnouncement(status: NativeCaptureStatusDto, success: string): string;
    loadProviders(): Promise<void>;
    loadProviderCapabilities(modelRouteId: string): Promise<void>;
    refreshProviderModelSync(jobId: string): Promise<void>;
}

export class ProviderController {
    private readonly providerSettingsEpoch = new EpochGuard();
    private readonly providerSettingsMutations = new SerializedMutation();

    private readonly loader: ProviderWorkspaceLoader;

    constructor(
        private readonly context: AppControllerContext,
        private readonly hooks: ProviderControllerHooks,
    ) {
        this.loader = new ProviderWorkspaceLoader(context, this.providerSettingsEpoch);
    }

    loadProviders(): Promise<void> {
        return this.loader.load();
    }

    loadProviderDiagnostics(kind: ProviderDiagnosticsKind): Promise<string | null> {
        return this.loader.loadDiagnostics(kind);
    }

    async captureProviderCredential(target: CredentialTargetDto): Promise<boolean> {
        if (this.isRetainedLegacyConnectionCredentialTarget(target)) return false;
        this.loader.invalidateCredential(target);
        try {
            const capture = await this.context.client.captureCredential(target);
            const status = await this.context.client.credentialStatus(target);
            this.loader.invalidateCredential(target);
            this.context.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    workspace: {
                        ...state.providers.workspace,
                        credential_statuses: {
                            ...state.providers.workspace.credential_statuses,
                            [credentialKey(target)]: status.status,
                        },
                    },
                },
            }));
            this.context.announce(
                this.hooks.captureAnnouncement(capture, t('provider.notice.credential_stored')),
            );
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async deleteProviderCredential(target: CredentialTargetDto): Promise<void> {
        if (this.isRetainedLegacyConnectionCredentialTarget(target)) return;
        this.loader.invalidateCredential(target);
        try {
            await this.context.client.deleteCredential(target);
            this.loader.invalidateCredential(target);
            this.context.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    workspace: {
                        ...state.providers.workspace,
                        credential_statuses: {
                            ...state.providers.workspace.credential_statuses,
                            [credentialKey(target)]: 'missing',
                        },
                    },
                },
            }));
            this.context.announce(t('provider.notice.credential_deleted'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    private isRetainedLegacyConnectionCredentialTarget(target: CredentialTargetDto): boolean {
        return (
            target.kind === 'connection' &&
            this.context
                .readState()
                .providers.workspace.legacy_profiles.some(
                    (profile) => profile.id === target.connection_id,
                )
        );
    }

    async createProviderConnection(input: CreateProviderConnectionInput): Promise<boolean> {
        try {
            await this.context.client.createProviderConnection(input);
            await this.loader.load(false);
            this.context.announce(t('provider.notice.connection_created'));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async updateProviderConnection(input: UpdateProviderConnectionInput): Promise<boolean> {
        try {
            await this.context.client.upsertProviderConnection(input);
            await this.loader.load(false);
            this.context.announce(t('provider.notice.connection_updated'));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async deleteProviderConnection(connectionId: string): Promise<boolean> {
        try {
            await this.context.client.deleteProviderConnection(connectionId);
            await this.loader.load(false);
            this.context.announce(t('provider.notice.connection_deleted'));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async upsertProviderModelRoute(input: UpsertModelRouteInput): Promise<boolean> {
        try {
            await this.context.client.upsertModelRoute(input);
            await this.loader.load(false);
            this.context.announce(t('provider.notice.route_saved'));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async deleteProviderModelRoute(modelRouteId: string): Promise<boolean> {
        try {
            await this.context.client.deleteModelRoute(modelRouteId);
            await this.loader.load(false);
            this.context.announce(t('provider.notice.route_deleted'));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async upsertProviderGenerationPreset(input: GenerationPresetInput): Promise<boolean> {
        try {
            await this.context.client.upsertGenerationPreset(input);
            await this.loader.load(false);
            this.context.announce(t('provider.notice.preset_saved'));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async deleteProviderGenerationPreset(generationPresetId: string): Promise<boolean> {
        try {
            await this.context.client.deleteGenerationPreset(generationPresetId);
            await this.loader.load(false);
            this.context.announce(t('provider.notice.preset_deleted'));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async validateProviderGenerationPresetCandidate(
        input: GenerationPresetInput,
    ): Promise<boolean> {
        try {
            await this.context.client.validateGenerationPresetCandidate(input);
            this.context.announce(t('provider.notice.preset_valid'));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async previewProviderRequestCandidate(input: GenerationPresetInput): Promise<void> {
        try {
            const preview = await this.context.client.previewProviderRequestCandidate(input);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                request_preview: preview,
            }));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async previewSelectedProviderRequest(): Promise<boolean> {
        const settings = this.context.readState().providers.workspace.settings;
        if (
            settings.selected_model_route_id === null ||
            settings.selected_generation_preset_id === null
        ) {
            this.context.announce(t('provider.notice.no_default_route'));
            return false;
        }
        try {
            const preview = await this.context.client.previewProviderRequest({
                model_route_id: settings.selected_model_route_id,
                generation_preset_id: settings.selected_generation_preset_id,
            });
            this.context.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    workspace: { ...state.providers.workspace, request_preview: preview },
                },
            }));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    private updateProviderWorkspace(
        updater: (workspace: ProviderWorkspaceDto) => ProviderWorkspaceDto,
    ): void {
        this.context.update((state) => ({
            ...state,
            providers: {
                ...state.providers,
                workspace: updater(state.providers.workspace),
            },
        }));
    }

    private storeProviderSettings(settings: AppSettingsDto): void {
        this.providerSettingsEpoch.advance();
        this.updateProviderWorkspace((workspace) => ({ ...workspace, settings }));
    }

    private enqueueProviderSettingsMutation<T>(mutation: () => Promise<T>): Promise<T> {
        return this.providerSettingsMutations.enqueue(mutation);
    }

    private storeModelSyncJob(job: ModelSyncJobDto): void {
        this.updateProviderWorkspace((workspace) => ({
            ...workspace,
            model_sync_jobs: [
                job,
                ...workspace.model_sync_jobs.filter((candidate) => candidate.id !== job.id),
            ],
            selected_model_sync_job_id: job.id,
        }));
    }

    async loadProviderCapabilities(modelRouteId: string): Promise<void> {
        if (modelRouteId === '') return;
        try {
            const [observations, parameterSpecs] = await Promise.all([
                this.context.client.listCapabilityObservations(modelRouteId),
                this.context.client.effectiveParameterSpecs(modelRouteId),
            ]);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                selected_capability_model_route_id: modelRouteId,
                capability_observations: observations,
                capability_parameter_specs: parameterSpecs,
                effective_capability: null,
            }));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async inspectEffectiveProviderCapability(key: CapabilityKeyInput): Promise<void> {
        const routeId =
            this.context.readState().providers.workspace.selected_capability_model_route_id;
        if (routeId === null) return;
        try {
            const capability = await this.context.client.effectiveCapability(routeId, key);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                effective_capability: capability,
            }));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async upsertProviderCapabilityOverride(input: UpsertCapabilityOverrideInput): Promise<boolean> {
        try {
            await this.context.client.upsertUserCapabilityOverride(input);
            await this.hooks.loadProviderCapabilities(input.model_route_id);
            this.context.announce(t('provider.notice.override_saved'));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async deleteProviderCapabilityOverride(observationId: string): Promise<void> {
        const routeId =
            this.context.readState().providers.workspace.selected_capability_model_route_id;
        if (routeId === null) return;
        try {
            await this.context.client.deleteUserCapabilityOverride(routeId, observationId);
            await this.hooks.loadProviderCapabilities(routeId);
            this.context.announce(t('provider.notice.override_deleted'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async selectProviderGenerationTarget(
        modelRouteId: string | null,
        generationPresetId: string | null,
    ): Promise<boolean> {
        if ((modelRouteId === null) !== (generationPresetId === null)) return false;
        return this.enqueueProviderSettingsMutation(async () => {
            try {
                const settings = await this.context.client.selectGenerationTarget(
                    modelRouteId === null || generationPresetId === null
                        ? null
                        : {
                              model_route_id: modelRouteId,
                              generation_preset_id: generationPresetId,
                          },
                );
                this.storeProviderSettings(settings);
                this.context.announce(
                    modelRouteId === null
                        ? t('provider.notice.target_cleared')
                        : t('provider.notice.target_saved'),
                );
                return true;
            } catch (error: unknown) {
                this.context.announce(this.context.errorLabel(error));
                return false;
            }
        });
    }

    async selectLegacyProviderProfile(profileId: string): Promise<boolean> {
        return this.enqueueProviderSettingsMutation(async () => {
            const workspace = this.context.readState().providers.workspace;
            if (!workspace.legacy_profiles.some((profile) => profile.id === profileId))
                return false;
            try {
                const settings = await this.context.client.updateSettings({
                    ...workspace.settings,
                    selected_provider_profile_id: profileId,
                    selected_model_route_id: null,
                    selected_generation_preset_id: null,
                });
                this.storeProviderSettings(settings);
                this.context.announce(t('provider.notice.existing_target_saved'));
                return true;
            } catch (error: unknown) {
                this.context.announce(this.context.errorLabel(error));
                return false;
            }
        });
    }

    async setPreservePartialGenerations(preserve: boolean): Promise<boolean> {
        return this.enqueueProviderSettingsMutation(async () => {
            const current = this.context.readState().providers.workspace.settings;
            try {
                const settings = await this.context.client.updateSettings({
                    ...current,
                    preserve_partial_generations: preserve,
                });
                this.storeProviderSettings(settings);
                this.context.announce(t('provider.notice.partial_saved'));
                return true;
            } catch (error: unknown) {
                this.context.announce(this.context.errorLabel(error));
                return false;
            }
        });
    }

    async startProviderModelSync(connectionId: string): Promise<void> {
        try {
            const started = await this.context.client.startProviderModelSync(connectionId);
            await this.hooks.refreshProviderModelSync(started.job_id);
            this.context.announce(t('provider.notice.sync_started'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async refreshProviderModelSync(jobId: string): Promise<void> {
        try {
            const [job, events] = await Promise.all([
                this.context.client.getProviderModelSync(jobId),
                this.context.client.pollProviderModelSyncEvents(jobId, 100),
            ]);
            const latestEvent = events.at(-1) ?? null;
            for (const event of events) {
                await this.context.client.ackProviderModelSyncEvent(jobId, event.sequence);
            }
            this.storeModelSyncJob(job);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                model_sync_event: latestEvent ?? workspace.model_sync_event,
            }));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async approveProviderModelSync(jobId: string): Promise<void> {
        const job = this.context
            .readState()
            .providers.workspace.model_sync_jobs.find((candidate) => candidate.id === jobId);
        if (job?.review === null || job?.review === undefined) return;
        try {
            this.storeModelSyncJob(
                await this.context.client.approveProviderModelSync(jobId, job.review.sha256),
            );
            await this.loader.load(false);
            this.context.announce(t('provider.notice.sync_applied'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async cancelProviderModelSync(jobId: string): Promise<void> {
        try {
            this.storeModelSyncJob(await this.context.client.cancelProviderModelSync(jobId));
            this.context.announce(t('provider.notice.sync_cancelled'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async pickProviderCatalogImport(): Promise<void> {
        try {
            const ticket = await this.context.client.pickProviderCatalogImport();
            if (ticket === null) return;
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                pending_catalog_import: ticket,
            }));
            this.context.announce(t('provider.notice.catalog_plan'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async activateProviderCatalogImport(): Promise<void> {
        const ticket = this.context.readState().providers.workspace.pending_catalog_import;
        if (ticket === null) return;
        this.loader.invalidateDiagnostics('catalog');
        try {
            const result = await this.context.client.activateProviderCatalogImport(
                ticket.ticket_id,
            );
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                catalog_status: result.status,
                pending_catalog_import: null,
                catalog_diff: result.diff,
            }));
            await this.loader.load(false);
            const refreshError = await this.loader.loadDiagnostics('catalog');
            this.context.announce(
                [t('provider.notice.catalog_applied'), refreshError].filter(Boolean).join(' '),
            );
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async discardProviderCatalogImport(): Promise<void> {
        const ticket = this.context.readState().providers.workspace.pending_catalog_import;
        if (ticket === null) return;
        try {
            await this.context.client.discardProviderCatalogImport(ticket.ticket_id);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                pending_catalog_import: null,
            }));
            this.context.announce(t('provider.notice.catalog_discarded'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async diffProviderCatalogRevisions(fromRevision: number, toRevision: number): Promise<void> {
        try {
            const catalogDiff = await this.context.client.diffProviderCatalogRevisions(
                fromRevision,
                toRevision,
            );
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                catalog_diff: catalogDiff,
            }));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async prepareProviderCatalogRollback(targetRevision: number): Promise<void> {
        try {
            const plan = await this.context.client.prepareProviderCatalogRollback(targetRevision);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                pending_catalog_rollback: plan,
                catalog_diff: plan.catalog_plan.diff,
            }));
            this.context.announce(t('provider.notice.rollback_plan'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async activateProviderCatalogRollback(plan?: ProviderCatalogRollbackPlanDto): Promise<void> {
        const exactPlan =
            plan ?? this.context.readState().providers.workspace.pending_catalog_rollback;
        if (exactPlan === null) return;
        this.loader.invalidateDiagnostics('catalog');
        try {
            const result = await this.context.client.activateProviderCatalogRollback(exactPlan);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                catalog_status: result.status,
                pending_catalog_rollback: null,
                catalog_diff: exactPlan.catalog_plan.diff,
            }));
            await this.loader.load(false);
            const refreshError = await this.loader.loadDiagnostics('catalog');
            this.context.announce(
                [t('provider.notice.rolled_back'), refreshError].filter(Boolean).join(' '),
            );
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    destroy(): void {
        this.loader.destroy();
    }
}

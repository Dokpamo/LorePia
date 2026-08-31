import type {
    BeginProviderDiscoveryCurlInput,
    BeginProviderDiscoveryInput,
    ContinueProviderDiscoveryActionInput,
    DiscoveryAssistantFailureKindInput,
    DiscoveryAssistantInterruptionOutcomeInput,
    NativeCaptureStatusDto,
    ProviderDiscoverySessionDto,
    ProviderWorkspaceDto,
} from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { EpochGuard } from '../operations/epoch-guard';
import {
    drainProviderDiscoveryEvents,
    loadProviderDiscoverySnapshot,
    mergeProviderDiscoverySnapshot,
    storeProviderDiscoverySession,
} from '../provider-discovery-flow';
import type { AppControllerContext } from './controller-context';

interface DiscoveryControllerHooks {
    captureAnnouncement(status: NativeCaptureStatusDto, success: string): string;
    updateProviderWorkspace(
        updater: (workspace: ProviderWorkspaceDto) => ProviderWorkspaceDto,
    ): void;
    loadProviders(): Promise<void>;
    refreshProviderDiscovery(sessionId: string): Promise<void>;
    pollSelectedProviderDiscoveryEvents(): Promise<void>;
}

export class DiscoveryController {
    private readonly discoveryRequestEpoch = new EpochGuard();

    constructor(
        private readonly context: AppControllerContext,
        private readonly hooks: DiscoveryControllerHooks,
    ) {}

    private storeDiscoverySession(session: ProviderDiscoverySessionDto): void {
        this.hooks.updateProviderWorkspace((workspace) =>
            storeProviderDiscoverySession(workspace, session),
        );
    }

    async beginProviderDiscovery(
        request:
            | { kind: 'site'; input: BeginProviderDiscoveryInput }
            | { kind: 'curl'; input: BeginProviderDiscoveryCurlInput },
    ): Promise<boolean> {
        try {
            let session: ProviderDiscoverySessionDto;
            let capture: NativeCaptureStatusDto | null = null;
            if (request.kind === 'site') {
                session = await this.context.client.beginProviderDiscovery(request.input);
            } else {
                const captured = await this.context.client.beginProviderDiscoveryCurl(
                    request.input,
                );
                session = captured.session;
                capture = captured.capture;
            }
            this.storeDiscoverySession(session);
            await this.hooks.refreshProviderDiscovery(session.id);
            await this.hooks.pollSelectedProviderDiscoveryEvents();
            this.context.announce(
                capture === null
                    ? t('provider.notice.discovery_started')
                    : this.hooks.captureAnnouncement(
                          capture,
                          t('provider.notice.discovery_started_curl'),
                      ),
            );
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async refreshProviderDiscovery(sessionId: string): Promise<void> {
        const requestEpoch = this.discoveryRequestEpoch.advance();
        this.hooks.updateProviderWorkspace((workspace) => ({
            ...workspace,
            selected_discovery_id: sessionId,
        }));
        await this.refreshProviderDiscoveryAtEpoch(sessionId, requestEpoch);
    }

    private isCurrentDiscoveryRequest(sessionId: string, requestEpoch: number): boolean {
        return (
            this.discoveryRequestEpoch.isCurrent(requestEpoch) &&
            this.context.readState().providers.workspace.selected_discovery_id === sessionId
        );
    }

    private async refreshProviderDiscoveryAtEpoch(
        sessionId: string,
        requestEpoch: number,
    ): Promise<void> {
        try {
            const snapshot = await loadProviderDiscoverySnapshot(
                this.context.client,
                sessionId,
                () => this.isCurrentDiscoveryRequest(sessionId, requestEpoch),
            );
            if (snapshot === null || !this.isCurrentDiscoveryRequest(sessionId, requestEpoch))
                return;
            this.hooks.updateProviderWorkspace((workspace) =>
                mergeProviderDiscoverySnapshot(workspace, snapshot),
            );
        } catch (error: unknown) {
            if (!this.isCurrentDiscoveryRequest(sessionId, requestEpoch)) return;
            this.context.announce(this.context.errorLabel(error));
        }
    }

    private selectedProviderDiscoveryId(): string | null {
        return this.context.readState().providers.workspace.selected_discovery_id;
    }

    async runProviderDiscoveryAssistant(): Promise<void> {
        const sessionId = this.selectedProviderDiscoveryId();
        if (sessionId === null) return;
        try {
            const hostAction =
                await this.context.client.runProviderDiscoveryAssistantTurn(sessionId);
            this.hooks.updateProviderWorkspace((workspace) => ({
                ...workspace,
                discovery_assistant_host_action: hostAction,
            }));
            await this.hooks.refreshProviderDiscovery(sessionId);
            this.context.announce(t('provider.notice.assistant_ready'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async resumeProviderDiscoveryAssistantCoreHostAction(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.context.client.resumeProviderDiscoveryAssistantCoreHostAction(sessionId),
        );
    }

    async approveProviderDiscoveryAssistantRetry(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.context.client.approveProviderDiscoveryAssistantRetry(sessionId),
        );
    }

    async requestProviderDiscoveryAssistantRevision(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.context.client.requestProviderDiscoveryAssistantRevision(sessionId),
        );
    }

    async acceptProviderDiscoveryAssistantDraft(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.context.client.acceptProviderDiscoveryAssistantDraft(sessionId),
        );
    }

    async recordProviderDiscoveryAssistantFailure(
        kind: DiscoveryAssistantFailureKindInput,
        retryable: boolean,
    ): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.context.client.recordProviderDiscoveryAssistantFailure(sessionId, kind, retryable),
        );
    }

    async interruptProviderDiscoveryAssistant(
        outcome: DiscoveryAssistantInterruptionOutcomeInput,
    ): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.context.client.interruptProviderDiscoveryAssistant(sessionId, outcome),
        );
    }

    async restartProviderDiscoveryAssistantAfterInterruption(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.context.client.restartProviderDiscoveryAssistantAfterInterruption(sessionId),
        );
    }

    private async mutateSelectedDiscoveryAssistant(
        action: (sessionId: string) => Promise<ProviderDiscoverySessionDto>,
    ): Promise<void> {
        const sessionId = this.selectedProviderDiscoveryId();
        if (sessionId === null) return;
        try {
            this.storeDiscoverySession(await action(sessionId));
            await this.hooks.refreshProviderDiscovery(sessionId);
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async pollSelectedProviderDiscoveryEvents(): Promise<void> {
        const selectedId = this.context.readState().providers.workspace.selected_discovery_id;
        if (selectedId === null) return;
        const requestEpoch = this.discoveryRequestEpoch.advance();
        try {
            const result = await drainProviderDiscoveryEvents(this.context.client, selectedId, () =>
                this.isCurrentDiscoveryRequest(selectedId, requestEpoch),
            );
            if (result === null || !this.isCurrentDiscoveryRequest(selectedId, requestEpoch))
                return;
            if (result.latest !== null) {
                this.hooks.updateProviderWorkspace((workspace) => ({
                    ...workspace,
                    discovery_event: result.latest,
                }));
            }
            if (!this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) return;
            await this.refreshProviderDiscoveryAtEpoch(selectedId, requestEpoch);
            if (!result.drained && this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) {
                this.context.announce(t('provider.notice.events_truncated'));
            }
        } catch (error: unknown) {
            if (!this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) return;
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async continueProviderDiscovery(
        action: ContinueProviderDiscoveryActionInput,
    ): Promise<boolean> {
        const workspace = this.context.readState().providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (!session?.action_required) {
            this.context.announce(t('provider.notice.no_next_step'));
            return false;
        }
        const actionId = globalThis.crypto.randomUUID();
        try {
            const next = await this.context.client.continueProviderDiscovery({
                session_id: session.id,
                action_id: actionId,
                expected_revision: session.revision,
                action,
            });
            this.storeDiscoverySession(next);
            await this.hooks.refreshProviderDiscovery(next.id);
            await this.hooks.pollSelectedProviderDiscoveryEvents();
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async supplyProviderDiscoveryDocumentEvidence(documentUrl: string): Promise<boolean> {
        const workspace = this.context.readState().providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (session === undefined || documentUrl.trim() === '') return false;
        try {
            this.storeDiscoverySession(
                await this.context.client.supplyProviderDiscoveryDocumentEvidence(
                    session.id,
                    session.revision,
                    documentUrl.trim(),
                ),
            );
            await this.hooks.refreshProviderDiscovery(session.id);
            await this.hooks.pollSelectedProviderDiscoveryEvents();
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async supplyProviderDiscoveryCurlEvidence(): Promise<boolean> {
        const workspace = this.context.readState().providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (session === undefined) return false;
        try {
            const captured = await this.context.client.supplyProviderDiscoveryCurlEvidence(
                session.id,
                session.revision,
            );
            this.storeDiscoverySession(captured.session);
            await this.hooks.refreshProviderDiscovery(session.id);
            await this.hooks.pollSelectedProviderDiscoveryEvents();
            this.context.announce(
                this.hooks.captureAnnouncement(captured.capture, t('provider.notice.curl_added')),
            );
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async cancelProviderDiscovery(): Promise<void> {
        const workspace = this.context.readState().providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (session === undefined) return;
        try {
            this.storeDiscoverySession(
                await this.context.client.cancelProviderDiscovery(session.id, session.revision),
            );
            await this.hooks.refreshProviderDiscovery(session.id);
            this.context.announce(t('provider.notice.discovery_cancelled'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async commitProviderDiscovery(): Promise<boolean> {
        const sessionId = this.context.readState().providers.workspace.selected_discovery_id;
        if (sessionId === null) return false;
        try {
            await this.context.client.commitProviderDiscovery(sessionId);
            await this.hooks.loadProviders();
            this.context.announce(t('provider.notice.connection_saved'));
            return true;
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
            return false;
        }
    }

    async recoverProviderDiscoveries(): Promise<void> {
        try {
            const results = await this.context.client.recoverProviderDiscovery();
            this.hooks.updateProviderWorkspace((workspace) => ({
                ...workspace,
                discovery_recovery_results: results,
            }));
            await this.hooks.loadProviders();
            this.context.announce(
                results.length === 0
                    ? t('provider.notice.no_recovery')
                    : t('provider.notice.recovered', { count: results.length }),
            );
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async continueProviderDiscoveryCompensation(resume: boolean): Promise<void> {
        const sessionId = this.context.readState().providers.workspace.selected_discovery_id;
        if (sessionId === null) return;
        try {
            const session = resume
                ? await this.context.client.resumeProviderDiscoveryCompensation(sessionId)
                : await this.context.client.continueProviderDiscoveryCompensation(sessionId);
            this.storeDiscoverySession(session);
            await this.hooks.refreshProviderDiscovery(sessionId);
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    destroy(): void {
        this.discoveryRequestEpoch.advance();
    }
}

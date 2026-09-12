import type { CredentialTargetDto, ProviderWorkspaceDto } from '../../lib/ipc/contracts';
import { EpochGuard } from '../operations/epoch-guard';
import { credentialKey, discoveryCredentialTarget } from '../provider-credential';
import type { AppControllerContext } from './controller-context';

export type ProviderDiagnosticsKind = 'catalog' | 'discovery' | 'sync';

/** Snapshot loading and optional diagnostics have independent freshness/error authority. */
export class ProviderWorkspaceLoader {
    private readonly epoch = new EpochGuard();
    private readonly diagnostics = new Map<ProviderDiagnosticsKind, EpochGuard>();
    private readonly credentialEpochs = new Map<string, number>();

    constructor(
        private readonly context: AppControllerContext,
        private readonly settingsEpoch: EpochGuard,
    ) {}

    invalidateCredential(target: CredentialTargetDto): void {
        const key = credentialKey(target);
        this.credentialEpochs.set(key, (this.credentialEpochs.get(key) ?? 0) + 1);
        this.patch({
            credential_statuses: {
                ...this.context.readState().providers.workspace.credential_statuses,
                [key]: 'unreadable',
            },
        });
    }

    async load(refreshCredentials = true): Promise<void> {
        const epoch = this.epoch.advance();
        const settingsEpoch = this.settingsEpoch.current();
        this.context.update((state) => ({
            ...state,
            providers: {
                ...state.providers,
                phase: state.providers.phase === 'ready' ? 'ready' : 'loading',
                error: null,
            },
        }));
        try {
            const overview = await this.context.client.getProviderOverview();
            if (!Array.isArray(overview.routes) || !Array.isArray(overview.presets)) {
                throw new Error('Provider overview is missing its generation catalog');
            }
            if (!this.epoch.isCurrent(epoch)) return;
            const previous = this.context.readState().providers.workspace;
            this.context.update((state) => ({
                ...state,
                providers: {
                    phase: 'ready',
                    error: null,
                    workspace: {
                        ...state.providers.workspace,
                        ...overview,
                        settings: this.settingsEpoch.isCurrent(settingsEpoch)
                            ? overview.settings
                            : state.providers.workspace.settings,
                    },
                },
            }));
            const legacyIds = new Set(overview.legacy_profiles.map((profile) => profile.id));
            const previousConnections = new Map(
                (refreshCredentials ? [] : previous.connections).map((connection) => [
                    connection.id,
                    JSON.stringify(connection),
                ]),
            );
            const previousProfiles = new Map(
                (refreshCredentials ? [] : previous.legacy_profiles).map((profile) => [
                    profile.id,
                    JSON.stringify(profile),
                ]),
            );
            const targets: CredentialTargetDto[] = [
                ...overview.connections
                    .filter(
                        (connection) =>
                            connection.credential_binding_required && !legacyIds.has(connection.id),
                    )
                    .filter(
                        (connection) =>
                            refreshCredentials ||
                            previousConnections.get(connection.id) !== JSON.stringify(connection),
                    )
                    .map((connection): CredentialTargetDto => ({
                        kind: 'connection',
                        connection_id: connection.id,
                    })),
                ...overview.legacy_profiles
                    .filter(
                        (profile) =>
                            refreshCredentials ||
                            previousProfiles.get(profile.id) !== JSON.stringify(profile),
                    )
                    .map((profile): CredentialTargetDto => ({
                        kind: 'legacy_profile',
                        provider_profile_id: profile.id,
                    })),
            ];
            for (const target of targets) this.invalidateCredential(target);
            await this.refreshCredentials(targets, () => this.epoch.isCurrent(epoch));
        } catch (error: unknown) {
            if (!this.epoch.isCurrent(epoch)) return;
            this.context.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    phase: state.providers.phase === 'ready' ? 'ready' : 'error',
                    error: this.context.errorLabel(error),
                },
            }));
        }
    }

    invalidateDiagnostics(kind: ProviderDiagnosticsKind): void {
        this.diagnostics.get(kind)?.advance();
    }

    async loadDiagnostics(kind: ProviderDiagnosticsKind): Promise<string | null> {
        const guard = this.diagnostics.get(kind) ?? new EpochGuard();
        this.diagnostics.set(kind, guard);
        const epoch = guard.advance();
        try {
            let patch: Partial<ProviderWorkspaceDto>;
            if (kind === 'catalog') {
                const [catalog_status, catalog_history] = await Promise.all([
                    this.context.client.providerCatalogStatus(),
                    this.context.client.providerCatalogHistory(50, null, null),
                ]);
                patch = { catalog_status, catalog_history };
            } else if (kind === 'discovery') {
                const discoveries = await this.context.client.listProviderDiscoveries(50);
                if (!guard.isCurrent(epoch)) return null;
                await this.refreshCredentials(
                    discoveries.flatMap((session) => {
                        const target = discoveryCredentialTarget(session);
                        return target === null ? [] : [target];
                    }),
                    () => guard.isCurrent(epoch),
                );
                patch = { discoveries };
            } else {
                const groups = await Promise.all(
                    this.context
                        .readState()
                        .providers.workspace.connections.map((connection) =>
                            this.context.client.listProviderModelSyncs(connection.id, 20),
                        ),
                );
                patch = {
                    model_sync_jobs: groups
                        .flat()
                        .sort((left, right) => right.updated_at.localeCompare(left.updated_at)),
                };
            }
            if (guard.isCurrent(epoch)) {
                if (patch.discoveries) {
                    const current = new Map(
                        this.context
                            .readState()
                            .providers.workspace.discoveries.map((session) => [
                                session.id,
                                session,
                            ]),
                    );
                    patch.discoveries = patch.discoveries.map((session) => {
                        const existing = current.get(session.id);
                        return existing && existing.revision > session.revision
                            ? existing
                            : session;
                    });
                }
                this.patch(patch);
            }
            return null;
        } catch (error: unknown) {
            return guard.isCurrent(epoch) ? this.context.errorLabel(error) : null;
        }
    }

    private async refreshCredentials(
        targets: CredentialTargetDto[],
        isCurrent: () => boolean,
    ): Promise<void> {
        await Promise.all(
            targets.map(async (target) => {
                const key = credentialKey(target);
                const epoch = this.credentialEpochs.get(key) ?? 0;
                const status = await this.context.client.credentialStatus(target).then(
                    (result) => result.status,
                    () => 'unreadable' as const,
                );
                if (!isCurrent() || epoch !== (this.credentialEpochs.get(key) ?? 0)) return;
                if (target.kind === 'discovery_session') {
                    const session = this.context
                        .readState()
                        .providers.workspace.discoveries.find(
                            (item) => item.id === target.session_id,
                        );
                    if (session && session.revision > target.expected_revision) return;
                }
                this.context.update((state) => ({
                    ...state,
                    providers: {
                        ...state.providers,
                        workspace: {
                            ...state.providers.workspace,
                            credential_statuses: {
                                ...state.providers.workspace.credential_statuses,
                                [key]: status,
                            },
                        },
                    },
                }));
            }),
        );
    }

    private patch(patch: Partial<ProviderWorkspaceDto>): void {
        this.context.update((state) => ({
            ...state,
            providers: {
                ...state.providers,
                workspace: { ...state.providers.workspace, ...patch },
            },
        }));
    }

    destroy(): void {
        this.epoch.advance();
        for (const epoch of this.diagnostics.values()) epoch.advance();
    }
}

<script lang="ts">
    import type { LorepiaAppState } from '../../../app/app-controller';
    import type {
        CredentialTargetDto,
        ProviderConnectionDto,
        ProviderProfileDto,
    } from '../../../lib/ipc/contracts';

    interface Props {
        appState: LorepiaAppState;
        connection?: ProviderConnectionDto;
        legacyProfile?: ProviderProfileDto;
        retainedLegacyProfileIds: ReadonlySet<string>;
        settingsBusy: boolean;
        selectingProfileId: string | null;
        onOpenDetailPage: (page: string) => void;
        onSelectLegacyProfile: (profileId: string) => void;
    }

    let {
        appState,
        connection,
        legacyProfile,
        retainedLegacyProfileIds,
        settingsBusy,
        selectingProfileId,
        onOpenDetailPage,
        onSelectLegacyProfile,
    }: Props = $props();

    const workspace = $derived(appState.providers.workspace);

    function connectionTarget(connectionId: string): CredentialTargetDto {
        return { kind: 'connection', connection_id: connectionId };
    }

    function profileTarget(profileId: string): CredentialTargetDto {
        return { kind: 'legacy_profile', provider_profile_id: profileId };
    }

    function targetKey(target: CredentialTargetDto): string {
        switch (target.kind) {
            case 'connection':
                return `connection:${target.connection_id}`;
            case 'legacy_profile':
                return `legacy_profile:${target.provider_profile_id}`;
            case 'discovery_session':
                return `discovery_session:${target.session_id}`;
        }
    }

    function statusLabel(key: string): string {
        const status = appState.providers.workspace.credential_statuses[key];
        if (status === 'available') return '자격증명 저장됨';
        if (status === 'unreadable') return '자격증명 확인 불가';
        return '자격증명 없음';
    }

    function routesFor(connectionValue: ProviderConnectionDto) {
        return appState.providers.workspace.routes.filter(
            (route) => route.connection_id === connectionValue.id,
        );
    }

    function presetsFor(routeId: string) {
        return appState.providers.workspace.presets.filter(
            (preset) => preset.model_route_id === routeId,
        );
    }

    function profileSelected(profile: ProviderProfileDto): boolean {
        return appState.providers.workspace.settings.selected_provider_profile_id === profile.id;
    }
</script>

{#if connection}
    {@const target = connectionTarget(connection.id)}
    {@const key = targetKey(target)}
    <section class="detail-read-page connection-detail" aria-label={connection.display_name}>
        <dl class="detail-value-list">
            <div>
                <dt>템플릿</dt>
                <dd>{connection.template_id}</dd>
            </div>
            <div>
                <dt>상태</dt>
                <dd>{connection.status}</dd>
            </div>
            <div>
                <dt>네트워크</dt>
                <dd>{connection.network_mode}</dd>
            </div>
            <div>
                <dt>시간 제한</dt>
                <dd>{connection.timeout_seconds}초</dd>
            </div>
            <div>
                <dt>자격증명</dt>
                <dd>{statusLabel(key)}</dd>
            </div>
        </dl>
        {#if routesFor(connection).length > 0}
            <section class="detail-subsection" aria-label="모델 라우트">
                <h3>모델 라우트</h3>
                <ul class="detail-plain-list">
                    {#each routesFor(connection) as route (route.id)}
                        <li>
                            <strong>{route.display_name ?? route.model_id}</strong>
                            <span>{route.status} · {route.metadata_source}</span>
                            <small
                                >{presetsFor(route.id).length === 0
                                    ? '프리셋 없음'
                                    : presetsFor(route.id)
                                          .map((preset) => preset.display_name)
                                          .join(', ')}</small
                            >
                        </li>
                    {/each}
                </ul>
            </section>
        {/if}
        {#if connection.credential_binding_required && !retainedLegacyProfileIds.has(connection.id)}
            <!-- prettier-ignore -->
            <p class="inline-note">
                자격증명은 클립보드에서 네이티브로 캡처하며 WebView에 전달되지
                않습니다.
            </p>
        {/if}
    </section>
{:else if legacyProfile}
    {@const target = profileTarget(legacyProfile.id)}
    {@const key = targetKey(target)}
    <section class="detail-read-page connection-detail" aria-label={legacyProfile.display_name}>
        <dl class="detail-value-list">
            <div>
                <dt>종류</dt>
                <dd>기존 프로필</dd>
            </div>
            <div>
                <dt>모델</dt>
                <dd>{legacyProfile.model}</dd>
            </div>
            <div>
                <dt>자격증명</dt>
                <dd>{statusLabel(key)}</dd>
            </div>
        </dl>
        <button
            class="detail-secondary-action"
            type="button"
            disabled={settingsBusy || profileSelected(legacyProfile)}
            onclick={() => onSelectLegacyProfile(legacyProfile.id)}
            >{profileSelected(legacyProfile)
                ? '기본 대상으로 사용 중'
                : selectingProfileId === legacyProfile.id
                  ? '기본 대상으로 설정 중'
                  : '기본 대상으로 선택'}</button
        >
        <!-- prettier-ignore -->
        <p class="inline-note">
            자격증명은 클립보드에서 네이티브로 캡처하며 WebView에 전달되지
            않습니다.
        </p>
    </section>
{:else if workspace.connections.length === 0 && workspace.legacy_profiles.length === 0}
    <p class="inline-note">저장된 프로바이더 연결이 없습니다.</p>
{:else}
    <div class="setting-list detail-record-list" aria-label="연결 목록">
        {#each workspace.connections as connectionItem (connectionItem.id)}
            {@const target = connectionTarget(connectionItem.id)}
            <button
                class="setting-row detail-record-row"
                type="button"
                onclick={() => onOpenDetailPage(`connection:${connectionItem.id}`)}
            >
                <span class="setting-content">
                    <span class="setting-copy detail-row-copy">
                        <strong>{connectionItem.display_name}</strong>
                        <small>{connectionItem.template_id} · {connectionItem.status}</small>
                    </span>
                    <span class="setting-value">{statusLabel(targetKey(target))}</span>
                </span>
            </button>
        {/each}
        {#each workspace.legacy_profiles as profile (profile.id)}
            <button
                class="setting-row detail-record-row"
                type="button"
                onclick={() => onOpenDetailPage(`legacy:${profile.id}`)}
            >
                <span class="setting-content">
                    <span class="setting-copy detail-row-copy">
                        <strong>{profile.display_name}</strong>
                        <small>기존 프로필 · {profile.model}</small>
                    </span>
                    <span class="setting-value"
                        >{profileSelected(profile)
                            ? '기본 대상'
                            : statusLabel(targetKey(profileTarget(profile.id)))}</span
                    >
                </span>
            </button>
        {/each}
    </div>
{/if}

<script lang="ts">
    import {
        BriefcaseBusiness,
        CircleDot,
        Link2,
        LayoutTemplate,
        Scale,
        Search,
        SlidersHorizontal,
        SunMoon,
        UserRound,
    } from '@lucide/svelte';
    import lorepiaLogoMark from '../../../assets/lorepia-logo-mark.png';
    import type { LorepiaAppState } from '../../../app/app-controller';
    import { tr } from '../../../lib/i18n';
    import { themePreference } from '../../../lib/theme';
    import type { PersonaState } from '../../personas/persona-controller';
    import type { SettingsSection } from '../settings-contracts';

    interface Props {
        appState: LorepiaAppState;
        desktop: boolean;
        personaState?: PersonaState;
        titlebarOverlay: boolean;
        onSelectSection: (section: SettingsSection) => void;
    }

    let { appState, desktop, personaState, titlebarOverlay, onSelectSection }: Props = $props();

    const entries = [
        { id: 'appearance' as const },
        { id: 'persona' as const },
        { id: 'target' as const },
        { id: 'connections' as const },
        { id: 'templates' as const },
        { id: 'discovery' as const },
        { id: 'catalog' as const },
        { id: 'advanced' as const },
        { id: 'licenses' as const },
    ];

    function settingValue(id: SettingsSection): string {
        const workspace = appState.providers.workspace;
        switch (id) {
            case 'appearance':
                return $themePreference === 'system'
                    ? '시스템 기준'
                    : $themePreference === 'light'
                      ? '라이트'
                      : '다크';
            case 'persona':
                return `${String(personaState?.personas.length ?? 0)}개`;
            case 'target': {
                const legacyProfile = workspace.legacy_profiles.find(
                    (profile) => profile.id === workspace.settings.selected_provider_profile_id,
                );
                if (legacyProfile) return legacyProfile.display_name;
                const preset = workspace.presets.find(
                    (candidate) =>
                        candidate.id === workspace.settings.selected_generation_preset_id,
                );
                if (preset) return preset.display_name;
                const route = workspace.routes.find(
                    (candidate) => candidate.id === workspace.settings.selected_model_route_id,
                );
                return route?.display_name ?? route?.model_id ?? '선택 안 함';
            }
            case 'connections': {
                const count = workspace.connections.length + workspace.legacy_profiles.length;
                return `${String(count)}개 연결`;
            }
            case 'templates':
                return `${String(workspace.templates.length)}개 템플릿`;
            case 'discovery':
                return workspace.selected_discovery_id === null
                    ? `${String(workspace.discoveries.length)}개 기록`
                    : '진행 중';
            case 'catalog':
                return workspace.catalog_status === null
                    ? '기본 카탈로그'
                    : `${String(workspace.catalog_status.active_revision)}차`;
            case 'advanced':
                return `${String(workspace.routes.length)}개 라우트`;
            case 'licenses':
                return 'ISC · MIT';
        }
    }

    function settingDescription(id: SettingsSection): string {
        switch (id) {
            case 'appearance':
                return '앱의 밝기와 화면 표현을 선택합니다.';
            case 'persona':
                return '대화에 사용할 캐릭터 페르소나를 관리합니다.';
            case 'target':
                return '새 대화가 기본으로 사용할 모델과 생성 프리셋입니다.';
            case 'connections':
                return '모델 공급자 연결과 자격증명을 관리합니다.';
            case 'templates':
                return '공급자 연결에 재사용할 요청 템플릿입니다.';
            case 'discovery':
                return '호환 가능한 공급자와 모델을 찾아 기록합니다.';
            case 'catalog':
                return '검증된 모델 카탈로그와 활성 리비전을 관리합니다.';
            case 'advanced':
                return '라우트, 프리셋과 모델 기능을 세부 조정합니다.';
            case 'licenses':
                return 'LorePia와 포함된 라이브러리의 라이선스를 확인합니다.';
        }
    }
</script>

{#snippet tileMark(id: SettingsSection)}
    {#if id === 'appearance'}
        <SunMoon aria-hidden="true" />
    {:else if id === 'persona'}
        <UserRound aria-hidden="true" />
    {:else if id === 'target'}
        <CircleDot aria-hidden="true" />
    {:else if id === 'connections'}
        <Link2 aria-hidden="true" />
    {:else if id === 'templates'}
        <LayoutTemplate aria-hidden="true" />
    {:else if id === 'discovery'}
        <Search aria-hidden="true" />
    {:else if id === 'catalog'}
        <BriefcaseBusiness aria-hidden="true" />
    {:else if id === 'advanced'}
        <SlidersHorizontal aria-hidden="true" />
    {:else}
        <Scale aria-hidden="true" />
    {/if}
{/snippet}

{#snippet desktopSummaryRow(id: SettingsSection)}
    <button class="desktop-settings-summary-row" type="button" onclick={() => onSelectSection(id)}>
        <span class="desktop-settings-summary-copy">
            <strong>{$tr(`settings.section.${id}.title`)}</strong>
            <small>{settingDescription(id)}</small>
        </span>
        <span class="desktop-settings-summary-value">{settingValue(id)}</span>
    </button>
{/snippet}

{#if desktop}
    <section class="desktop-settings-section" aria-labelledby="general-conversation-title">
        <h2 id="general-conversation-title">대화 환경</h2>
        <div class="desktop-settings-card">
            {@render desktopSummaryRow('appearance')}
            {@render desktopSummaryRow('persona')}
            {@render desktopSummaryRow('target')}
        </div>
    </section>

    <section class="desktop-settings-section" aria-labelledby="general-provider-title">
        <h2 id="general-provider-title">모델과 데이터</h2>
        <div class="desktop-settings-card">
            {@render desktopSummaryRow('connections')}
            {@render desktopSummaryRow('templates')}
            {@render desktopSummaryRow('discovery')}
            {@render desktopSummaryRow('catalog')}
        </div>
    </section>

    <section class="desktop-settings-section" aria-labelledby="general-information-title">
        <h2 id="general-information-title">정보</h2>
        <div class="desktop-settings-card">
            {@render desktopSummaryRow('advanced')}
            {@render desktopSummaryRow('licenses')}
        </div>
    </section>
{:else}
    <section class="settings-identity" aria-labelledby="settings-identity-title">
        <span class="settings-avatar-wrap" aria-hidden="true">
            <span
                class="settings-avatar brand-logo-mark"
                style:--logo-mask={`url("${lorepiaLogoMark}")`}
            ></span>
            <span class="settings-avatar-badge">
                <SlidersHorizontal />
            </span>
        </span>
        <div
            class="settings-identity-copy"
            data-tauri-drag-region={titlebarOverlay ? '' : undefined}
        >
            <h2
                id="settings-identity-title"
                data-tauri-drag-region={titlebarOverlay ? '' : undefined}
            >
                LorePia
            </h2>
        </div>
    </section>
    <ul class="setting-list">
        {#each entries as entry (entry.id)}
            <li>
                <button class="setting-row" type="button" onclick={() => onSelectSection(entry.id)}>
                    <span class="setting-icon" aria-hidden="true">
                        {@render tileMark(entry.id)}
                    </span>
                    <span class="setting-content">
                        <span class="setting-copy">
                            <strong>{$tr(`settings.section.${entry.id}.title`)}</strong>
                        </span>
                        <span class="setting-trailing">
                            <span class="setting-value">{settingValue(entry.id)}</span>
                        </span>
                    </span>
                </button>
            </li>
        {/each}
    </ul>
{/if}

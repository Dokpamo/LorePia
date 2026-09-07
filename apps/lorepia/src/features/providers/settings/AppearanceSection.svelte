<script lang="ts">
    import { Check, Sun, Moon, Monitor } from '@lucide/svelte';
    import { tr } from '../../../lib/i18n';
    import { setThemePreference, themePreference } from '../../../lib/theme';
    import ChoiceField from '../../../components/ChoiceField.svelte';
    import { chatTextSize, setChatTextSize } from '../../../lib/display';

    interface Props {
        desktop: boolean;
    }

    let { desktop }: Props = $props();

    const themeIcons = { system: Monitor, light: Sun, dark: Moon };
    const themeOptions = [
        { id: 'system' as const, label: '시스템' },
        { id: 'light' as const, label: '라이트 모드' },
        { id: 'dark' as const, label: '다크 모드' },
    ];
</script>

{#if desktop}
    <section
        class="desktop-settings-section appearance-theme-section"
        aria-labelledby="appearance-theme-title"
    >
        <h2 id="appearance-theme-title">테마</h2>
        <div class="theme-preview-grid" role="group" aria-label="화면 모드 선택">
            {#each themeOptions as option (option.id)}
                <button
                    type="button"
                    class={`theme-preview-option theme-preview-${option.id}`}
                    aria-pressed={$themePreference === option.id}
                    onclick={() => setThemePreference(option.id)}
                >
                    <span class="theme-preview-canvas" aria-hidden="true">
                        <span class="theme-preview-sidebar"></span>
                        <span class="theme-preview-main">
                            <span class="theme-preview-title"></span>
                            <span class="theme-preview-line theme-preview-line-long"></span>
                            <span class="theme-preview-line"></span>
                            <span class="theme-preview-composer"></span>
                        </span>
                    </span>
                    <span class="theme-preview-label">
                        <span>{option.label}</span>
                        {#if $themePreference === option.id}
                            <Check aria-hidden="true" />
                        {/if}
                    </span>
                </button>
            {/each}
        </div>
    </section>

    <section class="desktop-settings-section" aria-labelledby="appearance-behavior-title">
        <h2 id="appearance-behavior-title">표시 방식</h2>
        <div class="desktop-settings-card">
            <div class="desktop-settings-static-row">
                <span class="desktop-settings-summary-copy">
                    <strong>시스템 테마 연동</strong>
                    <!-- prettier-ignore -->
                    <small
                        >시스템 모드에서는 운영체제의 밝기 설정을 자동으로 따릅니다.</small
                    >
                </span>
                <span class="desktop-settings-summary-value">
                    {$themePreference === 'system' ? '사용' : '사용 안 함'}
                </span>
            </div>
        </div>
    </section>
{:else}
    <p class="mobile-detail-intro">{$tr('mobile.theme.hint')}</p>
    <div class="mobile-theme-options" role="group" aria-label={$tr('mobile.theme.label')}>
        {#each themeOptions as option (option.id)}
            {@const Icon = themeIcons[option.id]}
            <button
                class="mobile-theme-option"
                type="button"
                aria-pressed={$themePreference === option.id}
                onclick={() => setThemePreference(option.id)}
            >
                <span class="mobile-theme-icon" aria-hidden="true"><Icon /></span>
                <span>{$tr(`mobile.theme.${option.id}`)}</span>
                {#if $themePreference === option.id}<Check
                        class="mobile-theme-check"
                        aria-hidden="true"
                    />{/if}
            </button>
        {/each}
    </div>
{/if}

<section class="settings-purpose-card settings-form">
    <ChoiceField id="chat-text-size" label={$tr('settingsLive.textSize')} value={$chatTextSize} options={[{ value: 'normal', label: $tr('settingsLive.textNormal') }, { value: 'large', label: $tr('settingsLive.textLarge') }]} onSelect={(value) => { if (value === 'normal' || value === 'large') setChatTextSize(value); }} />
    <p class="settings-note">{$tr('settingsLive.textHint')}</p>
</section>

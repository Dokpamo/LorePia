<script lang="ts">
    import { ArrowLeft } from '@lucide/svelte';
    import MobileDetailHeader from '../components/mobile/MobileDetailHeader.svelte';
    import { tr } from '../lib/i18n';

    interface Props {
        title: string;
        desktop: boolean;
        titlebarOverlay: boolean;
        fadeProgress: number;
        onBack: () => void;
        titleElement?: HTMLHeadingElement;
    }
    let {
        title,
        desktop,
        titlebarOverlay,
        fadeProgress,
        onBack,
        titleElement = $bindable(),
    }: Props = $props();
</script>

{#if !desktop}
    <MobileDetailHeader {title} {titlebarOverlay} {onBack} bind:titleElement />
{:else}
    <header
        class="mobile-top-frame mobile-top-frame-leading sub-header"
        data-tauri-drag-region={titlebarOverlay ? '' : undefined}
        style:--mobile-top-fade-progress={fadeProgress}
    >
        <button
            class="icon-button ghost mobile-top-action mobile-top-action-left back-button"
            type="button"
            aria-label={$tr('app.nav.back')}
            onclick={onBack}><ArrowLeft aria-hidden="true" /></button
        >
        <h1
            bind:this={titleElement}
            tabindex="-1"
            data-tauri-drag-region={titlebarOverlay ? '' : undefined}
        >
            {title}
        </h1>
    </header>
{/if}

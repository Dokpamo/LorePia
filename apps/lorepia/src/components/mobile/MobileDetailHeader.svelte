<script lang="ts">
    import { ArrowLeft } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';

    interface Props {
        title: string;
        onBack: () => void;
        titleElement?: HTMLHeadingElement | null;
        titlebarOverlay?: boolean;
    }
    let { title, onBack, titleElement = $bindable(), titlebarOverlay = false }: Props = $props();
</script>

<header class="mobile-detail-header" data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
    <button
        class="mobile-icon-button"
        type="button"
        aria-label={$tr('app.nav.back')}
        onclick={onBack}
    >
        <ArrowLeft aria-hidden="true" />
    </button>
    <h1
        bind:this={titleElement}
        tabindex="-1"
        data-tauri-drag-region={titlebarOverlay ? '' : undefined}
    >
        {title}
    </h1>
</header>

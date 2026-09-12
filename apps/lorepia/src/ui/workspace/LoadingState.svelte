<script lang="ts">
    let { label, cards = false }: { label: string; cards?: boolean } = $props();
</script>

<div class="ui-loading-state" class:ui-loading-cards={cards} role="status">
    <p>{label}</p>
    <div class="ui-loading-shapes" aria-hidden="true">
        {#each [0, 1, 2] as item (item)}<span></span>{/each}
    </div>
</div>

<style>
    .ui-loading-state {
        padding: 16px 0;
        color: var(--ui-text-secondary, #666);
    }
    p {
        margin: 0 0 16px;
        font-size: 14px;
    }
    .ui-loading-shapes {
        display: grid;
        gap: 14px;
    }
    span {
        display: block;
        width: 80%;
        height: 18px;
        border-radius: 6px;
        background: currentColor;
        opacity: 0.12;
    }
    span:nth-child(2) {
        width: 64%;
    }
    span:nth-child(3) {
        width: 72%;
    }
    .ui-loading-cards .ui-loading-shapes {
        grid-template-columns: repeat(3, 1fr);
    }
    .ui-loading-cards span {
        width: 100%;
        height: auto;
        aspect-ratio: 1;
        border-radius: 16px;
    }
</style>

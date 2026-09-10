<script lang="ts">
    let {
        characterName,
        title,
        titlebarOverlay,
    }: { characterName?: string; title: string; titlebarOverlay: boolean } = $props();
</script>

<div data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
    <h2 id="chat-title" data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
        {characterName?.trim() ? characterName : title}
    </h2>
    {#if characterName && characterName !== title}
        <p class="chat-subtitle" data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
            {title}
        </p>
    {/if}
</div>

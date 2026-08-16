<script lang="ts">
    import type { MarkdownInline } from './markdown';
    import Self from './MarkdownInline.svelte';

    interface Props {
        nodes: MarkdownInline[];
    }

    let { nodes }: Props = $props();
</script>

{#each nodes as node, index (index)}
    {#if node.kind === 'text'}{node.value}{:else if node.kind === 'code'}<code>{node.value}</code
        >{:else if node.kind === 'strong'}<strong><Self nodes={node.children} /></strong
        >{:else if node.kind === 'emphasis'}<em><Self nodes={node.children} /></em
        >{:else if node.kind === 'link'}<span class="link" title={node.href}
            ><Self nodes={node.children} /></span
        >{/if}
{/each}

<style>
    code {
        padding: 0.1em 0.3em;
        border-radius: 4px;
        background: var(--surface-sunken);
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        font-size: 0.92em;
    }

    /*
     * Links render as styled text, not anchors: the webview CSP forbids
     * navigation, so an anchor would be a dead control that looks live.
     */
    .link {
        color: var(--accent);
        text-decoration: underline;
        text-underline-offset: 2px;
    }
</style>

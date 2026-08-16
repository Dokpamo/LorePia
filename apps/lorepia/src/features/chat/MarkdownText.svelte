<script lang="ts">
    import MarkdownInline from './MarkdownInline.svelte';
    import { parseMarkdown } from './markdown';

    interface Props {
        text: string;
    }

    let { text }: Props = $props();
    const blocks = $derived(parseMarkdown(text));
</script>

<div class="markdown">
    {#each blocks as block, index (index)}
        {#if block.kind === 'paragraph'}
            <p><MarkdownInline nodes={block.children} /></p>
        {:else if block.kind === 'code'}
            <pre><code>{block.value}</code></pre>
        {:else if block.kind === 'quote'}
            <blockquote>
                {#each block.lines as line, lineIndex (lineIndex)}
                    <p><MarkdownInline nodes={line} /></p>
                {/each}
            </blockquote>
        {:else if block.kind === 'list'}
            {#if block.ordered}
                <ol>
                    {#each block.items as item, itemIndex (itemIndex)}
                        <li><MarkdownInline nodes={item} /></li>
                    {/each}
                </ol>
            {:else}
                <ul>
                    {#each block.items as item, itemIndex (itemIndex)}
                        <li><MarkdownInline nodes={item} /></li>
                    {/each}
                </ul>
            {/if}
        {:else if block.kind === 'rule'}
            <hr />
        {/if}
    {/each}
</div>

<style>
    .markdown {
        display: grid;
        gap: 0.6em;
    }

    p {
        margin: 0;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
    }

    pre {
        margin: 0;
        padding: 10px;
        overflow-x: auto;
        border-radius: 8px;
        background: var(--surface-sunken);
    }

    pre code {
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        font-size: 0.9em;
        white-space: pre;
    }

    blockquote {
        display: grid;
        gap: 0.4em;
        margin: 0;
        padding-left: 10px;
        border-left: 3px solid var(--line);
        color: var(--ink-muted);
    }

    ol,
    ul {
        display: grid;
        gap: 0.25em;
        margin: 0;
        padding-left: 1.4em;
    }

    hr {
        width: 100%;
        margin: 0;
        border: 0;
        border-top: 1px solid var(--line);
    }
</style>

<script lang="ts">
    import { untrack } from 'svelte';
    import {
        ArrowLeft,
        Check,
        Copy,
        GitBranch,
        Pencil,
        Plus,
        RotateCcw,
        Trash2,
    } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import { scale } from 'svelte/transition';
    import { positionMessageMenu } from './message-menu-position';
    import { trapFocus } from './focus-trap';
    import type { SampleBranch, SampleMessage } from './view-types';
    import './ui-message-menu.css';
    let {
        anchor,
        initialView = 'actions',
        message,
        busy,
        branches,
        branchId,
        onclose,
        oncopy,
        onedit,
        onfork,
        onbranch,
        onremove,
        onregenerate,
    }: {
        anchor: HTMLElement;
        initialView?: 'actions' | 'branches';
        message: SampleMessage;
        busy: boolean;
        branches: SampleBranch[];
        branchId?: string;
        onclose: (restoreFocus: boolean) => void;
        oncopy: () => void;
        onedit: () => void;
        onfork: () => void;
        onbranch: (id: string) => void;
        onremove: () => void;
        onregenerate: () => void;
    } = $props();
    let view = $state<'actions' | 'branches' | 'remove'>(untrack(() => initialView));
    let closing = $state(false);
    const exitDuration = matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 100;
    function dismiss(restoreFocus: boolean) {
        if (closing) return;
        closing = true;
        onclose(restoreFocus);
    }
    function choose(action: () => void) {
        if (closing) return;
        dismiss(true);
        action();
    }
</script>

<div
    class="ui-message-menu"
    class:ui-message-menu-closing={closing}
    inert={closing}
    aria-hidden={closing}
    out:scale={{ duration: exitDuration, start: 0.95 }}
    onoutrostart={() => (closing = true)}
    onpointerdowncapture={(event: PointerEvent) => event.stopPropagation()}
    onwheelcapture={(event: WheelEvent) => event.stopPropagation()}
    role={view === 'remove' ? 'alertdialog' : 'menu'}
    aria-label={$tr(
        view === 'remove'
            ? 'uiPreview.removeConfirm'
            : view === 'branches'
              ? 'uiPreview.branch'
              : 'uiPreview.messageTools',
    )}
    data-ui-no-swipe
    use:positionMessageMenu={{ anchor, onclose: dismiss, view }}
    onkeydowncapture={(event: KeyboardEvent & { currentTarget: HTMLDivElement }) => {
        if (event.key === 'Escape') {
            event.preventDefault();
            event.stopPropagation();
            dismiss(true);
            return;
        }
        if (view === 'remove') {
            trapFocus(event);
            return;
        }
        if (event.key === 'Tab') {
            event.preventDefault();
            dismiss(true);
            return;
        }
        if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
        event.preventDefault();
        event.stopPropagation();
        const root = event.currentTarget;
        const items = [
            ...root.querySelectorAll<HTMLButtonElement>('[role^="menuitem"]:not(:disabled)'),
        ];
        const index = items.indexOf(document.activeElement as HTMLButtonElement);
        const next =
            event.key === 'Home'
                ? 0
                : event.key === 'End'
                  ? items.length - 1
                  : (index + (event.key === 'ArrowUp' ? -1 : 1) + items.length) % items.length;
        items[next]?.focus({ preventScroll: true });
    }}
>
    {#if view === 'remove'}
        <div class="ui-message-menu-confirm">
            <strong>{$tr('uiPreview.removeConfirm')}</strong>
            <p>{$tr('uiPreview.removeMessageHint')}</p>
            <div class="ui-confirm-actions">
                <button
                    class="ui-discard ui-pressable"
                    disabled={busy}
                    onclick={() => choose(onremove)}
                    ><span class="ui-press-visual">{$tr('uiPreview.confirmRemove')}</span></button
                >
                <button
                    class="ui-submit ui-pressable"
                    data-menu-initial
                    onclick={() => (view = 'actions')}
                    ><span class="ui-press-visual">{$tr('uiPreview.cancel')}</span></button
                >
            </div>
        </div>
    {:else if view === 'branches'}
        <button role="menuitem" class="ui-menu-item ui-pressable" onclick={() => (view = 'actions')}
            ><span class="ui-press-visual"><ArrowLeft />{$tr('uiPreview.back')}</span></button
        >
        <button
            role="menuitem"
            class="ui-menu-item ui-pressable"
            disabled={busy}
            onclick={() => choose(onfork)}
            ><span class="ui-press-visual"><Plus />{$tr('uiPreview.branchFrom')}</span></button
        >
        {#if branches.length > 1}<hr />{/if}
        {#each branches as branch (branch.id)}
            {#if branches.length > 1}<button
                    role="menuitemradio"
                    aria-checked={branch.id === branchId}
                    class="ui-menu-item ui-pressable"
                    disabled={busy}
                    onclick={() => choose(() => onbranch(branch.id))}
                    ><span class="ui-press-visual"
                        ><GitBranch /><span>{branch.title}</span>{#if branch.id === branchId}<Check
                            />{/if}</span
                    ></button
                >{/if}
        {/each}
    {:else}
        <button
            role="menuitem"
            class="ui-menu-item ui-pressable"
            aria-label={$tr('uiPreview.copyMessage')}
            onclick={() => choose(oncopy)}
            ><span class="ui-press-visual"><Copy />{$tr('uiPreview.copyTool')}</span></button
        >
        {#if message.role === 'user'}<button
                role="menuitem"
                class="ui-menu-item ui-pressable"
                disabled={busy}
                aria-label={$tr('uiPreview.editMessage')}
                onclick={() => choose(onedit)}
                ><span class="ui-press-visual"><Pencil />{$tr('uiPreview.editTool')}</span></button
            >{/if}
        <button
            role="menuitem"
            class="ui-menu-item ui-pressable"
            disabled={busy}
            aria-haspopup="menu"
            onclick={() => (view = 'branches')}
            ><span class="ui-press-visual"><GitBranch />{$tr('uiPreview.branch')}</span></button
        >
        {#if message.role === 'assistant'}<button
                role="menuitem"
                class="ui-menu-item ui-pressable"
                disabled={busy}
                onclick={() => choose(onregenerate)}
                ><span class="ui-press-visual"><RotateCcw />{$tr('uiPreview.regenerate')}</span
                ></button
            >{/if}
        <hr />
        <button
            role="menuitem"
            class="ui-menu-item ui-pressable"
            disabled={busy}
            aria-label={$tr('uiPreview.removeFrom')}
            onclick={() => (view = 'remove')}
            ><span class="ui-press-visual"><Trash2 />{$tr('uiPreview.deleteTool')}</span></button
        >
    {/if}
</div>

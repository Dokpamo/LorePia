<script lang="ts">
    import { tr } from '../../lib/i18n';
    import { choiceSheetDrag, setSheetExpanded } from './choice-sheet-motion';

    let {
        panel,
        onclose,
        expanded = $bindable(false),
        disabled = false,
    }: {
        panel: () => HTMLElement;
        onclose: () => void;
        expanded?: boolean;
        disabled?: boolean;
    } = $props();
    function resize(next: boolean) {
        if (disabled) return;
        expanded = next;
        setSheetExpanded(panel(), next);
    }
</script>

<button
    type="button"
    class="ui-choice-handle"
    aria-label={$tr(expanded ? 'uiPreview.collapseSheet' : 'uiPreview.expandSheet')}
    aria-expanded={expanded}
    {disabled}
    use:choiceSheetDrag={{
        panel,
        onclose,
        enabled: !disabled,
        onexpanded: (next) => (expanded = next),
    }}
    onclick={() => resize(!expanded)}
    onkeydown={(event) => {
        if (!['ArrowUp', 'ArrowDown'].includes(event.key)) return;
        event.preventDefault();
        event.stopPropagation();
        if (event.key === 'ArrowDown' && !expanded) onclose();
        else resize(event.key === 'ArrowUp');
    }}><span aria-hidden="true"></span></button
>

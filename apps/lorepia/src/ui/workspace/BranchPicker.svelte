<script lang="ts">
    import { GitBranch } from '@lucide/svelte';
    import { t, tr } from '../../lib/i18n';
    import { useChoiceSheet } from './choice-sheet.svelte';

    let {
        branches,
        value,
        disabled,
        onselect,
    }: {
        branches: { id: string; title: string }[];
        value: string;
        disabled: boolean;
        onselect: (value: string) => void;
    } = $props();
    const sheet = useChoiceSheet();
</script>

<button
    type="button"
    class="ui-icon-button ui-branch-select ui-pressable"
    aria-label={$tr('uiPreview.branch')}
    aria-haspopup="dialog"
    data-ui-tooltip={branches.find((branch) => branch.id === value)?.title}
    {disabled}
    onclick={(event) =>
        sheet.open(
            {
                label: t('uiPreview.branch'),
                value,
                options: branches.map((branch) => ({ value: branch.id, label: branch.title })),
                onselect,
            },
            event.currentTarget,
        )}
>
    <span class="ui-press-visual" aria-hidden="true"><GitBranch /></span>
</button>

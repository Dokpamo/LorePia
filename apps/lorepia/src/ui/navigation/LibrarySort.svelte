<script lang="ts">
    import { EllipsisVertical } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import { useChoiceSheet } from '../workspace/choice-sheet.svelte';
    import type { LibrarySortOrder } from './navigation-types';

    let {
        value = $bindable<LibrarySortOrder>('newest'),
        label,
        nameLabel,
    }: { value?: LibrarySortOrder; label: string; nameLabel?: string } = $props();
    const choices = useChoiceSheet();
    const labels = $derived({
        newest: $tr('navigation.sortNewest'),
        oldest: $tr('navigation.sortOldest'),
        name: nameLabel ?? $tr('navigation.sortName'),
    });
</script>

<button
    type="button"
    class="seed-library-sort ui-icon-button ui-pressable"
    aria-label={label}
    aria-haspopup="dialog"
    aria-expanded={choices.request?.label === label}
    data-ui-tooltip={label}
    onclick={(event) =>
        choices.open(
            {
                label,
                value,
                options: Object.entries(labels).map(([value, label]) => ({ value, label })),
                onselect: (selected) => {
                    if (selected === 'newest' || selected === 'oldest' || selected === 'name')
                        value = selected;
                },
            },
            event.currentTarget,
        )}><span class="ui-press-visual"><EllipsisVertical aria-hidden="true" /></span></button
>

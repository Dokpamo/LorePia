<script lang="ts">
    import { Plus } from '@lucide/svelte';
    import { tr, t } from '../../lib/i18n';
    import type { SampleCharacter } from '../workspace/view-types';
    import { useChoiceSheet } from '../workspace/choice-sheet.svelte';
    import IconButton from '../workspace/IconButton.svelte';
    let {
        characters,
        onselect,
    }: {
        characters: SampleCharacter[];
        onselect: (id: string, trigger: HTMLButtonElement) => void;
    } = $props();
    const choices = useChoiceSheet();
    function choose(trigger: HTMLButtonElement) {
        choices.open(
            {
                label: t('uiPreview.newChat'),
                value: '',
                options: characters.map((item) => ({ value: item.id, label: item.name })),
                onselect: (id) => onselect(id, trigger),
            },
            trigger,
        );
    }
</script>

<IconButton
    label={$tr('uiPreview.newChat')}
    disabled={!characters.length}
    onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
        choose(event.currentTarget)}><Plus /></IconButton
>

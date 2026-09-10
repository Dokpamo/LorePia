<script lang="ts">
    import SettingsRow from '../../../ui/workspace/SettingsRow.svelte';
    let {
        label,
        onclick,
        disabled = false,
    }: { label: string; onclick: () => void; disabled?: boolean } = $props();
</script>

<SettingsRow {label} {disabled} {onclick} />

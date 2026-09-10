<script lang="ts">
    import { locale } from '../../../lib/i18n';
    import { reviewRows } from './ai-review';
    let { label, value }: { label: string; value: unknown } = $props();
    const rows = $derived.by(() => {
        void $locale;
        return reviewRows(value, label);
    });
</script>

<section aria-label={label}>
    <h3 class="ui-settings-heading">{label}</h3>
    <dl class="ui-card-details">
        {#each rows as row, index (index)}
            <dt>{row.label}</dt>
            <dd>{row.value}</dd>
        {/each}
    </dl>
</section>

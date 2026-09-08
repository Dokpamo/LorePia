<script lang="ts">
    import { untrack } from 'svelte';
    import { tr, t } from '../../lib/i18n';
    import type { CreatorKnowledgeBookDocumentDto } from '../../lib/ipc/contracts';
    import type {
        OrchestrationState,
        CreatorDocumentKind,
        CreatorDocumentValue,
    } from '../../features/orchestration/orchestration-controller';
    import type { CreatorDocumentController } from '../../features/orchestration/controllers/creator-document-controller';
    import SettingsPanel from '../workspace/SettingsPanel.svelte';
    import NavigationRow from './NavigationRow.svelte';
    import { useTextEditor } from '../workspace/text-editor.svelte';
    import EditField from '../workspace/EditField.svelte';
    import DiscardChanges from '../workspace/DiscardChanges.svelte';
    let {
        value,
        kind,
        title,
        controller,
        orchestrationState,
        onclose,
    }: {
        value: CreatorDocumentValue;
        kind: CreatorDocumentKind;
        title: string;
        controller: CreatorDocumentController;
        orchestrationState: OrchestrationState;
        onclose: () => void;
    } = $props();
    const editor = useTextEditor();
    let draft = $state(untrack(() => structuredClone($state.snapshot(value))));
    let original = $state(untrack(() => JSON.stringify(value)));
    let advanced = $state<string | null>(null);
    let confirming = $state(false);
    let busy = $state(false);
    let error = $state('');
    let notice = $state('');
    const dirty = $derived(advanced !== null || JSON.stringify(draft) !== original);
    const book = $derived(
        kind === 'knowledge_book' ? (draft as CreatorKnowledgeBookDocumentDto) : null,
    );
    function beforeback() {
        if (busy) return false;
        if (dirty) {
            confirming = true;
            return false;
        }
        return true;
    }
    function addEntry() {
        if (!book) return;
        book.entries.push({
            id: crypto.randomUUID(),
            name: '',
            content: '',
            enabled: true,
            activation: {
                kind: 'keyword',
                primary: [],
                secondary: [],
                selective: false,
                case_sensitive: false,
                whole_word: false,
            },
            priority: 0,
            importance: 1,
            placement: 'retrieved_context',
            token_policy: { priority: 0, min_tokens: null, max_tokens: null, reserve_tokens: null },
            parent_id: null,
            activation_probability_basis_points: 10000,
        });
    }
    async function save() {
        if (busy) return;
        error = '';
        notice = '';
        let submitted: CreatorDocumentValue;
        try {
            const parsed: unknown =
                advanced === null ? $state.snapshot(draft) : JSON.parse(advanced);
            if (
                typeof parsed !== 'object' ||
                parsed === null ||
                !('id' in parsed) ||
                parsed.id !== draft.id ||
                !('name' in parsed) ||
                typeof parsed.name !== 'string' ||
                !parsed.name.trim()
            )
                throw new Error();
            if (
                kind === 'content_module' &&
                (!('asset_ids' in parsed) ||
                    !Array.isArray(parsed.asset_ids) ||
                    parsed.asset_ids.length)
            )
                throw new Error();
            submitted = parsed as CreatorDocumentValue;
        } catch {
            error = t('navigation.invalidDocument');
            return;
        }
        busy = true;
        try {
            // Existing rows retain their expected revision; new rows acquire it only after save.
            const collections = [
                orchestrationState.editable_knowledge_books,
                orchestrationState.editable_memory_profiles,
                orchestrationState.editable_transform_sets,
                orchestrationState.editable_interaction_rule_sets,
                orchestrationState.editable_content_modules,
            ];
            if (!collections.some((items) => items.some((item) => item.value.id === submitted.id)))
                controller.addCreatorDocumentDraft(kind, submitted.id);
            if (
                !controller.replaceCreatorDocument(kind, submitted.id, submitted) ||
                !(await controller.saveCreatorDocument(kind, submitted.id))
            ) {
                error = t('navigation.saveFailed');
                return;
            }
            // Retain the normalized local presentation only after native validation succeeds.
            draft = submitted;
            advanced = null;
            original = JSON.stringify(submitted);
            notice = t('navigation.saved');
        } finally {
            busy = false;
        }
    }
</script>

<SettingsPanel {title} {onclose} {beforeback} disabled={busy} covered={confirming}>
    <fieldset disabled={busy} class="seed-editor-fields">
        {#if advanced === null}
            <EditField
                label={$tr('navigation.name')}
                value={draft.name}
                maxlength={256}
                requiredMessage={$tr('navigation.requiredName')}
                onchange={(name: string) => (draft.name = name)}
            />
            {#if book}
                {#each book.entries as entry (entry.id)}
                    <section class="seed-material-entry">
                        <EditField
                            label={$tr('navigation.name')}
                            value={entry.name}
                            maxlength={256}
                            onchange={(name: string) => (entry.name = name)}
                        />
                        {#if entry.activation.kind === 'keyword'}
                            <EditField
                                label={$tr('navigation.condition')}
                                value={entry.activation.primary.join(', ')}
                                hint={$tr('navigation.conditionHint')}
                                maxlength={4096}
                                onchange={(keywords: string) => {
                                    if (entry.activation.kind === 'keyword')
                                        entry.activation.primary = keywords
                                            .split(',')
                                            .map((word) => word.trim())
                                            .filter(Boolean);
                                }}
                            />
                        {/if}
                        <EditField
                            label={$tr('navigation.content')}
                            value={entry.content}
                            maxlength={65536}
                            onchange={(content: string) => (entry.content = content)}
                        />
                        <button
                            class="seed-secondary ui-pressable"
                            onclick={() => {
                                book.entries = book.entries.filter((item) => item.id !== entry.id);
                            }}
                            ><span class="ui-press-visual">{$tr('navigation.removeEntry')}</span
                            ></button
                        >
                    </section>
                {/each}
                <button class="seed-secondary ui-pressable" onclick={addEntry}
                    ><span class="ui-press-visual">{$tr('navigation.addEntry')}</span></button
                >
            {/if}
        {/if}
        <NavigationRow
            title={$tr('navigation.advancedDocument')}
            description={$tr('navigation.advancedHint')}
            onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                editor.open(
                    {
                        label: t('navigation.advancedDocument'),
                        value: advanced ?? JSON.stringify(draft, null, 2),
                        maxlength: 262144,
                        onchange: (json) => (advanced = json),
                    },
                    event.currentTarget,
                )}
        />
        <button
            class="seed-primary ui-pressable"
            disabled={busy || !dirty}
            onclick={() => void save()}
            ><span class="ui-press-visual">{$tr('navigation.save')}</span></button
        >
    </fieldset>
    {#if error}<p role="alert">{error}</p>{/if}
    {#if notice}<p role="status">{notice}</p>{/if}
</SettingsPanel>
{#if confirming}<DiscardChanges onkeep={() => (confirming = false)} ondiscard={onclose} />{/if}

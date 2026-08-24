<script lang="ts">
    import { Plus } from '@lucide/svelte';
    import { tick } from 'svelte';

    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import type {
        CreatorDocumentKind,
        CreatorDocumentValue,
        EditableCreatorDocumentState,
        OrchestrationController,
        OrchestrationState,
    } from './orchestration-controller';

    interface Props {
        orchestrationState: OrchestrationState;
        controller: OrchestrationController;
        detailPage?: string | null;
    }

    interface CreatorDocumentFamily {
        kind: CreatorDocumentKind;
        title: string;
        create_label: string;
        guide: string;
    }

    type EditableCreatorDocument = EditableCreatorDocumentState<CreatorDocumentValue>;
    type CreatorDocumentRoute =
        | { mode: 'index'; kind: null; id: null }
        | { mode: 'list' | 'create'; kind: CreatorDocumentKind; id: null }
        | { mode: 'edit'; kind: CreatorDocumentKind; id: string };

    const MAX_DOCUMENT_JSON_CHARS = 262_144;
    const CREATOR_DOCUMENT_FAMILIES: readonly CreatorDocumentFamily[] = [
        {
            kind: 'memory_profile',
            title: '메모리 프로필',
            create_label: '새 메모리 프로필 ID',
            guide: 'summary_task, summary_schema, 세 토큰 예산, retrieval/weight 값을 편집합니다.',
        },
        {
            kind: 'knowledge_book',
            title: '지식 책',
            create_label: '새 지식 책 ID',
            guide: 'entries는 안전한 activation AST, placement, token_policy를 가진 typed 배열입니다.',
        },
        {
            kind: 'transform_set',
            title: '변환 세트',
            create_label: '새 변환 세트 ID',
            guide: 'rules는 제한된 정규식 descriptor와 선언형 condition만 허용하며 스크립트를 허용하지 않습니다.',
        },
        {
            kind: 'interaction_rule_set',
            title: '상호작용 규칙 세트',
            create_label: '새 상호작용 규칙 세트 ID',
            guide: 'rules는 닫힌 event/action union만 허용하며 임의 코드나 네트워크 작업을 표현할 수 없습니다.',
        },
        {
            kind: 'content_module',
            title: '콘텐츠 모듈',
            create_label: '새 콘텐츠 모듈 ID',
            guide: '구성 요소에 맞는 required_capabilities를 선언해야 합니다. 이 경로에서는 asset_ids가 비어 있어야 합니다.',
        },
    ];

    let { orchestrationState, controller, detailPage = $bindable('documents') }: Props = $props();
    let newIds = $state<Record<CreatorDocumentKind, string>>({
        memory_profile: '',
        knowledge_book: '',
        transform_set: '',
        interaction_rule_set: '',
        content_module: '',
    });
    let createErrors = $state<Partial<Record<CreatorDocumentKind, string>>>({});
    let jsonDrafts = $state<Record<string, string>>({});
    let jsonErrors = $state<Record<string, string>>({});
    let pendingDeleteKey = $state<string | null>(null);
    let draftContextKey = '';
    let lastDetailPage = detailPage;

    const route = $derived(parseDocumentRoute(detailPage));
    const selectedFamily = $derived(
        route.kind === null
            ? null
            : (CREATOR_DOCUMENT_FAMILIES.find((family) => family.kind === route.kind) ?? null),
    );
    const selectedDocument = $derived(
        route.mode === 'edit'
            ? (documentsFor(route.kind).find((document) => document.value.id === route.id) ?? null)
            : null,
    );
    const busy = $derived(orchestrationState.editable_creator_documents_loading);

    $effect(() => {
        if (draftContextKey === orchestrationState.context_key) return;
        if (draftContextKey !== '') detailPage = 'documents';
        draftContextKey = orchestrationState.context_key;
        jsonDrafts = {};
        jsonErrors = {};
        createErrors = {};
        pendingDeleteKey = null;
    });

    $effect(() => {
        const currentPage = detailPage;
        if (currentPage === lastDetailPage) return;
        lastDetailPage = currentPage;
        pendingDeleteKey = null;
    });

    function familyRoute(kind: CreatorDocumentKind): string {
        return `documents/${kind}`;
    }

    function createRoute(kind: CreatorDocumentKind): string {
        return `${familyRoute(kind)}/create`;
    }

    function editRoute(kind: CreatorDocumentKind, id: string): string {
        return `${familyRoute(kind)}/edit/${encodeURIComponent(id)}`;
    }

    function parseDocumentRoute(page: string | null | undefined): CreatorDocumentRoute {
        if (page === null || page === undefined || page === 'documents') {
            return { mode: 'index', kind: null, id: null };
        }
        for (const family of CREATOR_DOCUMENT_FAMILIES) {
            const base = familyRoute(family.kind);
            if (page === base) return { mode: 'list', kind: family.kind, id: null };
            if (page === `${base}/create`) {
                return { mode: 'create', kind: family.kind, id: null };
            }
            const editPrefix = `${base}/edit/`;
            if (page.startsWith(editPrefix)) {
                const encodedId = page.slice(editPrefix.length);
                try {
                    return { mode: 'edit', kind: family.kind, id: decodeURIComponent(encodedId) };
                } catch {
                    return { mode: 'index', kind: null, id: null };
                }
            }
        }
        return { mode: 'index', kind: null, id: null };
    }

    function documentsFor(kind: CreatorDocumentKind): EditableCreatorDocument[] {
        if (kind === 'memory_profile') {
            return orchestrationState.editable_memory_profiles;
        }
        if (kind === 'knowledge_book') {
            return orchestrationState.editable_knowledge_books;
        }
        if (kind === 'transform_set') {
            return orchestrationState.editable_transform_sets;
        }
        if (kind === 'interaction_rule_set') {
            return orchestrationState.editable_interaction_rule_sets;
        }
        return orchestrationState.editable_content_modules;
    }

    function documentKey(kind: CreatorDocumentKind, document: EditableCreatorDocument): string {
        return `${kind}:${document.value.id}:${String(document.expected_revision ?? 'new')}`;
    }

    function documentHelpId(kind: CreatorDocumentKind, document: EditableCreatorDocument): string {
        return `creator-document-${encodeURIComponent(documentKey(kind, document)).replaceAll('%', '_')}-help`;
    }

    function documentJson(kind: CreatorDocumentKind, document: EditableCreatorDocument): string {
        const key = documentKey(kind, document);
        return jsonDrafts[key] ?? JSON.stringify(document.value, null, 2);
    }

    function setDocumentJson(
        kind: CreatorDocumentKind,
        document: EditableCreatorDocument,
        value: string,
    ): void {
        const key = documentKey(kind, document);
        jsonDrafts[key] = value;
        jsonErrors = Object.fromEntries(
            Object.entries(jsonErrors).filter(([candidate]) => candidate !== key),
        );
    }

    function documentJsonChanged(
        kind: CreatorDocumentKind,
        document: EditableCreatorDocument,
    ): boolean {
        const key = documentKey(kind, document);
        return (
            jsonDrafts[key] !== undefined &&
            jsonDrafts[key] !== JSON.stringify(document.value, null, 2)
        );
    }

    function isRecord(value: unknown): value is Record<string, unknown> {
        return typeof value === 'object' && value !== null && !Array.isArray(value);
    }

    function parsedCreatorDocument(
        kind: CreatorDocumentKind,
        document: EditableCreatorDocument,
    ): CreatorDocumentValue | null {
        const key = documentKey(kind, document);
        const source = documentJson(kind, document);
        if (source.length > MAX_DOCUMENT_JSON_CHARS) {
            jsonErrors[key] =
                `문서 JSON은 ${MAX_DOCUMENT_JSON_CHARS.toLocaleString()}자 이하여야 합니다.`;
            return null;
        }
        try {
            const parsed: unknown = JSON.parse(source);
            if (!isRecord(parsed)) {
                jsonErrors[key] = '문서 JSON은 객체여야 합니다.';
                return null;
            }
            if (parsed.id !== document.value.id) {
                jsonErrors[key] = '문서 ID는 생성 후 변경할 수 없습니다.';
                return null;
            }
            if (
                kind === 'content_module' &&
                (!Array.isArray(parsed.asset_ids) || parsed.asset_ids.length > 0)
            ) {
                jsonErrors[key] = '현재 안전 CRUD 경로에서는 asset_ids가 빈 배열이어야 합니다.';
                return null;
            }
            return parsed as unknown as CreatorDocumentValue;
        } catch {
            jsonErrors[key] = '유효한 JSON 문서를 입력해 주세요.';
            return null;
        }
    }

    async function saveDocument(
        kind: CreatorDocumentKind,
        document: EditableCreatorDocument,
    ): Promise<void> {
        const submittedKey = documentKey(kind, document);
        const submittedSource = documentJson(kind, document);
        const parsed = parsedCreatorDocument(kind, document);
        if (parsed === null) return;
        if (!controller.replaceCreatorDocument(kind, document.value.id, parsed)) {
            jsonErrors[submittedKey] = '편집 중인 문서를 찾지 못했습니다.';
            return;
        }
        if (await controller.saveCreatorDocument(kind, document.value.id)) {
            await tick();
            const latestSource = jsonDrafts[submittedKey];
            const hasNewerDraft = latestSource !== undefined && latestSource !== submittedSource;
            const remainingDrafts = Object.fromEntries(
                Object.entries(jsonDrafts).filter(([candidate]) => candidate !== submittedKey),
            );
            if (hasNewerDraft) {
                const currentDocument = documentsFor(kind).find(
                    (candidate) => candidate.value.id === document.value.id,
                );
                const currentKey =
                    currentDocument === undefined
                        ? submittedKey
                        : documentKey(kind, currentDocument);
                remainingDrafts[currentKey] = latestSource;
            }
            jsonDrafts = remainingDrafts;
            jsonErrors = Object.fromEntries(
                Object.entries(jsonErrors).filter(([candidate]) => candidate !== submittedKey),
            );
            detailPage = familyRoute(kind);
        }
    }

    function addDocument(kind: CreatorDocumentKind): void {
        const requestedId = newIds[kind].trim();
        if (controller.addCreatorDocumentDraft(kind, requestedId)) {
            newIds[kind] = '';
            createErrors[kind] = '';
            detailPage = editRoute(kind, requestedId);
        } else {
            createErrors[kind] =
                '비어 있지 않은 고유 ID를 입력해 주세요. ID는 256자 이하여야 합니다.';
        }
    }

    async function confirmDelete(
        kind: CreatorDocumentKind,
        document: EditableCreatorDocument,
    ): Promise<void> {
        const key = `${kind}:${document.value.id}`;
        if (pendingDeleteKey !== key) {
            pendingDeleteKey = key;
            return;
        }
        if (await controller.deleteCreatorDocument(kind, document.value.id)) {
            jsonDrafts = Object.fromEntries(
                Object.entries(jsonDrafts).filter(
                    ([candidate]) => !candidate.startsWith(`${kind}:${document.value.id}:`),
                ),
            );
            detailPage = familyRoute(kind);
        }
        pendingDeleteKey = null;
    }

    function documentType(value: CreatorDocumentValue): string {
        if ('summary_task' in value) return 'MemoryProfile';
        if ('entries' in value) return 'KnowledgeBook';
        if ('max_rules_per_phase' in value) return 'TransformSet';
        if ('max_actions_per_event' in value) return 'InteractionRuleSet';
        return 'ContentModule';
    }
</script>

{#snippet detailContent()}
    {#if orchestrationState.editable_creator_documents_loading}
        <p class="creator-note" role="status">Creator 문서를 불러오거나 저장하는 중입니다.</p>
    {/if}
    {#if orchestrationState.editable_creator_documents_error}
        <p class="creator-error" role="alert">
            {orchestrationState.editable_creator_documents_error}
        </p>
    {/if}

    {#if route.mode === 'index'}
        <div class="setting-list family-list" aria-label="Creator 문서 유형">
            {#each CREATOR_DOCUMENT_FAMILIES as family (family.kind)}
                {@const documents = documentsFor(family.kind)}
                <button
                    class="setting-row family-row"
                    type="button"
                    disabled={busy}
                    onclick={() => (detailPage = familyRoute(family.kind))}
                >
                    <span class="setting-content">
                        <span class="setting-copy creator-copy">
                            <strong>{family.title}</strong>
                            <small>{documents.length}개 · {family.guide}</small>
                        </span>
                    </span>
                </button>
            {/each}
        </div>
    {:else if route.mode === 'list' && selectedFamily !== null}
        {@const documents = documentsFor(selectedFamily.kind)}
        <div class="setting-list document-list" aria-label={`${selectedFamily.title} 목록`}>
            {#if documents.length === 0}
                <p class="creator-empty">아직 저장된 {selectedFamily.title} 문서가 없습니다.</p>
            {/if}
            {#each documents as document (document.value.id)}
                <button
                    class="setting-row document-row"
                    type="button"
                    disabled={busy}
                    onclick={() => (detailPage = editRoute(selectedFamily.kind, document.value.id))}
                >
                    <span class="setting-content">
                        <span class="setting-copy creator-copy">
                            <strong>{document.value.id}</strong>
                            <small>
                                {documentType(document.value)} ·
                                {document.expected_revision === null
                                    ? '새 문서'
                                    : `revision ${String(document.expected_revision)}`}
                                {document.dirty ||
                                documentJsonChanged(selectedFamily.kind, document)
                                    ? ' · 저장 안 됨'
                                    : ''}
                            </small>
                        </span>
                    </span>
                </button>
            {/each}
        </div>
    {:else if route.mode === 'create' && selectedFamily !== null}
        <form
            id="creator-document-create-form"
            class="creator-form creator-id-form"
            aria-label={`${selectedFamily.title} 만들기`}
            onsubmit={(event) => {
                event.preventDefault();
                addDocument(selectedFamily.kind);
            }}
        >
            <label>
                <span>{selectedFamily.create_label}</span>
                <input
                    type="text"
                    maxlength="256"
                    autocomplete="off"
                    value={newIds[selectedFamily.kind]}
                    disabled={busy}
                    oninput={(event) => (newIds[selectedFamily.kind] = event.currentTarget.value)}
                />
            </label>
            {#if createErrors[selectedFamily.kind]}
                <p class="creator-error" role="alert">{createErrors[selectedFamily.kind]}</p>
            {/if}
        </form>
    {:else if route.mode === 'edit' && selectedFamily !== null}
        {#if selectedDocument === null}
            <p class="creator-error" role="alert">편집할 문서를 찾지 못했습니다.</p>
        {:else}
            {@const key = documentKey(selectedFamily.kind, selectedDocument)}
            {@const helpId = documentHelpId(selectedFamily.kind, selectedDocument)}
            <form
                id="creator-document-editor-form"
                class="creator-form creator-json-form"
                aria-label={`${selectedFamily.title} JSON 편집`}
                onsubmit={(event) => {
                    event.preventDefault();
                    void saveDocument(selectedFamily.kind, selectedDocument);
                }}
            >
                <p class="document-meta">
                    {selectedDocument.value.id} ·
                    {selectedDocument.expected_revision === null
                        ? '새 문서'
                        : `revision ${String(selectedDocument.expected_revision)}`}
                </p>
                <label>
                    <span>안전 문서 JSON</span>
                    <textarea
                        class="json-editor"
                        rows="16"
                        spellcheck="false"
                        aria-describedby={helpId}
                        value={documentJson(selectedFamily.kind, selectedDocument)}
                        disabled={busy}
                        oninput={(event) =>
                            setDocumentJson(
                                selectedFamily.kind,
                                selectedDocument,
                                event.currentTarget.value,
                            )}></textarea>
                    <small id={helpId}>
                        최대 {MAX_DOCUMENT_JSON_CHARS.toLocaleString()}자 · 전체 typed DTO를 Core가
                        다시 검증합니다.
                    </small>
                </label>
                {#if pendingDeleteKey === `${selectedFamily.kind}:${selectedDocument.value.id}`}
                    <p class="creator-note" role="status">
                        이 문서를 삭제합니다. 하단에서 한 번 더 확인해 주세요.
                    </p>
                {/if}
                {#if jsonErrors[key]}
                    <p class="creator-error" role="alert">{jsonErrors[key]}</p>
                {/if}
            </form>
        {/if}
    {/if}
{/snippet}

{#snippet detailActions()}
    {#if route.mode === 'list' && selectedFamily !== null}
        <DetailActionBar fixed ariaLabel={`${selectedFamily.title} 작업`}>
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                disabled={busy}
                onclick={() => (detailPage = createRoute(selectedFamily.kind))}
            >
                <Plus class="creator-add-icon" aria-hidden="true" />
                문서 추가하기
            </button>
        </DetailActionBar>
    {:else if route.mode === 'create' && selectedFamily !== null}
        <DetailActionBar fixed ariaLabel={`${selectedFamily.title} 만들기 작업`}>
            <button
                class="primary detail-action detail-action--wide"
                type="submit"
                form="creator-document-create-form"
                disabled={busy || newIds[selectedFamily.kind].trim() === ''}
            >
                문서 만들기
            </button>
        </DetailActionBar>
    {:else if route.mode === 'edit' && selectedFamily !== null && selectedDocument !== null}
        <DetailActionBar fixed ariaLabel={`${selectedFamily.title} 편집 작업`}>
            {#if pendingDeleteKey === `${selectedFamily.kind}:${selectedDocument.value.id}`}
                <button
                    class="danger detail-action detail-action--destructive"
                    type="button"
                    disabled={busy}
                    onclick={() => void confirmDelete(selectedFamily.kind, selectedDocument)}
                >
                    삭제 확인
                </button>
                <button
                    class="detail-action detail-action--grow"
                    type="button"
                    disabled={busy}
                    onclick={() => (pendingDeleteKey = null)}
                >
                    취소
                </button>
            {:else}
                <button
                    class="detail-action detail-action--destructive detail-action--borderless"
                    type="button"
                    disabled={busy}
                    onclick={() => void confirmDelete(selectedFamily.kind, selectedDocument)}
                >
                    삭제
                </button>
                <button
                    class="primary detail-action detail-action--grow"
                    type="submit"
                    form="creator-document-editor-form"
                    disabled={busy ||
                        (!selectedDocument.dirty &&
                            !documentJsonChanged(selectedFamily.kind, selectedDocument))}
                >
                    저장
                </button>
            {/if}
        </DetailActionBar>
    {/if}
{/snippet}

<section class="creator-editors" aria-label="Creator 문서">
    {@render detailContent()}
    {@render detailActions()}
</section>

<style>
    .creator-editors {
        display: grid;
        min-width: 0;
        gap: 18px;
    }

    .family-list,
    .document-list {
        width: 100%;
        margin: 0;
    }

    .creator-copy {
        display: grid;
        min-width: 0;
        gap: 5px;
    }

    .creator-copy strong,
    .creator-copy small {
        overflow: hidden;
        font-size: var(--detail-support-type);
        font-weight: 550;
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .creator-copy strong {
        color: var(--ink);
        white-space: nowrap;
    }

    .creator-copy small {
        display: -webkit-box;
        color: var(--ink-muted);
        overflow-wrap: anywhere;
        white-space: normal;
        line-clamp: 3;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 3;
    }

    :global(.creator-add-icon) {
        width: 20px;
        height: 20px;
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 1.8;
    }

    .creator-empty,
    .creator-note,
    .creator-error {
        padding: 12px;
        border-radius: var(--radius-md);
        margin: 0;
        background: var(--surface-sunken);
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        line-height: 1.5;
    }

    .creator-empty {
        margin: 0;
    }

    .creator-error {
        color: var(--danger);
        background: var(--danger-soft);
    }

    .creator-form {
        display: grid;
        gap: 14px;
    }

    .creator-form label {
        display: grid;
        gap: 7px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .creator-form :is(input, textarea) {
        width: 100%;
        min-width: 0;
        box-sizing: border-box;
        padding: clamp(12px, 3.432vw, 15px);
        border: 1.5px solid var(--line);
        border-radius: var(--radius-md);
        -webkit-appearance: none;
        appearance: none;
        background: color-mix(in srgb, var(--surface-sunken) 26%, var(--surface-raised));
        box-shadow: inset 0 1px 2px rgb(16 18 24 / 3%);
        caret-color: var(--accent);
        color: var(--ink);
        font: inherit;
        font-size: var(--detail-support-type);
        line-height: 1.5;
        transition:
            background-color 140ms ease,
            box-shadow 140ms ease;
    }

    .creator-form input {
        min-height: clamp(48px, 13.73vw, 60px);
    }

    .creator-form textarea {
        min-height: min(56vh, 520px);
        resize: vertical;
    }

    .creator-form .json-editor {
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
        font-size: 0.8rem;
    }

    .creator-form :is(input, textarea):hover:not(:focus, :disabled) {
        border-color: var(--line);
    }

    .creator-form :is(input, textarea):focus {
        border-color: var(--accent);
        outline: none;
    }

    .creator-form :is(input, textarea):disabled {
        cursor: not-allowed;
        opacity: 0.55;
    }

    .creator-form small,
    .document-meta {
        margin: 0;
        color: var(--ink-muted);
        overflow-wrap: anywhere;
        font-size: 0.8em;
        font-weight: 500;
        line-height: 1.5;
    }

    @container view (min-width: 701px) {
        .creator-id-form {
            grid-template-columns: repeat(2, minmax(0, 1fr));
        }

        .creator-id-form > .creator-error {
            grid-column: 1 / -1;
        }
    }
</style>

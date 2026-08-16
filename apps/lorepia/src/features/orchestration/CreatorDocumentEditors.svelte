<script lang="ts">
    import { tick } from 'svelte';

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
    }

    interface CreatorDocumentFamily {
        kind: CreatorDocumentKind;
        title: string;
        create_label: string;
        guide: string;
    }

    type EditableCreatorDocument = EditableCreatorDocumentState<CreatorDocumentValue>;

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

    let { orchestrationState, controller }: Props = $props();
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

    $effect(() => {
        if (draftContextKey === orchestrationState.context_key) return;
        draftContextKey = orchestrationState.context_key;
        jsonDrafts = {};
        jsonErrors = {};
        createErrors = {};
        pendingDeleteKey = null;
    });

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
        }
    }

    function addDocument(kind: CreatorDocumentKind): void {
        const requestedId = newIds[kind];
        if (controller.addCreatorDocumentDraft(kind, requestedId)) {
            newIds[kind] = '';
            createErrors[kind] = '';
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

<section class="creator-editors" aria-labelledby="creator-documents-title">
    <div class="creator-heading">
        <div>
            <h3 id="creator-documents-title">Creator 문서 라이브 편집</h3>
            <p>
                사용자 소유의 안전 DTO만 편집합니다. 저장과 삭제는 표시된 정확한 revision으로 Core가
                검증합니다.
            </p>
        </div>
        <span class="creator-count">
            {CREATOR_DOCUMENT_FAMILIES.reduce(
                (count, family) => count + documentsFor(family.kind).length,
                0,
            )}개
        </span>
    </div>

    <p class="safety-note">
        schema_version, provenance, imported 활성화 플래그는 이 경계에 존재하지 않습니다. 알 수 없는
        필드와 잘못된 nested union은 Rust가 거부하며, 실패한 저장은 성공으로 표시하지 않습니다.
    </p>

    {#if orchestrationState.editable_creator_documents_loading}
        <p class="loading-note" role="status">Creator 문서를 불러오거나 저장하는 중입니다.</p>
    {/if}
    {#if orchestrationState.editable_creator_documents_error}
        <p class="creator-error" role="alert">
            {orchestrationState.editable_creator_documents_error}
        </p>
    {/if}

    <div class="family-list">
        {#each CREATOR_DOCUMENT_FAMILIES as family (family.kind)}
            {@const documents = documentsFor(family.kind)}
            <details class="family-card" open={documents.length > 0}>
                <summary>
                    <span>{family.title}</span>
                    <span>{documents.length}개</span>
                </summary>
                <div class="family-body">
                    <p>{family.guide}</p>
                    <div class="create-row">
                        <label>
                            <span>{family.create_label}</span>
                            <input
                                type="text"
                                maxlength="256"
                                value={newIds[family.kind]}
                                oninput={(event) =>
                                    (newIds[family.kind] = event.currentTarget.value)}
                            />
                        </label>
                        <button
                            type="button"
                            disabled={newIds[family.kind].trim() === '' ||
                                orchestrationState.editable_creator_documents_loading}
                            onclick={() => addDocument(family.kind)}
                        >
                            새 문서
                        </button>
                    </div>
                    {#if createErrors[family.kind]}
                        <p class="creator-error" role="alert">{createErrors[family.kind]}</p>
                    {/if}

                    {#if documents.length === 0}
                        <p class="empty-note">편집 가능한 {family.title} 문서가 없습니다.</p>
                    {:else}
                        <ul class="document-list">
                            {#each documents as document (document.value.id)}
                                {@const key = documentKey(family.kind, document)}
                                {@const helpId = documentHelpId(family.kind, document)}
                                <li>
                                    <details class="document-card">
                                        <summary>
                                            <span>
                                                <strong>{document.value.id}</strong>
                                                <small>{documentType(document.value)}</small>
                                            </span>
                                            <span>
                                                {document.expected_revision === null
                                                    ? '새 문서'
                                                    : `revision ${String(document.expected_revision)}`}
                                                {document.dirty ||
                                                documentJsonChanged(family.kind, document)
                                                    ? ' · 저장 안 됨'
                                                    : ''}
                                            </span>
                                        </summary>
                                        <div class="document-body">
                                            <label>
                                                <span>안전 문서 JSON</span>
                                                <textarea
                                                    rows="16"
                                                    spellcheck="false"
                                                    aria-describedby={helpId}
                                                    value={documentJson(family.kind, document)}
                                                    oninput={(event) =>
                                                        setDocumentJson(
                                                            family.kind,
                                                            document,
                                                            event.currentTarget.value,
                                                        )}></textarea>
                                            </label>
                                            <small id={helpId}>
                                                최대 {MAX_DOCUMENT_JSON_CHARS.toLocaleString()}자 ·
                                                전체 typed DTO를 Core가 다시 검증합니다.
                                            </small>
                                            {#if jsonErrors[key]}
                                                <p class="creator-error" role="alert">
                                                    {jsonErrors[key]}
                                                </p>
                                            {/if}
                                            <div class="document-actions">
                                                <button
                                                    type="button"
                                                    disabled={(!document.dirty &&
                                                        !documentJsonChanged(
                                                            family.kind,
                                                            document,
                                                        )) ||
                                                        orchestrationState.editable_creator_documents_loading}
                                                    onclick={() =>
                                                        void saveDocument(family.kind, document)}
                                                >
                                                    Core 검증 후 저장
                                                </button>
                                                <button
                                                    class="danger"
                                                    type="button"
                                                    aria-pressed={pendingDeleteKey ===
                                                        `${family.kind}:${document.value.id}`}
                                                    disabled={orchestrationState.editable_creator_documents_loading}
                                                    onclick={() =>
                                                        void confirmDelete(family.kind, document)}
                                                >
                                                    {pendingDeleteKey ===
                                                    `${family.kind}:${document.value.id}`
                                                        ? document.expected_revision === null
                                                            ? '초안 버리기 확인'
                                                            : '삭제 확인'
                                                        : document.expected_revision === null
                                                          ? '초안 버리기'
                                                          : '삭제'}
                                                </button>
                                                {#if pendingDeleteKey === `${family.kind}:${document.value.id}`}
                                                    <button
                                                        type="button"
                                                        onclick={() => (pendingDeleteKey = null)}
                                                    >
                                                        취소
                                                    </button>
                                                {/if}
                                            </div>
                                        </div>
                                    </details>
                                </li>
                            {/each}
                        </ul>
                    {/if}
                </div>
            </details>
        {/each}
    </div>
</section>

<style>
    .creator-editors {
        display: grid;
        gap: 14px;
        padding: 18px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
    }

    .creator-heading,
    .create-row,
    .document-actions,
    .family-card > summary,
    .document-card > summary {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
    }

    .creator-heading h3,
    .creator-heading p,
    .family-body > p {
        margin: 0;
    }

    .creator-heading p,
    .family-body > p,
    .safety-note,
    .loading-note,
    .empty-note,
    .document-body small,
    .document-card summary small {
        color: var(--ink-muted);
    }

    .creator-count {
        flex: none;
        padding: 4px 9px;
        border: 1px solid var(--line);
        border-radius: 999px;
        font-size: 0.78rem;
    }

    .safety-note,
    .loading-note,
    .creator-error,
    .empty-note {
        margin: 0;
        padding: 10px 12px;
        border-radius: var(--radius-sm);
        background: var(--surface);
    }

    .creator-error {
        color: var(--danger);
        border: 1px solid color-mix(in srgb, var(--danger) 35%, transparent);
    }

    .family-list,
    .family-body,
    .document-list,
    .document-body {
        display: grid;
        gap: 12px;
    }

    .family-card,
    .document-card {
        border: 1px solid var(--line);
        border-radius: var(--radius-sm);
        background: var(--surface);
    }

    .family-card > summary,
    .document-card > summary {
        cursor: pointer;
        padding: 12px;
    }

    .family-body,
    .document-body {
        padding: 0 12px 12px;
    }

    .create-row {
        align-items: end;
        justify-content: flex-start;
    }

    .create-row label,
    .document-body label {
        display: grid;
        flex: 1;
        gap: 6px;
    }

    .create-row input,
    .document-body textarea {
        width: 100%;
        box-sizing: border-box;
    }

    .document-list {
        margin: 0;
        padding: 0;
        list-style: none;
    }

    .document-card > summary > span {
        display: grid;
        gap: 2px;
    }

    .document-card > summary > span:last-child {
        text-align: right;
        color: var(--ink-muted);
        font-size: 0.78rem;
    }

    .document-body textarea {
        resize: vertical;
        min-height: 220px;
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
        font-size: 0.78rem;
        line-height: 1.45;
    }

    .document-actions {
        justify-content: flex-start;
        flex-wrap: wrap;
    }

    .danger {
        color: var(--danger);
    }

    @media (max-width: 720px) {
        .creator-heading,
        .create-row {
            align-items: stretch;
            flex-direction: column;
        }

        .creator-count {
            align-self: flex-start;
        }
    }
</style>

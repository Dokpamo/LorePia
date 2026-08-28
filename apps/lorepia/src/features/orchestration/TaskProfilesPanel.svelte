<script lang="ts">
    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import ChoiceField from '../../components/ChoiceField.svelte';
    import type { LorepiaAppState } from '../../app/app-controller';
    import { tr } from '../../lib/i18n';
    import {
        taskProfileValidationError,
        type OrchestrationController,
        type OrchestrationState,
    } from './orchestration-controller';

    interface Props {
        appState: LorepiaAppState;
        orchestrationState: OrchestrationState;
        controller: OrchestrationController;
        detailPage?: string | null;
    }

    const EDIT_PREFIX = 'profiles/edit/';

    let {
        appState,
        orchestrationState,
        controller,
        detailPage = $bindable('profiles'),
    }: Props = $props();
    let newTaskProfileId = $state('');
    let pendingDeleteId = $state<string | null>(null);
    let createError = $state('');

    const busy = $derived(orchestrationState.editable_task_profiles_loading);
    const selectedProfileId = $derived.by(() => {
        if (!detailPage?.startsWith(EDIT_PREFIX)) return null;
        try {
            return decodeURIComponent(detailPage.slice(EDIT_PREFIX.length));
        } catch {
            return null;
        }
    });
    const selectedProfile = $derived(
        selectedProfileId === null
            ? null
            : (orchestrationState.editable_task_profiles.find(
                  (profile) => profile.value.id === selectedProfileId,
              ) ?? null),
    );
    const selectedValidationError = $derived(
        selectedProfile === null ? null : taskProfileValidationError(selectedProfile.value),
    );

    function editRoute(profileId: string): string {
        return `${EDIT_PREFIX}${encodeURIComponent(profileId)}`;
    }

    function openCreate(): void {
        newTaskProfileId = '';
        createError = '';
        pendingDeleteId = null;
        detailPage = 'profiles/create';
    }

    function openEdit(profileId: string): void {
        pendingDeleteId = null;
        detailPage = editRoute(profileId);
    }

    function createProfile(): void {
        const id = newTaskProfileId.trim();
        if (!controller.addTaskProfileDraft(id)) {
            createError = $tr('orchestration.error.task_profile_create');
            return;
        }
        newTaskProfileId = '';
        createError = '';
        detailPage = editRoute(id);
    }

    async function saveProfile(): Promise<void> {
        if (selectedProfile === null) return;
        if (await controller.saveTaskProfile(selectedProfile.value.id)) {
            detailPage = 'profiles';
        }
    }

    async function deleteProfile(): Promise<void> {
        if (selectedProfile === null) return;
        const id = selectedProfile.value.id;
        if (pendingDeleteId !== id) {
            pendingDeleteId = id;
            return;
        }
        if (await controller.deleteTaskProfile(id)) {
            detailPage = 'profiles';
        }
        pendingDeleteId = null;
    }
</script>

<section class="profile-panel" aria-label="생성·작업 프로필">
    {#if orchestrationState.editable_task_profiles_error}
        <p class="profile-feedback error" role="alert">
            {orchestrationState.editable_task_profiles_error}
        </p>
    {:else if busy}
        <p class="profile-feedback" role="status">작업 프로필을 불러오거나 저장하는 중입니다.</p>
    {/if}

    {#if detailPage === 'profiles'}
        <section class="profile-group" aria-labelledby="generation-presets-label">
            <h3 id="generation-presets-label">생성 프리셋</h3>
            {#if appState.providers.workspace.presets.length === 0}
                <p class="profile-feedback">로드된 생성 프리셋이 없습니다.</p>
            {:else}
                <div class="setting-list profile-list" aria-label="생성 프리셋 목록">
                    {#each appState.providers.workspace.presets.slice(0, 100) as preset (preset.id)}
                        <div class="setting-row profile-readonly-row">
                            <span class="setting-content">
                                <span class="setting-copy profile-copy">
                                    <strong>{preset.display_name}</strong>
                                    <small
                                        >{preset.reasoning.mode} · {preset.prompt_cache.mode}</small
                                    >
                                </span>
                            </span>
                        </div>
                    {/each}
                </div>
            {/if}
        </section>

        <section class="profile-group" aria-labelledby="task-profiles-label">
            <h3 id="task-profiles-label">보조 작업 프로필</h3>
            {#if orchestrationState.editable_task_profiles.length === 0 && !busy}
                <p class="profile-feedback">편집 가능한 보조 작업 프로필이 없습니다.</p>
            {:else}
                <div class="setting-list profile-list" aria-label="보조 작업 프로필 목록">
                    {#each orchestrationState.editable_task_profiles.slice(0, 100) as profile (profile.value.id)}
                        <button
                            class="setting-row profile-row"
                            type="button"
                            disabled={busy}
                            onclick={() => openEdit(profile.value.id)}
                        >
                            <span class="setting-content">
                                <span class="setting-copy profile-copy">
                                    <strong>{profile.value.id}</strong>
                                    <small>
                                        {profile.value.kind}{#if profile.dirty}
                                            · 저장 안 됨{/if}
                                    </small>
                                </span>
                            </span>
                        </button>
                    {/each}
                </div>
                {#if orchestrationState.editable_task_profiles.length > 100}
                    <p class="profile-feedback">처음 100개 작업 프로필만 표시합니다.</p>
                {/if}
            {/if}
        </section>

        <DetailActionBar fixed ariaLabel="작업 프로필 목록 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                disabled={busy}
                onclick={openCreate}
            >
                프로필 추가
            </button>
        </DetailActionBar>
    {:else if detailPage === 'profiles/create'}
        <form
            id="task-profile-create-form"
            class="profile-form"
            aria-label="작업 프로필 추가"
            onsubmit={(event) => {
                event.preventDefault();
                createProfile();
            }}
        >
            <label>
                <span>프로필 ID</span>
                <input
                    type="text"
                    maxlength="256"
                    autocomplete="off"
                    bind:value={newTaskProfileId}
                    disabled={busy}
                />
            </label>
            {#if createError !== ''}
                <p class="profile-feedback error" role="alert">{createError}</p>
            {/if}
        </form>

        <DetailActionBar fixed ariaLabel="작업 프로필 추가 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="submit"
                form="task-profile-create-form"
                disabled={busy || newTaskProfileId.trim() === ''}
            >
                추가
            </button>
        </DetailActionBar>
    {:else if selectedProfile !== null}
        {@const profile = selectedProfile}
        <form
            id="task-profile-editor-form"
            class="profile-form"
            aria-label="작업 프로필 편집"
            onsubmit={(event) => {
                event.preventDefault();
                void saveProfile();
            }}
        >
            <label>
                <span>프로필 ID</span>
                <input type="text" readonly value={profile.value.id} />
            </label>
            <ChoiceField
                id={`task-profile-kind-${profile.value.id}`}
                label="작업 종류"
                value={profile.value.kind}
                options={[
                    { value: 'memory_summary', label: 'memory summary' },
                    { value: 'memory_embedding', label: 'memory embedding' },
                    { value: 'translation', label: 'translation' },
                    { value: 'emotion_classification', label: 'emotion classification' },
                    { value: 'state_extraction', label: 'state extraction' },
                    { value: 'image_prompt', label: 'image prompt' },
                    { value: 'title_generation', label: 'title generation' },
                ]}
                disabled={busy}
                onSelect={(value) =>
                    controller.stageTaskProfile(profile.value.id, {
                        kind: value as typeof profile.value.kind,
                    })}
            />
            <label>
                <span>모델 route ID</span>
                <input
                    type="text"
                    maxlength="256"
                    value={profile.value.route_id}
                    disabled={busy}
                    oninput={(event) =>
                        controller.stageTaskProfile(profile.value.id, {
                            route_id: event.currentTarget.value,
                        })}
                />
            </label>
            <label>
                <span>생성 프리셋 ID</span>
                <input
                    type="text"
                    maxlength="256"
                    value={profile.value.generation_preset_id}
                    disabled={busy}
                    oninput={(event) =>
                        controller.stageTaskProfile(profile.value.id, {
                            generation_preset_id: event.currentTarget.value,
                        })}
                />
            </label>
            {#if profile.value.kind === 'memory_embedding'}
                <label>
                    <span>임베딩 차원</span>
                    <input
                        type="number"
                        min="1"
                        max="32768"
                        step="1"
                        required
                        value={profile.value.embedding_dimensions ?? ''}
                        disabled={busy}
                        oninput={(event) =>
                            controller.stageTaskProfile(profile.value.id, {
                                embedding_dimensions:
                                    event.currentTarget.value === ''
                                        ? null
                                        : Number(event.currentTarget.value),
                            })}
                    />
                </label>
            {:else}
                <label>
                    <span>Fallback route IDs (쉼표 구분)</span>
                    <input
                        type="text"
                        maxlength="4096"
                        value={profile.value.fallback_route_ids.join(', ')}
                        disabled={busy}
                        oninput={(event) =>
                            controller.stageTaskProfile(profile.value.id, {
                                fallback_route_ids: event.currentTarget.value
                                    .split(',')
                                    .map((value) => value.trim())
                                    .filter(Boolean),
                            })}
                    />
                </label>
            {/if}
            <label>
                <span>Timeout (ms)</span>
                <input
                    type="number"
                    min="1"
                    value={profile.value.timeout_ms}
                    disabled={busy}
                    oninput={(event) =>
                        controller.stageTaskProfile(profile.value.id, {
                            timeout_ms: Number(event.currentTarget.value),
                        })}
                />
            </label>
            <label>
                <span>Rate requests</span>
                <input
                    type="number"
                    min="1"
                    value={profile.value.rate_limit.requests}
                    disabled={busy}
                    oninput={(event) =>
                        controller.stageTaskProfile(profile.value.id, {
                            rate_limit: {
                                ...profile.value.rate_limit,
                                requests: Number(event.currentTarget.value),
                            },
                        })}
                />
            </label>
            <label>
                <span>Rate period (seconds)</span>
                <input
                    type="number"
                    min="1"
                    value={profile.value.rate_limit.per_seconds}
                    disabled={busy}
                    oninput={(event) =>
                        controller.stageTaskProfile(profile.value.id, {
                            rate_limit: {
                                ...profile.value.rate_limit,
                                per_seconds: Number(event.currentTarget.value),
                            },
                        })}
                />
            </label>
            <label>
                <span>동시 실행 제한</span>
                <input
                    type="number"
                    min="1"
                    value={profile.value.concurrency_limit}
                    disabled={busy}
                    oninput={(event) =>
                        controller.stageTaskProfile(profile.value.id, {
                            concurrency_limit: Number(event.currentTarget.value),
                        })}
                />
            </label>
            {#if selectedValidationError}
                <p class="profile-feedback error" role="alert">{selectedValidationError}</p>
            {/if}
        </form>

        <DetailActionBar fixed ariaLabel="작업 프로필 편집 작업">
            {#if pendingDeleteId === profile.value.id}
                <button
                    class="danger detail-action detail-action--destructive"
                    type="button"
                    disabled={busy}
                    onclick={() => void deleteProfile()}
                >
                    삭제 확인
                </button>
                <button
                    class="detail-action detail-action--grow"
                    type="button"
                    disabled={busy}
                    onclick={() => (pendingDeleteId = null)}
                >
                    취소
                </button>
            {:else}
                <button
                    class="detail-action detail-action--destructive detail-action--borderless"
                    type="button"
                    disabled={busy}
                    onclick={() => (pendingDeleteId = profile.value.id)}
                >
                    {#if profile.expected_revision === null}초안 삭제{:else}삭제{/if}
                </button>
                <button
                    class="primary detail-action detail-action--grow"
                    type="submit"
                    form="task-profile-editor-form"
                    disabled={busy ||
                        !profile.dirty ||
                        profile.value.route_id.trim() === '' ||
                        profile.value.generation_preset_id.trim() === '' ||
                        selectedValidationError !== null}
                >
                    저장
                </button>
            {/if}
        </DetailActionBar>
    {:else}
        <p class="profile-feedback error" role="alert">작업 프로필을 찾을 수 없습니다.</p>
    {/if}
</section>

<style>
    .profile-panel {
        display: grid;
        gap: 18px;
    }

    .profile-group {
        display: grid;
        gap: 8px;
    }

    .profile-group > h3 {
        margin: 0 3px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .profile-copy {
        min-width: 0;
        flex-direction: column;
        gap: 5px;
    }

    .profile-copy small {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        line-height: 1.35;
        overflow-wrap: anywhere;
    }

    .profile-readonly-row {
        cursor: default;
    }

    .profile-feedback {
        padding: 12px;
        border-radius: 12px;
        margin: 0;
        color: var(--ink-muted);
        background: var(--surface-sunken);
        line-height: 1.5;
    }

    .profile-feedback.error {
        border: 1px solid var(--status-error-border);
        color: var(--status-error-fg);
        background: var(--status-error-bg);
    }

    .profile-form {
        display: grid;
        gap: 14px;
    }

    .profile-form label {
        display: grid;
        gap: 7px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .profile-form input {
        width: 100%;
        min-width: 0;
        min-height: clamp(48px, 13.73vw, 60px);
        box-sizing: border-box;
        padding: clamp(12px, 3.432vw, 15px);
        border: 1.5px solid var(--line);
        border-radius: var(--radius-md);
        -webkit-appearance: none;
        appearance: none;
        color: var(--ink);
        background: color-mix(in srgb, var(--surface-sunken) 26%, var(--surface-raised));
        box-shadow: var(--control-inset-shadow);
        caret-color: var(--accent);
        font-size: var(--detail-support-type);
        line-height: 1.5;
    }

    .profile-form input:hover:not(:focus, :disabled) {
        border-color: var(--line);
    }

    .profile-form input:focus {
        border-color: var(--accent);
        outline: none;
    }

    .profile-form input:disabled {
        cursor: not-allowed;
        opacity: var(--disabled-opacity);
    }
</style>

<script lang="ts">
    import ChoiceField from '../../../components/ChoiceField.svelte';
    import ToggleSwitch from '../../../components/ToggleSwitch.svelte';
    import type { LorepiaAppState } from '../../../app/app-controller';
    import type { GenerationPresetDto, ModelRouteDto } from '../../../lib/ipc/contracts';

    interface Props {
        appState: LorepiaAppState;
        settingsBusy: boolean;
        selectedRouteId: string;
        selectedPresetId: string;
        preservePartialGenerations: boolean;
        selectableRoutes: ModelRouteDto[];
        selectedRoutePresets: GenerationPresetDto[];
        preview: boolean;
        onChangeRoute: (routeId: string) => void;
        onSelectPreset: (presetId: string) => void;
        onPreservePartialChange: (preserve: boolean) => void;
        openTargetPreview: () => void | Promise<void>;
    }

    let {
        appState,
        settingsBusy,
        selectedRouteId,
        selectedPresetId,
        preservePartialGenerations,
        selectableRoutes,
        selectedRoutePresets,
        preview,
        onChangeRoute,
        onSelectPreset,
        onPreservePartialChange,
        openTargetPreview,
    }: Props = $props();

    const workspace = $derived(appState.providers.workspace);
</script>

{#if preview}
    <section class="detail-read-page" aria-label="민감값이 제거된 요청 구조">
        {#if workspace.request_preview}
            <dl class="detail-value-list">
                <div>
                    <dt>Method</dt>
                    <dd>{workspace.request_preview.method}</dd>
                </div>
                <div>
                    <dt>Origin</dt>
                    <dd>{workspace.request_preview.origin}</dd>
                </div>
                <div>
                    <dt>Path</dt>
                    <dd>{workspace.request_preview.path}</dd>
                </div>
                <div>
                    <dt>Headers</dt>
                    <!-- prettier-ignore -->
                    <dd>
                        {workspace.request_preview.header_names.join(', ') ||
                            '없음'}
                    </dd>
                </div>
            </dl>
            <!-- prettier-ignore -->
            <p class="inline-note">
                메시지 본문과 자격증명 값은 이 미리보기에 포함되지 않습니다.
            </p>
        {:else}
            <p class="inline-note">표시할 요청 구조가 없습니다.</p>
        {/if}
    </section>
{:else}
    <section class="detail-form-page target-page" aria-label="기본 생성 대상 편집">
        {#if workspace.settings.selected_provider_profile_id !== null}
            <!-- prettier-ignore -->
            <p class="inline-note">
                기존 프로바이더 프로필을 기본 대상으로 사용 중입니다.
            </p>
        {:else if workspace.settings.selected_model_route_id === null}
            <p class="inline-note warning">저장된 기본 생성 대상이 없습니다.</p>
        {/if}

        <div class="target-form detail-form">
            <ChoiceField
                id="default-model-route"
                label="모델 라우트"
                value={selectedRouteId}
                options={[
                    { value: '', label: '선택 안 함' },
                    ...selectableRoutes.map((route) => ({
                        value: route.id,
                        label: route.display_name ?? route.model_id,
                    })),
                ]}
                disabled={settingsBusy}
                onSelect={onChangeRoute}
            />
            <ChoiceField
                id="default-generation-preset"
                label="생성 프리셋"
                value={selectedPresetId}
                options={[
                    { value: '', label: '선택 안 함' },
                    ...selectedRoutePresets.map((preset) => ({
                        value: preset.id,
                        label: preset.display_name,
                    })),
                ]}
                disabled={settingsBusy || selectedRouteId === ''}
                onSelect={onSelectPreset}
            />
        </div>

        <div class="settings-control-row">
            <span class="settings-control-copy">
                <strong>부분 응답 보존</strong>
                <small>취소·오류 시 생성된 일부 응답을 보존</small>
            </span>
            <ToggleSwitch
                label="취소·오류 시 생성된 일부 응답을 보존"
                checked={preservePartialGenerations}
                disabled={settingsBusy}
                onChange={onPreservePartialChange}
            />
        </div>

        <button
            class="detail-secondary-action"
            type="button"
            disabled={settingsBusy ||
                workspace.settings.selected_provider_profile_id !== null ||
                workspace.settings.selected_model_route_id === null ||
                selectedRouteId !== workspace.settings.selected_model_route_id ||
                selectedPresetId !== workspace.settings.selected_generation_preset_id}
            onclick={() => void openTargetPreview()}>요청 구조 미리보기</button
        >
    </section>
{/if}

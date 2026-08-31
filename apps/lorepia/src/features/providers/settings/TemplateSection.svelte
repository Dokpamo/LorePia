<script lang="ts">
    import type { LorepiaAppState } from '../../../app/app-controller';
    import type {
        AuthBindingDto,
        ParameterLiteralDto,
        ProviderTemplateDto,
    } from '../../../lib/ipc/contracts';

    interface Props {
        appState: LorepiaAppState;
        detailPage: string | null;
        onOpenDetailPage: (page: string, title?: string) => void;
    }

    let { appState, detailPage, onOpenDetailPage }: Props = $props();

    function selectedTemplate(): ProviderTemplateDto | undefined {
        if (!detailPage?.startsWith('template:')) return undefined;
        return appState.providers.workspace.templates.find(
            (template) => template.id === detailPage.slice('template:'.length),
        );
    }

    function authBindingLabel(binding: AuthBindingDto): string {
        if (binding.kind === 'none') return '없음';
        if (binding.kind === 'bearer_header') return 'Bearer 인증 헤더';
        return `${binding.header_name} 헤더 API 키`;
    }

    function formatParameterLiteral(literal: ParameterLiteralDto): string {
        if (literal.type === 'string_list' || literal.type === 'stop_sequence_list') {
            return literal.value.join(', ');
        }
        return String(literal.value);
    }

    const workspace = $derived(appState.providers.workspace);
    const selectedTemplateValue = $derived(selectedTemplate());
</script>

{#if selectedTemplateValue}
    {@const template = selectedTemplateValue}
    <section
        class="detail-read-page template-detail"
        aria-label={`${template.display_name} 템플릿 정보`}
    >
        <dl class="detail-value-list" aria-label="템플릿 기본 정보">
            <div>
                <dt>템플릿 ID</dt>
                <dd>{template.id}</dd>
            </div>
            <div>
                <dt>원본</dt>
                <dd>{template.source}</dd>
            </div>
            <div>
                <dt>API 패밀리</dt>
                <dd>{template.api_family}</dd>
            </div>
            <div>
                <dt>매니페스트</dt>
                <dd>v{template.manifest_version}</dd>
            </div>
            <div>
                <dt>기본 API Origin</dt>
                <dd>{template.default_api_origin ?? '사용자가 입력'}</dd>
            </div>
            <div>
                <dt>기본 네트워크</dt>
                <dd>{template.default_network_mode}</dd>
            </div>
            <div>
                <dt>인증 방식</dt>
                <dd>{authBindingLabel(template.auth_binding)}</dd>
            </div>
            <div>
                <dt>자격증명</dt>
                <dd>{template.credential_required ? '필요' : '필요 없음'}</dd>
            </div>
            <div>
                <dt>모델 목록</dt>
                <!-- prettier-ignore -->
                <dd>
                    {template.supports_model_listing ? '지원' : '지원 안 함'}
                </dd>
            </div>
        </dl>

        <section class="detail-subsection" aria-labelledby="template-fields-title">
            <h3 id="template-fields-title">연결 필드</h3>
            {#if template.connection_fields.length === 0}
                <p class="inline-note">추가 연결 필드가 없습니다.</p>
            {:else}
                <dl class="detail-value-list template-spec-list">
                    {#each template.connection_fields as field (field.key)}
                        <div>
                            <dt>{field.label_key}</dt>
                            <dd>
                                <span
                                    >{field.key} · {field.value_type} · {field.required
                                        ? '필수'
                                        : '선택'}</span
                                >
                                {#if field.description_key}
                                    <small>{field.description_key}</small>
                                {/if}
                            </dd>
                        </div>
                    {/each}
                </dl>
            {/if}
        </section>

        <section class="detail-subsection" aria-labelledby="template-parameters-title">
            <h3 id="template-parameters-title">생성 파라미터</h3>
            {#if template.parameters.length === 0}
                <p class="inline-note">정의된 생성 파라미터가 없습니다.</p>
            {:else}
                <dl class="detail-value-list template-spec-list">
                    {#each template.parameters as parameter (parameter.id)}
                        <div>
                            <dt>{parameter.label_key}</dt>
                            <dd>
                                <span
                                    >{parameter.id} · {parameter.value_type} · {parameter.level}</span
                                >
                                <small
                                    >기본 {parameter.default_mode} · 전달 {parameter
                                        .provider_mapping.target}:{parameter.provider_mapping
                                        .field_name}</small
                                >
                                {#if parameter.allowed_values.length > 0}
                                    <small
                                        >허용값: {parameter.allowed_values
                                            .map((choice) => formatParameterLiteral(choice.value))
                                            .join(', ')}</small
                                    >
                                {/if}
                                {#if parameter.minimum !== null || parameter.maximum !== null || parameter.step !== null}
                                    <!-- prettier-ignore -->
                                    <small
                                        >범위 {parameter.minimum ??
                                            '제한 없음'}–{parameter.maximum ??
                                            '제한 없음'} · 단계 {parameter.step ??
                                            '기본값'}</small
                                    >
                                {/if}
                                {#if parameter.visibility}
                                    <!-- prettier-ignore -->
                                    <small
                                        >표시 조건: {parameter.visibility
                                            .parameter_id}
                                        {parameter.visibility.operator}
                                        {formatParameterLiteral(
                                            parameter.visibility.value,
                                        )}</small
                                    >
                                {/if}
                                {#if parameter.conflicts.length > 0}
                                    <!-- prettier-ignore -->
                                    <small
                                        >충돌 규칙 {parameter.conflicts
                                            .length}개</small
                                    >
                                {/if}
                            </dd>
                        </div>
                    {/each}
                </dl>
            {/if}
        </section>
    </section>
{:else if detailPage?.startsWith('template:')}
    <p class="inline-note">선택한 템플릿을 찾을 수 없습니다.</p>
{:else if workspace.templates.length === 0}
    <p class="inline-note">현재 사용할 수 있는 템플릿이 없습니다.</p>
{:else}
    <ul class="setting-list detail-record-list" aria-label="템플릿 목록">
        {#each workspace.templates as template (template.id)}
            <li>
                <button
                    class="setting-row detail-record-row"
                    type="button"
                    onclick={() =>
                        onOpenDetailPage(`template:${template.id}`, template.display_name)}
                >
                    <span class="setting-content">
                        <span class="setting-copy detail-row-copy">
                            <strong>{template.display_name}</strong>
                            <small>{template.api_family} · v{template.manifest_version}</small>
                        </span>
                        <span class="setting-value"
                            >필드 {template.connection_fields.length} · 파라미터 {template
                                .parameters.length}</span
                        >
                    </span>
                </button>
            </li>
        {/each}
    </ul>
{/if}

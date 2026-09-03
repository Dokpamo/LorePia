<script lang="ts">
    import ChoicePopover from '../../components/ChoicePopover.svelte';
    import ToggleSwitch from '../../components/ToggleSwitch.svelte';
    import type { GenerationSelectionInput } from '../../lib/ipc/contracts';
    import { tr } from '../../lib/i18n';
    import type {
        PortableCharacterRuntime,
        PortableRuntimeCapability,
        PortableRuntimeGrant,
        PortableRuntimeModelCallStatus,
        PortableRuntimePersistenceStatus,
    } from './portable-runtime';
    import type { PortableRuntimeModelBudgetSnapshot } from './portable-runtime-model-policy';

    type PortableRuntimePhase = 'idle' | 'blocked' | 'loading' | 'ready' | 'busy' | 'error';

    interface AuxiliaryModelOption {
        value: string;
        label: string;
        selection: GenerationSelectionInput | null;
    }

    interface Props {
        phase: PortableRuntimePhase;
        grant: PortableRuntimeGrant | null;
        capabilities: PortableRuntimeCapability[];
        selectedCapabilities?: PortableRuntimeCapability[];
        runtime: PortableCharacterRuntime | null;
        selectedAuxiliaryModel: string;
        auxiliaryModelOptions: AuxiliaryModelOption[];
        modelBudget: PortableRuntimeModelBudgetSnapshot | null;
        modelCall: PortableRuntimeModelCallStatus | null;
        persistenceStatus: PortableRuntimePersistenceStatus | null;
        optionValue: (key: string) => string;
        onApprove: () => void | Promise<void>;
        onRevoke: () => void;
        onSelectAuxiliaryModel: (value: string) => void;
        onSetOption: (key: string, value: string) => void | Promise<void>;
        onCancelModelCall: () => void | Promise<void>;
    }

    let {
        phase,
        grant,
        capabilities,
        selectedCapabilities = $bindable([]),
        runtime,
        selectedAuxiliaryModel,
        auxiliaryModelOptions,
        modelBudget,
        modelCall,
        persistenceStatus,
        optionValue,
        onApprove,
        onRevoke,
        onSelectAuxiliaryModel,
        onSetOption,
        onCancelModelCall,
    }: Props = $props();

    function capabilityLabel(capability: PortableRuntimeCapability): string {
        const labels: Record<PortableRuntimeCapability, string> = {
            'runtime:callbacks': $tr('chat.runtime.capability.callbacks'),
            'chat:read': $tr('chat.runtime.capability.chat_read'),
            'chat:write': $tr('chat.runtime.capability.chat_write'),
            'state:readwrite': $tr('chat.runtime.capability.state_readwrite'),
            'profile:read': $tr('chat.runtime.capability.profile_read'),
            'lore:read': $tr('chat.runtime.capability.lore_read'),
            'ui:write': $tr('chat.runtime.capability.ui_write'),
            'model:primary': $tr('chat.runtime.capability.model_primary'),
            'model:auxiliary': $tr('chat.runtime.capability.model_auxiliary'),
            elevated: $tr('chat.runtime.capability.elevated'),
        };
        return labels[capability];
    }

    function toggleCapability(capability: PortableRuntimeCapability, checked: boolean): void {
        selectedCapabilities = checked
            ? [...new Set([...selectedCapabilities, capability])]
            : selectedCapabilities.filter((candidate) => candidate !== capability);
    }

    function selectAllCompatibilityCapabilities(): void {
        selectedCapabilities = [...capabilities];
    }
</script>

<section
    class="portable-runtime-controls"
    data-phase={phase}
    aria-label={$tr('chat.runtime.controls.label')}
>
    <header>
        <span class="portable-runtime-label">캐릭터 기능</span>
        <small class="portable-runtime-phase"
            >{phase === 'loading'
                ? '준비 중'
                : phase === 'blocked'
                  ? '승인 필요'
                  : phase === 'busy'
                    ? '실행 중'
                    : phase === 'error'
                      ? '오류'
                      : '사용 가능'}</small
        >
    </header>

    {#if grant === null}
        <div class="portable-runtime-approval">
            <p>아래 권한은 현재 카드 리비전·스크립트 해시에만 이번 세션 동안 허용됩니다.</p>
            <p class="portable-runtime-sensitive-note">
                {$tr('chat.runtime.permissions.sensitive')}
            </p>
            <ul
                class="portable-runtime-capabilities"
                aria-label={$tr('chat.runtime.permissions.requested')}
            >
                {#each capabilities as capability (capability)}
                    <li>
                        <label>
                            <input
                                type="checkbox"
                                checked={selectedCapabilities.includes(capability)}
                                onchange={(event) =>
                                    toggleCapability(capability, event.currentTarget.checked)}
                            />
                            <span>{capabilityLabel(capability)}</span>
                        </label>
                    </li>
                {/each}
            </ul>
            <div class="portable-runtime-approval-actions">
                <button type="button" onclick={selectAllCompatibilityCapabilities}>
                    {$tr('chat.runtime.permissions.select_all')}
                </button>
                <button class="primary" type="button" onclick={() => void onApprove()}>
                    {$tr('chat.runtime.permissions.approve_selected')}
                </button>
            </div>
        </div>
    {:else}
        <button class="portable-runtime-revoke" type="button" onclick={onRevoke}>
            캐릭터 기능 권한 해제
        </button>
        {#if persistenceStatus?.mode === 'memory-only'}
            <p class="portable-runtime-persistence-warning" role="status" aria-live="polite">
                {$tr('chat.runtime.persistence.memory_only')}
            </p>
        {/if}
        {#if runtime !== null}
            {#if grant.capabilities.includes('model:auxiliary')}
                <div class="portable-runtime-choice">
                    <ChoicePopover
                        id="portable-runtime-auxiliary-model"
                        label="보조 생성 모델"
                        value={selectedAuxiliaryModel}
                        options={auxiliaryModelOptions}
                        disabled={phase === 'busy'}
                        onSelect={onSelectAuxiliaryModel}
                    />
                </div>
            {/if}

            {#if modelBudget !== null && (grant.capabilities.includes('model:primary') || grant.capabilities.includes('model:auxiliary'))}
                <p class="portable-runtime-budget" aria-live="polite">
                    이번 세션 남은 호출 {modelBudget.callsRemaining}회 · 남은 토큰 예산
                    {modelBudget.tokensRemaining.toLocaleString()}개
                </p>
                {#if modelBudget.blockedByUnknownOutcome}
                    <p class="runtime-error">
                        모델 호출 결과를 확인할 수 없어 이 카드의 추가 호출을 이번 세션에서
                        차단했습니다.
                    </p>
                {/if}
            {/if}

            {#if modelCall !== null}
                <div class="portable-runtime-model-call" role="status">
                    <span>
                        {modelCall.characterName}이(가)
                        {modelCall.target === 'primary' ? '기본 모델' : '보조 모델'}을 호출
                        중입니다.
                    </span>
                    <button type="button" onclick={() => void onCancelModelCall()}>중지</button>
                </div>
            {/if}

            {#each runtime.toggles as toggle (toggle.key)}
                {#if toggle.kind === 'select'}
                    <div class="portable-runtime-choice">
                        <ChoicePopover
                            id={`portable-runtime-${toggle.key}`}
                            label={toggle.label}
                            value={optionValue(toggle.key)}
                            options={toggle.choices.map((choice, index) => ({
                                value: String(index),
                                label: choice,
                            }))}
                            disabled={phase === 'busy'}
                            onSelect={(value: string) => void onSetOption(toggle.key, value)}
                        />
                    </div>
                {:else if toggle.kind === 'toggle'}
                    <ToggleSwitch
                        label={toggle.label}
                        checked={optionValue(toggle.key) === '1'}
                        disabled={phase === 'busy'}
                        showLabel
                        onChange={(checked: boolean) =>
                            void onSetOption(toggle.key, checked ? '1' : '0')}
                    />
                {:else}
                    <label class="portable-runtime-field">
                        <span>{toggle.label}</span>
                        <input
                            type="text"
                            value={optionValue(toggle.key)}
                            disabled={phase === 'busy'}
                            onchange={(event) =>
                                void onSetOption(toggle.key, event.currentTarget.value)}
                        />
                    </label>
                {/if}
            {/each}
        {/if}
    {/if}
</section>

<style>
    .portable-runtime-controls {
        display: grid;
        gap: 12px;
        padding: 12px;
        border: 1px solid var(--line);
        border-radius: var(--radius-lg);
        background: color-mix(in srgb, var(--surface-sunken) 58%, var(--surface-raised));
        box-shadow: var(--shadow-1);
    }

    header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 10px;
    }

    .portable-runtime-label {
        color: var(--ink);
        font-size: 0.8125rem;
        font-weight: 700;
    }

    .portable-runtime-field > span,
    .portable-runtime-sensitive-note,
    .portable-runtime-budget,
    .portable-runtime-persistence-warning {
        color: var(--ink-muted);
        font-size: 0.72rem;
    }

    .portable-runtime-phase {
        display: inline-flex;
        min-height: 24px;
        align-items: center;
        padding: 0 9px;
        border: 1px solid var(--line);
        border-radius: var(--radius-pill);
        background: var(--surface-raised);
        color: var(--ink-muted);
        font-size: 0.7rem;
        font-weight: 700;
    }

    [data-phase='blocked'] .portable-runtime-phase,
    [data-phase='busy'] .portable-runtime-phase {
        border-color: var(--status-warning-border);
        background: var(--status-warning-bg);
        color: var(--status-warning-fg);
    }

    [data-phase='error'] .portable-runtime-phase {
        border-color: var(--status-error-border);
        background: var(--status-error-bg);
        color: var(--status-error-fg);
    }

    .portable-runtime-persistence-warning {
        margin: 0;
        padding: 8px;
        border: 1px solid var(--status-warning-border);
        border-radius: var(--radius-sm);
        background: var(--status-warning-bg);
        color: var(--ink);
    }

    .portable-runtime-choice {
        min-width: 0;
        border-radius: var(--radius-md);
        background: var(--surface-sunken);
    }

    .portable-runtime-approval {
        display: grid;
        gap: 12px;
        padding: 12px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
        color: var(--ink);
        font-size: 0.78rem;
        line-height: 1.45;
    }

    .portable-runtime-approval p,
    .portable-runtime-approval ul {
        margin: 0;
    }

    .portable-runtime-capabilities {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 6px;
        padding: 0;
        color: var(--ink-muted);
        list-style: none;
    }

    .portable-runtime-approval li label {
        display: flex;
        align-items: center;
        gap: 9px;
        min-height: 40px;
        padding: 6px 8px;
        border: 1px solid var(--line);
        border-radius: var(--radius-sm);
        background: var(--surface-sunken);
        cursor: pointer;
    }

    .portable-runtime-approval input {
        width: 16px;
        height: 16px;
        accent-color: var(--accent);
    }

    .portable-runtime-approval button,
    .portable-runtime-revoke {
        min-height: 40px;
        border: 1px solid var(--line-strong);
        border-radius: var(--radius-pill);
        background: var(--surface);
        color: var(--ink);
        font: inherit;
        cursor: pointer;
    }

    .portable-runtime-approval button.primary {
        border-color: transparent;
        background: var(--primary-bg);
        color: var(--primary-ink);
    }

    .portable-runtime-approval-actions {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 8px;
    }

    .portable-runtime-field {
        display: grid;
        gap: 5px;
    }

    .portable-runtime-field input {
        width: 100%;
        min-height: 36px;
        padding: 6px 9px;
        border: 1px solid var(--line);
        border-radius: 9px;
        background: var(--surface);
        color: var(--ink);
        font: inherit;
    }

    @container view (max-width: 420px) {
        .portable-runtime-capabilities,
        .portable-runtime-approval-actions {
            grid-template-columns: 1fr;
        }
    }

    .portable-runtime-model-call {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 10px;
        padding: 8px 10px;
        border: 1px solid var(--status-warning-border);
        border-radius: 9px;
        background: var(--status-warning-bg);
        font-size: 0.75rem;
    }

    .portable-runtime-model-call button {
        min-height: 30px;
        padding: 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        background: var(--surface);
        color: var(--ink);
        font: inherit;
        cursor: pointer;
    }
</style>

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
</script>

<section class="portable-runtime-controls" aria-label="캐릭터 기능 설정">
    <header>
        <span class="portable-runtime-label">캐릭터 기능</span>
        <small
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
            <ul aria-label="요청한 캐릭터 기능 권한">
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
            <button type="button" onclick={() => void onApprove()}>
                {$tr('chat.runtime.permissions.approve_selected')}
            </button>
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
        gap: 10px;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-sunken);
    }

    header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 10px;
    }

    .portable-runtime-label {
        color: var(--ink-muted);
        font-size: 0.75rem;
        font-weight: 650;
    }

    header small,
    .portable-runtime-field > span,
    .portable-runtime-sensitive-note,
    .portable-runtime-budget,
    .portable-runtime-persistence-warning {
        color: var(--ink-muted);
        font-size: 0.72rem;
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
        gap: 8px;
        padding: 10px;
        border: 1px solid var(--status-warning-border);
        border-radius: var(--radius-md);
        background: var(--status-warning-bg);
        color: var(--ink);
        font-size: 0.78rem;
        line-height: 1.45;
    }

    .portable-runtime-approval p,
    .portable-runtime-approval ul {
        margin: 0;
    }

    .portable-runtime-approval ul {
        display: grid;
        gap: 6px;
        padding: 0;
        color: var(--ink-muted);
        list-style: none;
    }

    .portable-runtime-approval li label {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 28px;
        cursor: pointer;
    }

    .portable-runtime-approval input {
        width: 16px;
        height: 16px;
        accent-color: var(--accent);
    }

    .portable-runtime-approval button,
    .portable-runtime-revoke {
        min-height: 36px;
        border: 1px solid var(--line-strong);
        border-radius: 9px;
        background: var(--surface);
        color: var(--ink);
        font: inherit;
        cursor: pointer;
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

<script lang="ts">
    import { Sparkles } from '@lucide/svelte';
    import { tick } from 'svelte';

    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import ChoicePopover from '../../components/ChoicePopover.svelte';
    import SegmentedControl from '../../components/SegmentedControl.svelte';
    import type { ConversationMode } from '../../lib/ipc/contracts';
    import { t } from '../../lib/i18n';
    import type { ChatComposerState } from './composer-state.svelte';
    import type { PortableRuntimeLifecycle } from './portable-runtime-lifecycle.svelte';
    import PortableRuntimeControls from './PortableRuntimeControls.svelte';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        composer: ChatComposerState;
        runtime: PortableRuntimeLifecycle;
        onNotice: (message: string) => void;
        closeSettings: () => Promise<void>;
    }

    let { appState, controller, composer, runtime, onNotice, closeSettings }: Props = $props();
    let copyNotice = '';

    function setMode(mode: ConversationMode): void {
        void controller.setConversationMode(mode);
    }

    async function cancelRuntimeModelCall(): Promise<void> {
        const cancellation = await runtime.cancelActiveModelCall();
        copyNotice =
            cancellation === 'unconfirmed'
                ? t('chat.runtime.cancel.unconfirmed')
                : cancellation === 'confirmed'
                  ? '캐릭터 모델 호출 중지를 요청했습니다.'
                  : '중지할 캐릭터 모델 호출이 없습니다.';
        onNotice(copyNotice);
    }
</script>

<div class="chat-room-controls">
    <div class="chat-room-control-block">
        <span class="chat-room-control-label">대화 모드</span>
        <SegmentedControl
            id="conversation-mode"
            label="대화 모드"
            value={appState.conversation_state?.selected_mode ?? 'chat'}
            options={[
                { value: 'chat', label: '채팅' },
                { value: 'story', label: '스토리' },
            ]}
            onSelect={(value: string) => setMode(value as ConversationMode)}
        />
    </div>

    {#if runtime.profile !== null && (runtime.profile.runtime_scripts.length > 0 || runtime.profile.output_transforms.length > 0 || runtime.profile.display_transforms.length > 0 || runtime.profile.background_markup.trim().length > 0)}
        <PortableRuntimeControls
            phase={runtime.phase}
            grant={runtime.activeGrant}
            capabilities={runtime.capabilities}
            bind:selectedCapabilities={runtime.selectedCapabilities}
            runtime={runtime.runtime}
            selectedAuxiliaryModel={runtime.selectedAuxiliaryModel}
            auxiliaryModelOptions={runtime.auxiliaryModelOptions}
            modelBudget={runtime.modelBudget}
            modelCall={runtime.modelCall}
            persistenceStatus={runtime.persistenceStatus}
            optionValue={(key: string) => runtime.optionValue(key)}
            onApprove={() => runtime.approve()}
            onRevoke={() => runtime.revoke()}
            onSelectAuxiliaryModel={(value: string) => runtime.setAuxiliaryModel(value)}
            onSetOption={(key: string, value: string) => runtime.setOption(key, value)}
            onCancelModelCall={cancelRuntimeModelCall}
        />
    {/if}

    {#if appState.branches.length > 1}
        <div class="branch-picker chat-room-branch">
            <span>분기</span>
            <ChoicePopover
                id="chat-active-branch"
                label="분기"
                value={appState.conversation_state?.active_branch_id ?? ''}
                showLabel={false}
                options={appState.branches.map((branch, index) => ({
                    value: branch.id,
                    label: branch.title ?? `분기 ${String(index + 1)}`,
                }))}
                onSelect={(value: string) => void controller.selectBranch(value)}
            />
        </div>
    {/if}

    <button
        class="chat-room-new-operation"
        type="button"
        aria-label="새 생성 작업"
        disabled={composer.sending ||
            appState.chat.phase === 'loading' ||
            appState.chat.active_generation_id !== null}
        onclick={async () => {
            controller.beginNewGenerationOperation();
            const newGenerationOperationCopyNotice =
                '새 생성 작업으로 전환했습니다. 같은 입력도 새로운 요청으로 처리됩니다.';
            copyNotice = newGenerationOperationCopyNotice;
            onNotice(copyNotice);
            await closeSettings();
            await tick();
            composer.focusTextarea();
        }}
    >
        <Sparkles aria-hidden="true" />
        <span>
            <strong>새 생성 작업</strong>
            <small>현재 입력을 별도의 새 요청으로 처리합니다.</small>
        </span>
    </button>
</div>

<style>
    .chat-room-controls,
    .chat-room-control-block {
        display: grid;
        gap: 8px;
    }

    .chat-room-control-label,
    .chat-room-branch > span {
        color: var(--ink-muted);
        font-size: 0.75rem;
        font-weight: 650;
    }

    .chat-room-branch {
        min-height: 42px;
        justify-content: space-between;
        padding: 0 4px;
    }

    .chat-room-branch :global(.choice-popover) {
        width: min(68%, 240px);
    }

    .chat-room-new-operation {
        display: flex;
        width: 100%;
        min-height: 54px;
        align-items: center;
        padding: 9px 12px;
        border: 0;
        border-radius: var(--radius-md);
        background: var(--surface-raised);
        box-shadow: var(--shadow-1);
        color: var(--ink);
        gap: 10px;
        text-align: left;
    }

    .chat-room-new-operation :global(svg) {
        width: 20px;
        height: 20px;
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 1.8;
    }

    .chat-room-new-operation > span {
        display: grid;
        min-width: 0;
        gap: 2px;
    }

    .chat-room-new-operation strong {
        font-size: 0.875rem;
    }

    .chat-room-new-operation small {
        color: var(--ink-muted);
        font-size: 0.72rem;
        line-height: 1.35;
    }
</style>

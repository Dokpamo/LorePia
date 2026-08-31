<script lang="ts">
    import DetailActionBar from '../../../components/detail/DetailActionBar.svelte';
    import type { CredentialStatus, CredentialTargetDto } from '../../../lib/ipc/contracts';

    interface Props {
        credentialTarget: CredentialTargetDto | null;
        showActions: boolean;
        credentialStatuses: Record<string, CredentialStatus>;
        savingKey: string | null;
        credentialDeleteConfirmationKey?: string | null;
        deleteCredential: (target: CredentialTargetDto) => void | Promise<void>;
        captureCredential: (target: CredentialTargetDto) => void | Promise<void>;
    }

    let {
        credentialTarget,
        showActions,
        credentialStatuses,
        savingKey,
        credentialDeleteConfirmationKey = $bindable(null),
        deleteCredential,
        captureCredential,
    }: Props = $props();

    function targetKey(target: CredentialTargetDto): string {
        switch (target.kind) {
            case 'connection':
                return `connection:${target.connection_id}`;
            case 'legacy_profile':
                return `legacy_profile:${target.provider_profile_id}`;
            case 'discovery_session':
                return `discovery_session:${target.session_id}`;
        }
    }
</script>

{#if credentialTarget && showActions}
    {@const key = targetKey(credentialTarget)}
    <DetailActionBar ariaLabel="자격증명 작업">
        {#if credentialDeleteConfirmationKey === key}
            <!-- prettier-ignore -->
            <button
                class="danger detail-action detail-action--destructive"
                type="button"
                disabled={savingKey === key}
                onclick={() => void deleteCredential(credentialTarget)}
                >삭제 확인</button
            >
            <button
                class="detail-action detail-action--grow"
                type="button"
                disabled={savingKey === key}
                onclick={() => (credentialDeleteConfirmationKey = null)}>취소</button
            >
        {:else}
            <button
                class="detail-action detail-action--destructive detail-action--borderless"
                type="button"
                disabled={credentialStatuses[key] === 'missing' || savingKey === key}
                onclick={() => (credentialDeleteConfirmationKey = key)}>삭제</button
            >
            <!-- prettier-ignore -->
            <button
                class="primary detail-action detail-action--grow"
                type="button"
                disabled={savingKey === key}
                onclick={() => void captureCredential(credentialTarget)}
                >자격증명 캡처</button
            >
        {/if}
    </DetailActionBar>
{/if}

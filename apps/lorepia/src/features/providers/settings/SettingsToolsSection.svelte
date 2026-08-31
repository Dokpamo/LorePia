<script lang="ts">
    import {
        Compass,
        GitBranch,
        Link2,
        ListChecks,
        Route,
        SlidersHorizontal,
    } from '@lucide/svelte';
    import type { ProviderWorkspaceDto } from '../../../lib/ipc/contracts';

    interface Props {
        section: 'discovery' | 'advanced';
        workspace: ProviderWorkspaceDto;
        onOpenDetailPage: (page: string) => void;
    }

    let { section, workspace, onOpenDetailPage }: Props = $props();
</script>

{#if section === 'discovery'}
    <div class="setting-list detail-tool-list" aria-label="검색과 동기화 도구">
        <button
            class="setting-row"
            type="button"
            onclick={() => onOpenDetailPage('provider-discovery')}
        >
            <span class="setting-icon" aria-hidden="true"><Compass /></span>
            <span class="setting-content">
                <span class="setting-copy detail-row-copy">
                    <strong>프로바이더 탐색</strong>
                    <small>연결을 찾고 검토해 추가합니다.</small>
                </span>
            </span>
        </button>
        <button class="setting-row" type="button" onclick={() => onOpenDetailPage('model-sync')}>
            <span class="setting-icon" aria-hidden="true"><ListChecks /></span>
            <span class="setting-content">
                <span class="setting-copy detail-row-copy">
                    <strong>모델 동기화</strong>
                    <small>연결에서 사용할 모델을 검토합니다.</small>
                </span>
            </span>
        </button>
    </div>
{:else}
    <div class="setting-list detail-tool-list" aria-label="고급 설정 도구">
        <button class="setting-row" type="button" onclick={() => onOpenDetailPage('connections')}>
            <span class="setting-icon" aria-hidden="true"><Link2 /></span>
            <span class="setting-content"
                ><span class="setting-copy"><strong>연결 관리</strong></span><span
                    class="setting-value">{workspace.connections.length}개</span
                ></span
            >
        </button>
        <button class="setting-row" type="button" onclick={() => onOpenDetailPage('routes')}>
            <span class="setting-icon" aria-hidden="true"><Route /></span>
            <span class="setting-content"
                ><span class="setting-copy"><strong>모델 라우트</strong></span><span
                    class="setting-value">{workspace.routes.length}개</span
                ></span
            >
        </button>
        <button class="setting-row" type="button" onclick={() => onOpenDetailPage('presets')}>
            <span class="setting-icon" aria-hidden="true"><GitBranch /></span>
            <span class="setting-content"
                ><span class="setting-copy"><strong>생성 프리셋</strong></span><span
                    class="setting-value">{workspace.presets.length}개</span
                ></span
            >
        </button>
        <button class="setting-row" type="button" onclick={() => onOpenDetailPage('capabilities')}>
            <span class="setting-icon" aria-hidden="true"><SlidersHorizontal /></span>
            <span class="setting-content"
                ><span class="setting-copy"><strong>모델 기능</strong></span></span
            >
        </button>
    </div>
{/if}

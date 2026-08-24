<script lang="ts">
    import { BadgeCheck } from '@lucide/svelte';

    import thirdPartyNotices from '../../../THIRD_PARTY_NOTICES.md?raw';
</script>

<article class="license-details" aria-labelledby="lucide-license-title">
    <header class="license-intro">
        <h2 id="lucide-license-title">Lucide Icons</h2>
        <p>앱의 인터페이스 아이콘에 사용하는 오픈소스 아이콘 모음입니다.</p>
    </header>

    <section class="license-section" aria-labelledby="lucide-permissions-title">
        <h3 id="lucide-permissions-title">허용 범위</h3>
        <ul class="license-permissions">
            <li><BadgeCheck aria-hidden="true" /><span>상업적 사용</span></li>
            <li><BadgeCheck aria-hidden="true" /><span>수정</span></li>
            <li><BadgeCheck aria-hidden="true" /><span>재배포</span></li>
        </ul>
    </section>

    <section class="license-section" aria-labelledby="lucide-information-title">
        <h3 id="lucide-information-title">라이선스 정보</h3>
        <dl class="license-facts">
            <div>
                <dt>라이선스</dt>
                <dd>ISC · 일부 Feather 파생 아이콘은 MIT</dd>
            </div>
            <div>
                <dt>공식 원문</dt>
                <dd>https://lucide.dev/license</dd>
            </div>
        </dl>
    </section>

    <section class="license-section license-notice" aria-labelledby="lucide-notice-title">
        <h3 id="lucide-notice-title">라이선스 전문</h3>
        <pre aria-label="Lucide Icons 라이선스 전문">{thirdPartyNotices}</pre>
    </section>
</article>

<style>
    .license-details {
        display: grid;
        gap: clamp(22px, 6.407vw, 28px);
    }

    .license-intro {
        display: grid;
        padding-bottom: clamp(18px, 5.034vw, 22px);
        border-bottom: 1px solid var(--line);
        gap: 7px;
    }

    .license-intro h2,
    .license-intro p,
    .license-section h3,
    .license-facts,
    .license-facts dd {
        margin: 0;
    }

    .license-intro h2 {
        color: var(--ink);
        font-size: var(--detail-ui-type);
        font-weight: 700;
        line-height: 1.3;
    }

    .license-intro p {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        line-height: 1.5;
    }

    .license-section {
        display: grid;
        gap: 9px;
    }

    .license-section h3 {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
        line-height: 1.35;
    }

    .license-permissions {
        display: grid;
        padding: 0;
        margin: 0;
        list-style: none;
    }

    .license-permissions li {
        display: flex;
        min-height: clamp(48px, 13.73vw, 60px);
        align-items: center;
        border-bottom: 1px solid var(--line);
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 550;
        gap: 10px;
    }

    .license-permissions li:last-child {
        border-bottom: 0;
    }

    .license-permissions :global(svg) {
        width: 19px;
        height: 19px;
        flex: none;
        color: var(--accent);
        stroke-width: 2;
    }

    .license-facts {
        display: grid;
    }

    .license-facts > div {
        display: grid;
        min-height: clamp(54px, 15.103vw, 66px);
        align-items: center;
        grid-template-columns: minmax(82px, 0.34fr) minmax(0, 1fr);
        border-bottom: 1px solid var(--line);
        gap: 12px;
    }

    .license-facts > div:last-child {
        border-bottom: 0;
    }

    .license-facts dt {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .license-facts dd {
        color: var(--ink);
        overflow-wrap: anywhere;
        font-size: var(--detail-support-type);
        line-height: 1.45;
    }

    .license-notice {
        padding-top: clamp(18px, 5.034vw, 22px);
        border-top: 1px solid var(--line);
    }

    pre {
        padding: 0;
        border: 0;
        margin: 0;
        background: transparent;
        color: var(--ink-muted);
        font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
        font-size: clamp(0.6875rem, 2.975vw, 0.75rem);
        line-height: 1.6;
        overflow-wrap: anywhere;
        white-space: pre-wrap;
    }
</style>

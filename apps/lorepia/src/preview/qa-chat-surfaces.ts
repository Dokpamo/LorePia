import { mount } from 'svelte';
import WorkspaceApp from '../app/workspace/WorkspaceApp.svelte';
import { createPreviewClient, createPreviewCharacterPresentations } from './mock-client';
import '../ui/workspace/styles';
import '../app/workspace/workspace.css';
import '../ui/navigation/seed-navigation.css';
import '../ui/navigation/seed-pages.css';
import { initTheme } from '../lib/theme';
import { initDisplay } from '../lib/display';
import { assetPresentationContext } from '../ui/workspace/asset-presentation';
import PreviewAsset from './PreviewAsset.svelte';

const qaClient = createPreviewClient();
if (!qaClient.getCharacterRenderProfile) throw new Error('QA render profile API missing');
const originalProfile = qaClient.getCharacterRenderProfile.bind(qaClient);
qaClient.getCharacterRenderProfile = async (...args) => ({
    ...(await originalProfile(...args)),
    assets: [],
    display_transforms: [],
    initial_variables: { qa_panel: '0' },
    background_markup: `<style>
    .qa-control { position: fixed; right: 12px; top: 12px; }
    .qa-control button { width: 48px; height: 48px; border-radius: 24px; color: white; background: #1f2937; border: 0; }
    .qa-panel { position: fixed; inset: 80px 16px 20px; background: #202126; color: white; border-radius: 20px; padding: 20px; overflow-y: auto; }
    .qa-panel button { padding: 12px; }
    </style><div class="qa-control">{{button::열기::QAopen}}</div>
    {{#if {{? {{getvar::qa_panel}} == 1}}}}<section class="qa-panel"><h2>카드 시스템 설정</h2>{{button::닫기::QAclose}}<p>한국어 · English</p><p>채팅 위의 카드 전용 화면입니다.</p></section>{{/}}`,
    runtime_scripts: [
        {
            id: 'qa-script',
            name: 'QA',
            event: 'start',
            language: 'lua',
            elevated_access: false,
            source: 'function QAopen(triggerId) setChatVar(triggerId, "qa_panel", "1") end\nfunction QAclose(triggerId) setChatVar(triggerId, "qa_panel", "0") end',
        },
    ],
    runtime_script_count: 1,
    required_runtime_capabilities: [
        'runtime:callbacks',
        'state:readwrite',
        'ui:write',
        'chat:read',
    ],
    runtime_capabilities_declared: true,
});
const originalMessages = qaClient.listBranchMessages.bind(qaClient);
qaClient.listBranchMessages = async (...args) => {
    const messages = await originalMessages(...args);
    const last = messages.at(-1);
    if (last)
        last.content =
            '<div style="height:160px;max-height:160px;overflow-y:auto">' +
            Array.from(
                { length: 60 },
                (_, index) =>
                    '<p>긴 말풍선 확인 ' +
                    String(index + 1) +
                    ' — 이 글은 대화 전체와 함께 스크롤됩니다.</p>',
            ).join('') +
            '</div>';
    return messages;
};
initTheme();
initDisplay();
const target = document.getElementById('app');
if (target === null) throw new Error('LorePia UI preview root is missing.');
export default mount(WorkspaceApp, {
    target,
    context: new Map([[assetPresentationContext, PreviewAsset]]),
    props: {
        client: qaClient,
        characterPresentations: createPreviewCharacterPresentations(),
    },
});

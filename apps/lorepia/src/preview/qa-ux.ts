// Isolated in-memory UX fixture: never uses native IPC or production data.
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
import { installLongHistoryFixture } from './history-qa-fixture';

initTheme();
initDisplay();
const fixture = createPreviewClient();
if (new URLSearchParams(location.search).get('history') === '2000')
    installLongHistoryFixture(fixture);
const delay = new URLSearchParams(location.search).get('fast') === '1' ? 0 : 1800;
const delayed = new Set([
    'getProviderOverview',
    'getCharacterGreetingCatalog',
    'getConversationState',
    'listBranches',
    'listBranchMessages',
    'listBranchMessagesPage',
    'listConversations',
]);
const client = new Proxy(fixture, {
    get(target, property, receiver) {
        const value: unknown = Reflect.get(target, property, receiver);
        if (typeof value !== 'function') return value;
        return (...args: unknown[]) => {
            console.info('UX QA request', String(property));
            if (!delayed.has(String(property)) || delay === 0)
                return Reflect.apply(value, target, args) as unknown;
            return new Promise((resolve) => setTimeout(resolve, delay)).then(
                () => Reflect.apply(value, target, args) as unknown,
            );
        };
    },
});
const target = document.getElementById('app');
if (!target) throw new Error('UX QA root missing');
mount(WorkspaceApp, {
    target,
    context: new Map([[assetPresentationContext, PreviewAsset]]),
    props: { client, characterPresentations: createPreviewCharacterPresentations() },
});

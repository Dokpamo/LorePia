import '../styles/app.css';
import App from '../app/App.svelte';
import { initTheme } from '../lib/theme';
import { initDisplay } from '../lib/display';
import { DEMO_INITIAL_CHARACTER_ID, DEMO_INITIAL_CONVERSATION_ID } from './demo-data';
import { createPreviewClient } from './mock-client';
import { mount } from 'svelte';

initTheme();
initDisplay();

const target = document.getElementById('app');

if (target === null) {
    throw new Error('LorePia preview root is missing.');
}

const app = mount(App, {
    target,
    props: {
        client: createPreviewClient(),
        initialSelection: {
            characterId: DEMO_INITIAL_CHARACTER_ID,
            conversationId: DEMO_INITIAL_CONVERSATION_ID,
        },
    },
});

export default app;

import './styles/app.css';
import App from './app/App.svelte';
import { initTheme } from './lib/theme';
import { DEMO_INITIAL_CHARACTER_ID, DEMO_INITIAL_CONVERSATION_ID } from './preview/demo-data';
import { createPreviewClient } from './preview/mock-client';
import { mount } from 'svelte';

initTheme();

const target = document.getElementById('app');

if (target === null) {
    throw new Error('LorePia application root is missing.');
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

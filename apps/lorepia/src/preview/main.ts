import '../styles/app.css';

import { mount } from 'svelte';

import App from '../app/App.svelte';
import { initTheme } from '../lib/theme';
import { DEMO_INITIAL_CHARACTER_ID, DEMO_INITIAL_CONVERSATION_ID } from './demo-data';
import { createPreviewClient } from './mock-client';

initTheme();

const target = document.getElementById('app');
if (target === null) throw new Error('preview root is missing');

mount(App, {
    target,
    props: {
        client: createPreviewClient(),
        initialSelection: {
            characterId: DEMO_INITIAL_CHARACTER_ID,
            conversationId: DEMO_INITIAL_CONVERSATION_ID,
        },
    },
});

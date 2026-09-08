import App from './app/workspace/WorkspaceApp.svelte';
import './ui/workspace/styles';
import './app/workspace/workspace.css';
import { initTheme } from './lib/theme';
import { initDisplay } from './lib/display';
import { mount } from 'svelte';

initTheme();
initDisplay();

const target = document.getElementById('app');

if (target === null) {
    throw new Error('LorePia application root is missing.');
}

const app = mount(App, { target });

export default app;

import { mount } from 'svelte';
import WorkspaceApp from '../../app/workspace/WorkspaceApp.svelte';
import { createPreviewClient } from '../mock-client';
import '../../ui/workspace/styles';
import '../../app/workspace/workspace.css';
import '../../ui/navigation/seed-navigation.css';
import '../../ui/navigation/seed-pages.css';
import { initTheme } from '../../lib/theme';
import { initDisplay } from '../../lib/display';

initTheme();
initDisplay();
const target = document.getElementById('app');
if (target === null) throw new Error('LorePia UI preview root is missing.');
export default mount(WorkspaceApp, { target, props: { client: createPreviewClient() } });

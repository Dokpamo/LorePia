import { mount } from 'svelte';
import UiPreview from './UiPreview.svelte';
import './ui-tokens.css';
import './ui-layout.css';
import './ui-content.css';
import './ui-motion.css';
import './ui-responsive.css';
import './ui-editing.css';
import './ui-composer.css';
import './ui-cards.css';
import './ui-chat.css';
import './ui-feedback.css';
import './ui-choices.css';

const target = document.getElementById('app');
if (target === null) throw new Error('LorePia UI preview root is missing.');

export default mount(UiPreview, { target });

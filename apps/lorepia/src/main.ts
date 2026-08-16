import './styles/app.css';
import App from './app/App.svelte';
import { mount } from 'svelte';

const target = document.getElementById('app');

if (target === null) {
    throw new Error('LorePia application root is missing.');
}

const app = mount(App, { target });

export default app;

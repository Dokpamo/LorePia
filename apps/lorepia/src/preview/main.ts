import '../styles/app.css';

import { mount } from 'svelte';

import App from '../app/App.svelte';
import { createPreviewClient } from './mock-client';

const target = document.getElementById('app');
if (target === null) throw new Error('preview root is missing');

mount(App, { target, props: { client: createPreviewClient() } });

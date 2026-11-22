import {mount} from 'svelte';
import './app.css';
import App from './App.svelte';
import {app} from './lib/app.svelte.ts';
import {router} from './lib/router.svelte.ts';

router.start();
app.start();
mount(App, {target: document.getElementById('app')!});

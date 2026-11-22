<script lang="ts">
  import type {Snippet} from 'svelte';
  import {app} from '../lib/app.svelte.ts';
  import Icon from './Icon.svelte';

  let {
    open = $bindable(false),
    title,
    kicker,
    busy = false,
    children,
    actions,
  }: {
    open?: boolean;
    title: string;
    kicker?: string;
    /** While true, the dialog stays open. */
    busy?: boolean;
    children: Snippet;
    actions: Snippet;
  } = $props();

  let dialog: HTMLDialogElement;

  // The polling waits while a dialog is open.
  $effect(() => {
    if (!open) return;
    dialog.showModal();
    const release = app.hold();
    return () => {
      release();
      dialog.close();
    };
  });

  function close() {
    if (!busy) open = false;
  }
</script>

<dialog
  bind:this={dialog}
  aria-labelledby="dialog-title"
  oncancel={event => {
    event.preventDefault();
    close();
  }}
  onclick={event => {
    if (event.target === dialog) close();
  }}
>
  <div class="body">
    <button class="icon-btn close" type="button" aria-label="Close" disabled={busy} onclick={close}><Icon name="close" /></button>
    {#if kicker}<span class="kicker">{kicker}</span>{/if}
    <h2 id="dialog-title">{title}</h2>
    {@render children()}
    <div class="actions">{@render actions()}</div>
  </div>
</dialog>

<style>
  dialog {
    width: min(440px, calc(100vw - 32px));
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-l);
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-l);
  }

  dialog::backdrop {
    background: var(--scrim);
    backdrop-filter: blur(2px);
  }

  dialog[open] {
    animation: pop 0.18s var(--ease);
  }

  .body {
    position: relative;
    display: grid;
    gap: var(--space-3);
    padding: var(--space-5);
  }

  .close {
    position: absolute;
    top: 12px;
    right: 12px;
  }

  .kicker {
    font-size: 12px;
    font-weight: 600;
    color: var(--accent);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  h2 {
    font-size: 18px;
    padding-right: 36px;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }

  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(6px) scale(0.98);
    }
  }
</style>

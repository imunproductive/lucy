<script lang="ts">
  import {app} from '../lib/app.svelte.ts';
  import Icon from './Icon.svelte';
</script>

<div class="toasts" aria-live="polite">
  {#each app.toasts as toast (toast.id)}
    <div class="toast" class:error={toast.error} role={toast.error ? 'alert' : 'status'}>
      <span class="mark"><Icon name={toast.error ? 'alert' : 'check'} size={16} /></span>
      <span class="text">{toast.text}</span>
      <button class="icon-btn" type="button" aria-label="Close" onclick={() => app.dismiss(toast.id)}>
        <Icon name="close" size={16} />
      </button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: var(--space-5);
    bottom: var(--space-5);
    z-index: 50;
    display: grid;
    gap: var(--space-2);
    width: min(400px, calc(100vw - 2 * var(--space-4)));
  }

  .toast {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 8px 8px 8px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    box-shadow: var(--shadow-l);
    animation: rise 0.2s var(--ease);
  }

  .mark {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    flex: none;
    background: var(--success-soft);
    color: var(--success);
  }

  .error .mark {
    background: var(--danger-soft);
    color: var(--danger);
  }

  .text {
    flex: 1;
    font-size: 13.5px;
  }

  .icon-btn {
    width: 28px;
    height: 28px;
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
  }

  @media (max-width: 640px) {
    .toasts {
      right: var(--space-4);
      bottom: var(--space-4);
    }
  }
</style>

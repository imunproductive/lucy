<script lang="ts">
  import type {Snippet} from 'svelte';
  import Icon from './Icon.svelte';
  import type {IconName} from './Icon.svelte';

  let {
    icon,
    title,
    text,
    children,
  }: {
    icon: IconName;
    title: string;
    text?: string;
    children?: Snippet;
  } = $props();
</script>

<div class="empty">
  <span class="mark"><Icon name={icon} size={22} /></span>
  <h2>{title}</h2>
  {#if text}<p>{text}</p>{/if}
  {#if children}<div class="actions">{@render children()}</div>{/if}
</div>

<style>
  .empty {
    display: grid;
    justify-items: center;
    gap: var(--space-2);
    padding: 48px var(--space-5);
    text-align: center;
  }

  .mark {
    display: grid;
    place-items: center;
    width: 48px;
    height: 48px;
    margin-bottom: var(--space-2);
    border-radius: 14px;
    background: var(--accent-soft);
    color: var(--accent-soft-text);
  }

  p {
    color: var(--muted);
    max-width: 52ch;
  }

  .actions {
    margin-top: var(--space-3);
    justify-content: center;
  }
</style>

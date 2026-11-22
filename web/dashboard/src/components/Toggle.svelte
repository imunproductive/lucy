<script lang="ts">
  import type {Snippet} from 'svelte';

  let {
    checked = $bindable(false),
    label,
    description,
    disabled = false,
    onchange,
    children,
  }: {
    checked?: boolean;
    label: string;
    description?: string;
    disabled?: boolean;
    onchange?: (checked: boolean) => void;
    children?: Snippet;
  } = $props();
</script>

<label class="toggle" class:disabled>
  <input type="checkbox" role="switch" bind:checked {disabled} onchange={() => onchange?.(checked)} />
  <span class="track" aria-hidden="true"><span class="thumb"></span></span>
  <span class="copy">
    <span class="title">{label}</span>
    {#if description}<span class="hint">{description}</span>{/if}
    {@render children?.()}
  </span>
</label>

<style>
  .toggle {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    cursor: pointer;
    position: relative;
  }

  .toggle.disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }

  input {
    position: absolute;
    opacity: 0;
    width: 36px;
    height: 20px;
    margin: 0;
  }

  .track {
    flex: none;
    width: 36px;
    height: 20px;
    margin-top: 1px;
    border-radius: 999px;
    background: var(--surface-3);
    border: 1px solid var(--border-strong);
    position: relative;
    transition: background 0.15s var(--ease);
  }

  .thumb {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px rgb(0 0 0 / 0.3);
    transition: transform 0.15s var(--ease);
  }

  input:checked + .track {
    background: var(--accent);
    border-color: var(--accent);
  }

  input:checked + .track .thumb {
    transform: translateX(16px);
  }

  input:focus-visible + .track {
    box-shadow: var(--focus);
  }

  .copy {
    display: grid;
    gap: 1px;
  }

  .title {
    font-weight: 560;
  }
</style>

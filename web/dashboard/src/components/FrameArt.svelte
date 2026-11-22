<script lang="ts">
  import {previewUrl} from '../lib/format.ts';

  // A portrait frame's picture (when the assets folder has it) with the
  // level shown on the card.
  let {guid, level, size = 96}: {guid: string | null; level: number; size?: number} = $props();

  let failed = $state<string | null>(null);
  const src = $derived(guid ? previewUrl(guid) : null);
</script>

<div class="frame" style:--size="{size}px">
  {#if src && failed !== src}
    <img {src} alt="" onerror={() => (failed = src)} />
  {:else}
    <span class="ring"></span>
  {/if}
  <b class="numeric">{level}</b>
</div>

<style>
  .frame {
    position: relative;
    display: grid;
    place-items: center;
    flex: none;
    width: var(--size);
    height: var(--size);
  }

  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .ring {
    position: absolute;
    inset: 10%;
    border-radius: 50%;
    border: 3px solid var(--accent);
    background: var(--accent-soft);
  }

  b {
    position: relative;
    font-size: calc(var(--size) * 0.2);
    font-weight: 750;
    color: var(--text);
    text-shadow: 0 1px 3px var(--surface);
  }
</style>

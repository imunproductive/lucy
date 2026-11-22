<script lang="ts">
  import {boxColour, boxPicture} from '../lib/boxes.ts';
  import type {BoxType} from '../lib/types.ts';

  let {box}: {box: BoxType} = $props();

  // The picture from the assets folder when there is one, else a drawn box
  // in the event's colour.
  let failed = $state<string | null>(null);
  const picture = $derived(boxPicture(box));
</script>

<div class="art" style:--box={boxColour(box)}>
  {#if picture && failed !== picture}
    <img src={picture} alt="" loading="lazy" onerror={() => (failed = picture)} />
  {:else}
    <svg viewBox="0 0 100 96" aria-hidden="true">
      <ellipse cx="50" cy="88" rx="30" ry="5" fill="#000" opacity="0.18" />
      <path d="M50 52 13 31v34l37 21z" fill="color-mix(in srgb, var(--box) 30%, #1b1e25)" />
      <path d="M50 52 87 31v34L50 86z" fill="color-mix(in srgb, var(--box) 45%, #262a33)" />
      <path d="M50 10 87 31 50 52 13 31z" fill="color-mix(in srgb, var(--box) 60%, #3a3f4a)" />
      <path d="M13 31 50 52 87 31M50 52v34" fill="none" stroke="var(--box)" stroke-width="2.4" />
      <path d="M13 40 50 61 87 40" fill="none" stroke="var(--box)" stroke-width="1.2" opacity="0.55" />
      <path d="M50 23 62 30 50 37 38 30z" fill="#fff" opacity="0.85" />
    </svg>
  {/if}
</div>

<style>
  .art {
    display: grid;
    place-items: center;
    aspect-ratio: 1;
    border-radius: var(--radius);
    background: radial-gradient(circle at 50% 40%, color-mix(in srgb, var(--box) 28%, transparent), transparent 70%);
  }

  img,
  svg {
    width: 82%;
    height: 82%;
    object-fit: contain;
    filter: drop-shadow(0 6px 10px rgb(0 0 0 / 0.18));
  }
</style>

<script lang="ts">
  import {currencies} from '../lib/format.ts';
  import type {Currency} from '../lib/types.ts';

  let {currency, size = 22}: {currency: Currency; size?: number} = $props();
</script>

<span class="coin {currency}" style:--size="{size}px" title={currencies[currency].label} aria-hidden="true">{currencies[currency].symbol}</span>

<style>
  .coin {
    display: inline-grid;
    place-items: center;
    flex: none;
    width: var(--size);
    height: var(--size);
    border-radius: 50%;
    font-size: calc(var(--size) * 0.42);
    font-weight: 750;
    letter-spacing: -0.02em;
    color: #fff;
  }

  .credits {
    background: linear-gradient(135deg, #f6c453, #d9921f);
  }

  .comp_points {
    background: linear-gradient(135deg, #f08bd0, #b44fd8);
  }

  .league_tokens {
    background: linear-gradient(135deg, #6fb6ff, #3c6fe0);
  }
</style>

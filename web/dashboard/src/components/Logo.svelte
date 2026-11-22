<script lang="ts">
  let {size = 30}: {size?: number} = $props();
</script>

<span class="logo">
  <svg viewBox="0 0 32 32" width={size} height={size} aria-hidden="true">
    <defs>
      <linearGradient id="lucy-mark" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0" stop-color="#7c6cff" />
        <stop offset="1" stop-color="#d45bf0" />
      </linearGradient>
    </defs>
    <rect width="32" height="32" rx="8" fill="url(#lucy-mark)" />
    <path d="M11 8v16h11" fill="none" stroke="#fff" stroke-width="3.6" stroke-linecap="round" stroke-linejoin="round" />
    <circle cx="22" cy="10" r="2.6" fill="#fff" />
  </svg>
  <span class="word">Lucy</span>
</span>

<style>
  .logo {
    display: inline-flex;
    align-items: center;
    gap: 10px;
  }

  svg {
    flex: none;
    border-radius: 8px;
    box-shadow: 0 4px 14px rgb(124 108 255 / 0.35);
  }

  .word {
    font-size: 18px;
    font-weight: 700;
    letter-spacing: -0.02em;
    color: var(--text);
  }
</style>

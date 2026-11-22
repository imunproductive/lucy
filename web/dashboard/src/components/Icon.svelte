<script lang="ts" module>
  // 24×24 stroke icons.
  const icons = {
    profile: 'M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8Zm-7 8a7 7 0 0 1 14 0',
    events: 'M4 6.5A1.5 1.5 0 0 1 5.5 5h13A1.5 1.5 0 0 1 20 6.5v12a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18.5Zm0 3.5h16M8 3v4m8-4v4',
    boxes: 'm3.5 7.5 8.5-4 8.5 4v9l-8.5 4-8.5-4Zm0 0 8.5 4 8.5-4M12 11.5v9M7.8 5.5l8.5 4',
    collection: 'M4 4h7v7H4Zm9 0h7v7h-7ZM4 13h7v7H4Zm9 0h7v7h-7Z',
    server: 'M4 5.5A1.5 1.5 0 0 1 5.5 4h13A1.5 1.5 0 0 1 20 5.5v4a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 9.5Zm0 9A1.5 1.5 0 0 1 5.5 13h13a1.5 1.5 0 0 1 1.5 1.5v4a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18.5ZM8 7.5h.01M8 16.5h.01',
    live: 'M3 12h4l3-8 4 16 3-8h4',
    refresh: 'M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7',
    sun: 'M12 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8Zm0-14v2m0 16v2M2 12h2m16 0h2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4m0-14.2-1.4 1.4M6.3 17.7l-1.4 1.4',
    moon: 'M20 14.5A8 8 0 0 1 9.5 4 8 8 0 1 0 20 14.5Z',
    menu: 'M4 7h16M4 12h16M4 17h16',
    close: 'M6 6l12 12M18 6 6 18',
    search: 'M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14Zm9 2-4-4',
    left: 'm15 6-6 6 6 6',
    right: 'm9 6 6 6-6 6',
    check: 'm5 12.5 4.5 4.5L19 7.5',
    play: 'M7 5v14l11-7Z',
    users: 'M9 11a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Zm-6 9a6 6 0 0 1 12 0m1-9a3 3 0 1 0 0-6m2 15a5 5 0 0 0-3-4.6',
    send: 'M21 3 10 14m11-11-7 18-4-7-7-4Z',
    alert: 'M12 9v4m0 4h.01M10.3 3.9 2.4 18a2 2 0 0 0 1.7 3h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z',
    arrow: 'M5 12h14m-6-6 6 6-6 6',
    plus: 'M12 5v14M5 12h14',
    minus: 'M5 12h14',
    clock: 'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18Zm0-13v4l3 2',
    message: 'M4 5h16v11H8l-4 4Z',
    stop: 'M6 6h12v12H6Z',
    gift: 'M4 11h16v9H4Zm-1-4h18v4H3Zm9 0v13M12 7c-1.5-3-5-3.5-5-1s5 1 5 1Zm0 0c1.5-3 5-3.5 5-1s-5 1-5 1Z',
    open: 'M14 4h6v6m0-6-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5',
  } as const;

  export type IconName = keyof typeof icons;
</script>

<script lang="ts">
  let {name, size = 18}: {name: IconName; size?: number} = $props();
</script>

<svg
  viewBox="0 0 24 24"
  width={size}
  height={size}
  fill="none"
  stroke="currentColor"
  stroke-width="1.8"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"><path d={icons[name]} /></svg>

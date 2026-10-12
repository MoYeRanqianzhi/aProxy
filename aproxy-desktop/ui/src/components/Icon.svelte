<!--
  内联描边图标（24×24 网格、圆头圆角），风格与吉祥物的圆润描边一致。
  不引入图标包：只画用得到的这几十个，产物里每个图标只是一小段 path。
  路径都是写死的常量，{@html} 不会接触任何外部输入。
-->
<script lang="ts" module>
  const paths = {
    home: '<path d="M3.5 10.5 12 3.5l8.5 7V19a1.5 1.5 0 0 1-1.5 1.5h-4.5v-6h-5v6H5A1.5 1.5 0 0 1 3.5 19z"/>',
    server: '<rect x="3" y="4" width="18" height="7" rx="3"/><rect x="3" y="13" width="18" height="7" rx="3"/><path d="M7 7.5h.01M7 16.5h.01"/>',
    list: '<path d="M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01"/>',
    sliders: '<path d="M4 6h8M18 6h2M4 12h2.5M11.5 12h8.5M4 18h10.5"/><circle cx="15" cy="6" r="2.5"/><circle cx="9" cy="12" r="2.5"/><circle cx="17" cy="18" r="2.5"/>',
    download: '<path d="M12 3.5v11M7.5 10.5 12 15l4.5-4.5M5 20.5h14"/>',
    gear: '<circle cx="12" cy="12" r="3"/><path d="M12 3.5v2M12 18.5v2M3.5 12h2M18.5 12h2M6 6l1.4 1.4M16.6 16.6 18 18M6 18l1.4-1.4M16.6 7.4 18 6"/><circle cx="12" cy="12" r="6.5"/>',
    search: '<circle cx="11" cy="11" r="6.5"/><path d="m20 20-4.2-4.2"/>',
    chevronDown: '<path d="m6 9 6 6 6-6"/>',
    chevronRight: '<path d="m9 6 6 6-6 6"/>',
    chevronLeft: '<path d="m15 6-6 6 6 6"/>',
    chevronUp: '<path d="m18 15-6-6-6 6"/>',
    x: '<path d="M18 6 6 18M6 6l12 12"/>',
    check: '<path d="M20 6.5 9.5 17 4 11.5"/>',
    copy: '<rect x="9" y="9" width="12" height="12" rx="3"/><path d="M5 15h-.5A1.5 1.5 0 0 1 3 13.5v-9A1.5 1.5 0 0 1 4.5 3h9A1.5 1.5 0 0 1 15 4.5V5"/>',
    pin: '<path d="M12 17v4.5"/><path d="M9 10.8a2 2 0 0 1-1.1 1.8l-1.8.9A2 2 0 0 0 5 15.2v.3a1.5 1.5 0 0 0 1.5 1.5h11a1.5 1.5 0 0 0 1.5-1.5v-.3a2 2 0 0 0-1.1-1.8l-1.8-.9a2 2 0 0 1-1.1-1.8V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>',
    play: '<path d="M7.5 5.2v13.6a1 1 0 0 0 1.5.9l11-6.8a1 1 0 0 0 0-1.8L9 4.3a1 1 0 0 0-1.5.9z"/>',
    stop: '<rect x="5.5" y="5.5" width="13" height="13" rx="3.5"/>',
    restart: '<path d="M20.5 12a8.5 8.5 0 1 1-2.5-6l2.5 2.5"/><path d="M20.5 3.5v5h-5"/>',
    heart: '<path d="M19 14c1.5-1.5 3-3.2 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.8 0-3 .5-4.5 2-1.5-1.5-2.7-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4 3 5.5l7 7z"/>',
    umbrella: '<path d="M21.5 12a9.5 9.5 0 0 0-19 0z"/><path d="M12 12v6.5a2 2 0 0 0 4 0"/><path d="M12 2.5v1"/>',
    bolt: '<path d="M13 2.5 4.5 13.5H11L10 21.5l8.5-11H12z"/>',
    arrowDown: '<path d="M12 5v14M18 13l-6 6-6-6"/>',
    arrowRight: '<path d="M5 12h14M13 6l6 6-6 6"/>',
    clock: '<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>',
    alert: '<path d="M10.3 4.2 2.7 17.4A2 2 0 0 0 4.4 20.5h15.2a2 2 0 0 0 1.7-3.1L13.7 4.2a2 2 0 0 0-3.4 0z"/><path d="M12 9.5v3.5M12 16.8h.01"/>',
    info: '<circle cx="12" cy="12" r="8.5"/><path d="M12 16v-4.5M12 8h.01"/>',
    shield: '<path d="M12 21.5s7.5-3.6 7.5-9.5V5.6L12 3 4.5 5.6V12c0 5.9 7.5 9.5 7.5 9.5z"/><path d="m9 12 2.2 2.2L15.5 10"/>',
    shieldOff: '<path d="M12 21.5s7.5-3.6 7.5-9.5V5.6L12 3 4.5 5.6V12c0 5.9 7.5 9.5 7.5 9.5z"/><path d="M9.5 10.5l5 5M14.5 10.5l-5 5"/>',
    key: '<circle cx="7.5" cy="15.5" r="4"/><path d="m10.5 12.5 9-9M16.5 6.5l3 3M14 9l2 2"/>',
    globe: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.5 2.5 3.5 5.5 3.5 8.5s-1 6-3.5 8.5M12 3.5C9.5 6 8.5 9 8.5 12s1 6 3.5 8.5"/>',
    terminal: '<rect x="3" y="4" width="18" height="16" rx="3.5"/><path d="m7.5 9 3 3-3 3M13 15h4"/>',
    file: '<path d="M14 3H7.5A2 2 0 0 0 5.5 5v14a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2V7.5z"/><path d="M14 3v4.5h4.5"/>',
    external: '<path d="M14.5 3.5h6v6M10 14 20.5 3.5M18.5 14v4.5a2 2 0 0 1-2 2h-11a2 2 0 0 1-2-2v-11a2 2 0 0 1 2-2H10"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    minus: '<path d="M5 12h14"/>',
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2.5v2M12 19.5v2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M2.5 12h2M19.5 12h2M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4"/>',
    moon: '<path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z"/>',
    monitor: '<rect x="3" y="4" width="18" height="12" rx="3"/><path d="M8.5 20h7M12 16v4"/>',
    languages: '<path d="M4 5.5h8M8 3.5v2M10.5 5.5C9.5 9.5 7 12.5 4.5 14M6.5 9c1 2 2.8 3.4 4.8 4.4"/><path d="m13 20.5 3.8-9 3.7 9M14.3 17.5h5"/>',
    sparkles: '<path d="M11 3.5 12.6 8l4.4 1.5-4.4 1.6L11 15.5l-1.6-4.4L5 9.5 9.4 8z"/><path d="M18 14.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z"/>',
    layers: '<path d="m12 3.5 8.5 4.8L12 13 3.5 8.3z"/><path d="m3.5 12.5 8.5 4.7 8.5-4.7"/><path d="m3.5 16.5 8.5 4.7 8.5-4.7"/>',
    power: '<path d="M12 3v8.5"/><path d="M18 6.5a8.5 8.5 0 1 1-12 0"/>',
    cloudRain: '<path d="M7 16.5a4.5 4.5 0 1 1 .9-8.9A6 6 0 0 1 19.4 9.6 3.5 3.5 0 0 1 18 16.5z"/><path d="m8.5 19.5-1 2M12.5 19.5l-1 2M16.5 19.5l-1 2"/>',
    hourglass: '<path d="M6.5 3h11M6.5 21h11M7.5 3c0 5 4.5 6 4.5 9s-4.5 4-4.5 9M16.5 3c0 5-4.5 6-4.5 9s4.5 4 4.5 9"/>',
    filter: '<path d="M3.5 5h17l-6.5 8v5.5l-4 2V13z"/>',
    eye: '<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/>',
    compare: '<circle cx="6" cy="6" r="2.5"/><circle cx="18" cy="18" r="2.5"/><path d="M6 8.5V15a3 3 0 0 0 3 3h3.5M18 15.5V9a3 3 0 0 0-3-3h-3.5"/><path d="m14 15.5 2.5 2.5-2.5 2.5M10 3.5 7.5 6 10 8.5"/>',
    shuffle: '<path d="M16 3.5h4.5V8M4 20 20.5 3.5M20.5 16v4.5H16M15 15l5.5 5.5M4 4l5 5"/>',
    lock: '<rect x="4.5" y="10.5" width="15" height="10.5" rx="3"/><path d="M8 10.5V7.5a4 4 0 0 1 8 0v3"/>',
    upstream: '<path d="M7 17 17 7M8.5 7H17v8.5"/>',
    repeat: '<path d="m17 2.5 3.5 3.5L17 9.5"/><path d="M3.5 11.5v-1a4.5 4.5 0 0 1 4.5-4.5h12.5"/><path d="m7 21.5-3.5-3.5L7 14.5"/><path d="M20.5 12.5v1a4.5 4.5 0 0 1-4.5 4.5H3.5"/>',
    more: '<circle cx="5.5" cy="12" r="1.2"/><circle cx="12" cy="12" r="1.2"/><circle cx="18.5" cy="12" r="1.2"/>',
    star: '<path d="m12 3.5 2.6 5.3 5.9.9-4.3 4.1 1 5.8L12 16.9l-5.2 2.7 1-5.8-4.3-4.1 5.9-.9z"/>',
    undo: '<path d="M9 14 4.5 9.5 9 5"/><path d="M4.5 9.5H15a5 5 0 0 1 0 10h-3.5"/>',
    braces: '<path d="M8 3.5H7a2 2 0 0 0-2 2v4a2 2 0 0 1-2 2.5 2 2 0 0 1 2 2.5v4a2 2 0 0 0 2 2h1M16 3.5h1a2 2 0 0 1 2 2v4a2 2 0 0 0 2 2.5 2 2 0 0 0-2 2.5v4a2 2 0 0 1-2 2h-1"/>',
    plug: '<path d="M9 3v4.5M15 3v4.5M6.5 7.5h11V11a5.5 5.5 0 0 1-11 0zM12 16.5V21"/>',
    command: '<path d="M9 9V6.5A2.5 2.5 0 1 0 6.5 9H9zm0 0h6m-6 0v6m6-6V6.5A2.5 2.5 0 1 1 17.5 9H15zm0 0v6m0 0h-6m6 0v2.5a2.5 2.5 0 1 0 2.5-2.5H15zm-6 0v2.5A2.5 2.5 0 1 1 6.5 15H9z"/>',
    puzzle: '<path d="M4 8h4.3a2.4 2.4 0 1 1 3.4 0H16v4.3a2.4 2.4 0 1 1 0 3.4V20H4z"/>',
    eyeOff: '<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><path d="m4 4 16 16"/>',
    palette: '<path d="M12 3.5a8.5 8.5 0 1 0 0 17c1.4 0 2-1 1.6-2.1-.5-1.3.4-2.4 1.8-2.4H17a3.5 3.5 0 0 0 3.5-3.5c0-5-3.8-9-8.5-9z"/><circle cx="7.6" cy="11.2" r=".9"/><circle cx="10.2" cy="7.6" r=".9"/><circle cx="14.6" cy="7.8" r=".9"/>',
  } as const;

  export type IconName = keyof typeof paths;
</script>

<script lang="ts">
  let {
    name,
    size = 18,
    stroke = 2,
    class: cls = '',
  }: { name: IconName; size?: number; stroke?: number; class?: string } = $props();
</script>

<svg
  class="icon {cls}"
  viewBox="0 0 24 24"
  width={size}
  height={size}
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {@html paths[name]}
</svg>

<style>
  .icon {
    flex: none;
    display: block;
  }
</style>

<!--
  分段选择：选中项背后有一块丁香色滑块，切换时弹簧滑过去。
  滑块的位置按实际按钮宽度测量，所以各项文字长短不一也能对齐。
-->
<script lang="ts" generics="T extends string">
  import Icon, { type IconName } from './Icon.svelte';

  let {
    options,
    value = $bindable(),
    label,
    size = 'md',
    onchange,
  }: {
    options: { value: T; label: string; icon?: IconName }[];
    value: T;
    label: string;
    size?: 'sm' | 'md';
    onchange?: (v: T) => void;
  } = $props();

  let buttons: HTMLButtonElement[] = $state([]);
  let group: HTMLDivElement;
  let thumb = $state({ left: 0, width: 0, ready: false });

  $effect(() => {
    const idx = options.findIndex((o) => o.value === value);
    const el = buttons[idx];
    if (!el) return;
    // 读取 options 的 label 让语言切换后重新测量
    void options.map((o) => o.label).join();
    const measure = () => (thumb = { left: el.offsetLeft, width: el.offsetWidth, ready: true });
    measure();
    // 字体晚到、或者主题换了字体，按钮宽度都会变：盯住整组的尺寸，变了就再量一次
    const ro = new ResizeObserver(measure);
    ro.observe(group);
    return () => ro.disconnect();
  });

  function choose(v: T) {
    value = v;
    onchange?.(v);
  }

  function onKey(ev: KeyboardEvent, idx: number) {
    if (ev.key !== 'ArrowRight' && ev.key !== 'ArrowLeft') return;
    ev.preventDefault();
    const next = (idx + (ev.key === 'ArrowRight' ? 1 : -1) + options.length) % options.length;
    choose(options[next].value);
    buttons[next]?.focus();
  }
</script>

<div class="seg {size}" role="radiogroup" aria-label={label} bind:this={group}>
  <span class="thumb" class:ready={thumb.ready} style="transform: translateX({thumb.left}px); width: {thumb.width}px"></span>
  {#each options as opt, i (opt.value)}
    <button
      type="button"
      role="radio"
      aria-checked={opt.value === value}
      tabindex={opt.value === value ? 0 : -1}
      class:active={opt.value === value}
      bind:this={buttons[i]}
      onclick={() => choose(opt.value)}
      onkeydown={(e) => onKey(e, i)}
    >
      {#if opt.icon}<Icon name={opt.icon} size={size === 'sm' ? 14 : 16} />{/if}
      <span>{opt.label}</span>
    </button>
  {/each}
</div>

<style>
  .seg {
    position: relative;
    display: inline-flex;
    align-items: center;
    padding: 3px;
    gap: 2px;
    border-radius: var(--r-pill);
    border: var(--line-w) solid var(--line);
    background: var(--card-2-bg);
  }
  .thumb {
    position: absolute;
    top: 3px;
    bottom: 3px;
    left: 0;
    border-radius: var(--r-pill);
    background: var(--lilac);
    /* 滑块不描边，用与自身同色相的柔和投影浮在槽里（同侧栏选中项） */
    box-shadow:
      0 1px 2px color-mix(in oklab, var(--lilac) 45%, transparent),
      0 3px 8px -2px color-mix(in oklab, var(--lilac) 85%, transparent);
    opacity: 0;
  }
  .thumb.ready {
    opacity: 1;
    transition:
      transform var(--dur-slow) var(--ease-spring),
      width var(--dur-slow) var(--ease-spring);
  }
  button {
    position: relative;
    z-index: 1;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 13px;
    border: 0;
    border-radius: var(--r-pill);
    background: transparent;
    color: var(--text-2);
    font-size: var(--fs-sm);
    font-weight: 550;
    white-space: nowrap;
    transition: color var(--dur-fast) linear;
  }
  button:hover:not(.active) {
    color: var(--text);
  }
  button.active {
    color: var(--on-fill);
  }
  .sm button {
    height: 24px;
    padding: 0 10px;
    font-size: var(--fs-xs);
  }
</style>

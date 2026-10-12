<!--
  下拉选择：胶囊触发器 + 弹出的列表。不用原生 <select>：它的弹出层无法跟随主题，
  在深色主题和 Mica 下会突兀地变成系统白底。
  键盘：Enter / 空格 / ↓ 打开，↑↓ 移动，Enter 选中，Esc 关闭。
-->
<script lang="ts" generics="T extends string | number">
  import Icon, { type IconName } from './Icon.svelte';
  import { pop, fadeOut } from '../lib/motion';

  let {
    options,
    value = $bindable(),
    label,
    icon,
    align = 'left',
  }: {
    options: { value: T; label: string; hint?: string; dot?: string }[];
    value: T;
    label: string;
    icon?: IconName;
    align?: 'left' | 'right';
  } = $props();

  let open = $state(false);
  let active = $state(0);
  let root: HTMLDivElement | undefined = $state();
  let list: HTMLUListElement | undefined = $state();
  const current = $derived(options.find((o) => o.value === value));
  const uid = `dd-${Math.random().toString(36).slice(2, 8)}`;

  function show() {
    open = true;
    active = Math.max(0, options.findIndex((o) => o.value === value));
    queueMicrotask(() => list?.focus());
  }

  function choose(v: T) {
    value = v;
    open = false;
    root?.querySelector<HTMLButtonElement>('.trigger')?.focus();
  }

  function onTriggerKey(ev: KeyboardEvent) {
    if (ev.key === 'ArrowDown' || ev.key === 'Enter' || ev.key === ' ') {
      ev.preventDefault();
      show();
    }
  }

  function onListKey(ev: KeyboardEvent) {
    if (ev.key === 'ArrowDown') active = (active + 1) % options.length;
    else if (ev.key === 'ArrowUp') active = (active - 1 + options.length) % options.length;
    else if (ev.key === 'Enter' || ev.key === ' ') choose(options[active].value);
    else if (ev.key === 'Escape' || ev.key === 'Tab') {
      open = false;
      if (ev.key === 'Escape') root?.querySelector<HTMLButtonElement>('.trigger')?.focus();
      return;
    } else return;
    ev.preventDefault();
  }

  function onWindowPointer(ev: PointerEvent) {
    if (open && root && !root.contains(ev.target as Node)) open = false;
  }
</script>

<svelte:window onpointerdown={onWindowPointer} />

<div class="dd" bind:this={root}>
  <button
    type="button"
    class="trigger"
    class:open
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-label={label}
    onclick={() => (open ? (open = false) : show())}
    onkeydown={onTriggerKey}
  >
    {#if icon}<Icon name={icon} size={15} />{/if}
    {#if current?.dot}<span class="dot {current.dot}"></span>{/if}
    <span class="cur">{current?.label ?? ''}</span>
    <span class="chev"><Icon name="chevronDown" size={14} stroke={2.4} /></span>
  </button>
  {#if open}
    <ul
      class="menu {align}"
      role="listbox"
      tabindex="-1"
      aria-label={label}
      aria-activedescendant="{uid}-{active}"
      bind:this={list}
      onkeydown={onListKey}
      in:pop={{ y: -6, scale: 0.97, duration: 420 }}
      out:fadeOut={{ duration: 120 }}
    >
      {#each options as opt, i (opt.value)}
        <li
          id="{uid}-{i}"
          role="option"
          aria-selected={opt.value === value}
          class:active={i === active}
          onpointerenter={() => (active = i)}
          onclick={() => choose(opt.value)}
          onkeydown={() => {}}
        >
          {#if opt.dot}<span class="dot {opt.dot}"></span>{/if}
          <span class="lbl">{opt.label}</span>
          {#if opt.hint}<span class="hint">{opt.hint}</span>{/if}
          {#if opt.value === value}<span class="tick"><Icon name="check" size={14} stroke={2.6} /></span>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .dd {
    position: relative;
    display: inline-block;
  }
  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 32px;
    padding: 0 10px 0 13px;
    border-radius: var(--r-pill);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 550;
    white-space: nowrap;
    /* 和 .btn 同一套浮起语言：平时贴身浅影，悬停/展开时上浮 1px、阴影加深 */
    box-shadow: var(--shadow-1);
    transition:
      box-shadow var(--dur-mid) var(--ease-soft),
      transform var(--dur-mid) var(--ease-spring);
  }
  .trigger:hover,
  .trigger.open {
    box-shadow: var(--shadow-2-hover);
    transform: translateY(-1px);
  }
  .chev {
    display: inline-flex;
    color: var(--text-2);
    transition: transform var(--dur-mid) var(--ease-spring);
  }
  .open .chev {
    transform: rotate(180deg);
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    z-index: 30;
    min-width: 100%;
    max-height: 280px;
    overflow: auto;
    margin: 0;
    padding: 5px;
    list-style: none;
    border-radius: var(--r-md);
    border: var(--line-w) solid var(--line);
    background: var(--overlay-bg);
    box-shadow: var(--shadow-2);
    transform-origin: top left;
    outline: none;
  }
  .menu.left {
    left: 0;
  }
  .menu.right {
    right: 0;
    transform-origin: top right;
  }
  li {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 32px;
    padding: 0 10px;
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    font-weight: 500;
    white-space: nowrap;
    cursor: pointer;
  }
  li.active {
    background: var(--hover);
  }
  li[aria-selected='true'] {
    color: var(--lilac-deep);
    font-weight: 600;
  }
  .lbl {
    flex: 1;
  }
  .hint {
    color: var(--text-3);
    font-size: var(--fs-xs);
  }
  .tick {
    display: inline-flex;
  }
</style>

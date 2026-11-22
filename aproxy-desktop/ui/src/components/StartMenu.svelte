<!--
  「启动实例」按钮与它弹出的配置列表。

  列表每次打开都现取（list_configs）：配置文件可能刚在别处加了、删了。选中一项就把它原样带回给 Rust 启动，
  界面不传路径、不猜端口——同一份配置已经在运行时由 Rust 以 desktop_port 拒绝，这里照常显示那句说明。
  样式与 Dropdown 的弹出列表一致；键盘：↑↓ 移动，Enter 启动，Esc 关闭。
-->
<script lang="ts">
  import type { Choice } from '../lib/backend/contract';
  import { toFailure } from '../lib/backend/link';
  import { faultText } from '../lib/faults';
  import { t } from '../lib/i18n';
  import { overlays } from '../lib/overlays.svelte';
  import { pop, fadeOut } from '../lib/motion';
  import { store } from '../lib/store.svelte';
  import Icon from './Icon.svelte';

  let open = $state(false);
  let choices = $state.raw<Choice[] | null>(null);
  let failed = $state<string | null>(null);
  let active = $state(0);
  /** 正在启动的那一项：防止连点，也让那一行显示转圈 */
  let starting = $state<string | null>(null);
  let root: HTMLDivElement | undefined = $state();
  let list: HTMLUListElement | undefined = $state();
  const uid = `sm-${Math.random().toString(36).slice(2, 8)}`;

  async function show() {
    open = true;
    choices = null;
    failed = null;
    active = 0;
    try {
      choices = await store.listConfigs();
      queueMicrotask(() => list?.focus());
    } catch (raw) {
      failed = faultText(toFailure(raw));
    }
  }

  async function start(c: Choice) {
    if (starting) return;
    starting = c.target.id;
    try {
      await store.start(c.target);
      overlays.toast(t('toast.started', { name: c.label }));
      open = false;
    } catch (raw) {
      overlays.toast(faultText(toFailure(raw)), 'coral');
    } finally {
      starting = null;
    }
  }

  function onListKey(ev: KeyboardEvent) {
    const n = choices?.length ?? 0;
    if (n === 0) return;
    if (ev.key === 'ArrowDown') active = (active + 1) % n;
    else if (ev.key === 'ArrowUp') active = (active - 1 + n) % n;
    else if (ev.key === 'Enter' || ev.key === ' ') void start(choices![active]);
    else if (ev.key === 'Escape' || ev.key === 'Tab') {
      open = false;
      return;
    } else return;
    ev.preventDefault();
  }

  function onWindowPointer(ev: PointerEvent) {
    if (open && root && !root.contains(ev.target as Node)) open = false;
  }
</script>

<svelte:window onpointerdown={onWindowPointer} />

<div class="start-menu" bind:this={root}>
  <button class="btn sm" aria-haspopup="listbox" aria-expanded={open} onclick={() => (open ? (open = false) : show())}>
    <Icon name="plus" size={14} stroke={2.4} />{t('ov.startInstance')}
  </button>
  {#if open}
    <div class="menu" in:pop={{ y: -6, scale: 0.97, duration: 420 }} out:fadeOut={{ duration: 120 }}>
      {#if failed}
        <p class="note err">{failed}</p>
      {:else if choices === null}
        <p class="note">{t('start.loading')}</p>
      {:else if choices.length === 0}
        <p class="note">{t('start.none')}</p>
      {:else}
        <ul
          role="listbox"
          tabindex="-1"
          aria-label={t('ov.startInstance')}
          aria-activedescendant="{uid}-{active}"
          bind:this={list}
          onkeydown={onListKey}
        >
          {#each choices as c, i (c.target.id)}
            <li
              id="{uid}-{i}"
              role="option"
              aria-selected={i === active}
              class:active={i === active}
              onpointerenter={() => (active = i)}
              onclick={() => start(c)}
              onkeydown={() => {}}
            >
              <span class="lbl">{c.label}{#if c.isDefault}<span class="chip lilac">{t('start.default')}</span>{/if}</span>
              <span class="path mono">{c.exists ? c.path : t('start.firstUse', { path: c.path })}</span>
              {#if starting === c.target.id}<span class="spin"></span>{/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</div>

<style>
  .start-menu {
    position: relative;
    display: inline-block;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 30;
    min-width: 280px;
    max-width: 440px;
    max-height: 320px;
    overflow: auto;
    padding: 5px;
    border-radius: var(--r-md);
    border: var(--line-w) solid var(--line);
    background: var(--overlay-bg);
    box-shadow: var(--shadow-2);
    transform-origin: top right;
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
    outline: none;
  }
  li {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    column-gap: 8px;
    padding: 7px 10px;
    border-radius: var(--r-sm);
    cursor: pointer;
  }
  li.active {
    background: var(--hover);
  }
  .lbl {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .path {
    grid-column: 1;
    color: var(--text-3);
    font-size: var(--fs-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spin {
    grid-column: 2;
    grid-row: 1 / span 2;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    border: 2px solid currentColor;
    border-right-color: transparent;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to { transform: rotate(360deg); }
  }
  .note {
    margin: 0;
    padding: 8px 10px;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .note.err {
    color: var(--coral-deep);
  }
</style>

<!--
  命令面板（Ctrl+K 或点标题栏的搜索框）：跳页面、跳实例、按编号找请求、切主题与语言。
  只是导航与轻操作的快捷入口；停止、重启这类危险操作故意不放进来，免得手快误触。
-->
<script lang="ts">
  import { overlays } from '../lib/overlays.svelte';
  import { ui, type Page } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { t } from '../lib/i18n';
  import { pop, fadeOut } from '../lib/motion';
  import Icon, { type IconName } from './Icon.svelte';

  interface Item {
    id: string;
    group: 'pages' | 'instances' | 'actions' | 'requests';
    label: string;
    hint?: string;
    icon: IconName;
    run: () => void;
  }

  let query = $state('');
  let active = $state(0);
  let input: HTMLInputElement | undefined = $state();

  $effect(() => {
    if (overlays.palette) {
      query = '';
      active = 0;
      queueMicrotask(() => input?.focus());
    }
  });

  const pages = [
    ['overview', 'home'],
    ['logs', 'list'],
    ['config', 'sliders'],
    ['update', 'download'],
    ['settings', 'gear'],
  ] as const satisfies readonly (readonly [Page, IconName])[];

  const items = $derived.by((): Item[] => {
    const q = query.trim().toLowerCase();
    const all: Item[] = [
      ...pages.map(([p, icon]) => ({ id: `p-${p}`, group: 'pages' as const, label: t(`nav.${p}`), icon, run: () => ui.go(p) })),
      ...store.instances.map((i) => ({
        id: `i-${i.port}`,
        group: 'instances' as const,
        label: `${i.port}`,
        hint: i.alias ?? undefined,
        icon: 'server' as const,
        run: () => ui.go('instance', i.port),
      })),
      {
        id: 'a-theme',
        group: 'actions',
        label: t('cmd.toggleTheme'),
        icon: ui.variant === 'dark' ? 'sun' : 'moon',
        run: () => (ui.modePref = ui.variant === 'dark' ? 'light' : 'dark'),
      },
      {
        id: 'a-lang',
        group: 'actions',
        label: t('cmd.toggleLang'),
        icon: 'languages',
        run: () => (ui.langPref = ui.lang === 'en' ? 'zh-CN' : 'en'),
      },
    ];
    let found = q ? all.filter((it) => `${it.label} ${it.hint ?? ''}`.toLowerCase().includes(q)) : all;
    // 输入数字时按请求编号找（aProxy 的请求编号就是实例内的序号）
    const num = q.replace(/^#/, '');
    if (/^\d{2,}$/.test(num)) {
      const hits = store.logs.filter((e) => String(e.seq).startsWith(num)).slice(-6).reverse();
      found = [
        ...found,
        ...hits.map((e) => ({
          id: `r-${e.key}`,
          group: 'requests' as const,
          label: `#${e.seq}  ${e.method} ${e.path.split('?')[0]}`,
          hint: `${e.port}`,
          icon: 'list' as const,
          run: () => {
            ui.selectedLog = e.key;
            ui.go('logs');
          },
        })),
      ];
    }
    return found;
  });

  const groups = $derived(
    (['pages', 'instances', 'requests', 'actions'] as const)
      .map((g) => ({ g, list: items.filter((it) => it.group === g) }))
      .filter((x) => x.list.length > 0),
  );

  function run(it: Item | undefined) {
    if (!it) return;
    overlays.palette = false;
    it.run();
  }

  function onKey(ev: KeyboardEvent) {
    if (ev.key === 'ArrowDown') active = Math.min(items.length - 1, active + 1);
    else if (ev.key === 'ArrowUp') active = Math.max(0, active - 1);
    else if (ev.key === 'Enter') run(items[active]);
    else if (ev.key === 'Escape') overlays.palette = false;
    else return;
    ev.preventDefault();
  }
</script>

{#if overlays.palette}
  <div class="scrim" in:fadeOut={{ duration: 160 }} out:fadeOut={{ duration: 120 }} onclick={() => (overlays.palette = false)} aria-hidden="true"></div>
  <div class="layer">
    <div class="palette" role="dialog" aria-modal="true" aria-label={t('search.placeholder')} in:pop={{ y: -12, scale: 0.97, duration: 480 }} out:fadeOut={{ duration: 120 }}>
      <label class="field">
        <Icon name="search" size={18} />
        <input
          bind:this={input}
          bind:value={query}
          oninput={() => (active = 0)}
          onkeydown={onKey}
          placeholder={t('cmd.placeholder')}
          aria-controls="cmd-list"
          aria-activedescendant={items[active] ? `cmd-${items[active].id}` : undefined}
          role="combobox"
          aria-expanded="true"
          aria-autocomplete="list"
        />
        <span class="kbd">Esc</span>
      </label>
      <div class="list" id="cmd-list" role="listbox" aria-label={t('search.placeholder')}>
        {#each groups as { g, list } (g)}
          <div class="group" role="presentation">{t(`cmd.${g}`)}</div>
          {#each list as it (it.id)}
            {@const idx = items.indexOf(it)}
            <div
              id="cmd-{it.id}"
              class="item"
              class:active={idx === active}
              role="option"
              tabindex="-1"
              aria-selected={idx === active}
              onpointerenter={() => (active = idx)}
              onclick={() => run(it)}
              onkeydown={() => {}}
            >
              <span class="ic"><Icon name={it.icon} size={16} /></span>
              <span class="label" class:mono={it.group === 'instances' || it.group === 'requests'}>{it.label}</span>
              {#if it.hint}<span class="hint">{it.hint}</span>{/if}
              {#if idx === active}<span class="enter"><Icon name="arrowRight" size={14} stroke={2.4} /></span>{/if}
            </div>
          {/each}
        {:else}
          <div class="none">{t('cmd.noResults')}</div>
        {/each}
      </div>
      <div class="foot">
        <span><span class="kbd">↑</span><span class="kbd">↓</span> {t('cmd.select')}</span>
        <span><span class="kbd">Enter</span> {t('cmd.open')}</span>
        <span><span class="kbd">Esc</span> {t('cmd.close')}</span>
      </div>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 70;
    background: var(--scrim);
  }
  .layer {
    position: fixed;
    inset: 0;
    z-index: 71;
    display: flex;
    justify-content: center;
    padding-top: 72px;
    pointer-events: none;
  }
  .palette {
    width: min(560px, calc(100vw - 48px));
    height: fit-content;
    max-height: calc(100vh - 140px);
    display: flex;
    flex-direction: column;
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--overlay-bg);
    box-shadow: var(--shadow-3);
    overflow: hidden;
    pointer-events: auto;
  }
  .field {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 16px;
    border-bottom: var(--line-w-thin) solid var(--line-soft);
    color: var(--text-2);
  }
  .field input {
    flex: 1;
    border: 0;
    outline: none;
    background: transparent;
    font-size: var(--fs-lg);
    color: var(--text);
  }
  .field input::placeholder {
    color: var(--text-3);
  }
  .list {
    overflow: auto;
    padding: 6px 8px 8px;
  }
  .group {
    padding: 10px 10px 4px;
    color: var(--text-3);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 38px;
    padding: 0 10px;
    border-radius: var(--r-sm);
    cursor: pointer;
  }
  .item.active {
    background: var(--lilac-soft);
  }
  .ic {
    display: inline-grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 9px;
    background: var(--card-2-bg);
    color: var(--text-2);
  }
  .item.active .ic {
    background: var(--lilac);
    color: var(--on-fill);
  }
  .label {
    font-weight: 550;
  }
  .hint {
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .enter {
    margin-left: auto;
    color: var(--lilac-deep);
    display: inline-flex;
  }
  .none {
    padding: 24px;
    text-align: center;
    color: var(--text-3);
  }
  .foot {
    display: flex;
    gap: 16px;
    padding: 9px 16px;
    border-top: var(--line-w-thin) solid var(--line-soft);
    color: var(--text-3);
    font-size: var(--fs-xs);
  }
  .foot .kbd {
    margin-right: 2px;
  }
</style>

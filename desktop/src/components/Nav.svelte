<!--
  左侧导航：圆润的胶囊项，选中项是一块丁香色填充，带同色相的柔和投影。
  「实例」下面直接列出每个实例（状态点 + 端口 + 别名），一眼看到谁在线、谁在重试。
  底部是看护者状态与版本号。
-->
<script lang="ts">
  import { t } from '../lib/i18n';
  import { ui, type Page } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import type { Instance } from '../lib/types';
  import Icon, { type IconName } from './Icon.svelte';

  const items = [
    ['overview', 'home'],
    ['logs', 'list'],
    ['config', 'sliders'],
    ['update', 'download'],
    ['settings', 'gear'],
  ] as const satisfies readonly (readonly [Page, IconName])[];

  function dotOf(i: Instance): string {
    if (i.status === 'stopped') return 'gray';
    if (i.status === 'unresponsive') return 'coral';
    if (i.status === 'restarting') return 'butter';
    if (store.retrying.some((e) => e.port === i.port)) return 'sky breathe';
    return i.status === 'active' ? 'mint breathe' : 'mint';
  }

  const retryingCount = $derived(store.retrying.length);
  const watched = $derived(store.instances.filter((i) => i.status !== 'stopped').length);
</script>

<nav class="nav" aria-label={t('nav.main')}>
  <ul class="items">
    {#each items.slice(0, 1) as [page, icon] (page)}
      <li>
        <button class="item" aria-current={ui.page === page ? 'page' : undefined} onclick={() => ui.go(page)}>
          <Icon name={icon} size={18} />
          <span class="label">{t(`nav.${page}`)}</span>
        </button>
      </li>
    {/each}

    <li class="group">
      <div class="group-head">
        <Icon name="server" size={18} />
        <span class="label">{t('nav.instances')}</span>
      </div>
      <ul class="instances">
        {#each store.instances as inst (inst.port)}
          <li>
            <button
              class="inst"
              aria-current={ui.page === 'instance' && ui.instancePort === inst.port ? 'page' : undefined}
              onclick={() => ui.go('instance', inst.port)}
            >
              <span class="dot {dotOf(inst)}"></span>
              <span class="port mono">{inst.port}</span>
              {#if inst.alias}<span class="alias">{inst.alias}</span>{/if}
              <!-- 仅转发（不重试）用和别处一样的警示胶带表示，窄栏里不占字 -->
              {#if inst.forwardOnly}<span class="fo" title="{t('mode.forwardOnly')} / {t('mode.noRetry')}"><span class="sr-only">{t('mode.forwardOnly')}</span></span>{/if}
            </button>
          </li>
        {:else}
          <li class="none">{t('cfgsum.none')}</li>
        {/each}
      </ul>
    </li>

    {#each items.slice(1) as [page, icon] (page)}
      <li>
        <button class="item" aria-current={ui.page === page ? 'page' : undefined} onclick={() => ui.go(page)}>
          <Icon name={icon} size={18} />
          <span class="label">{t(`nav.${page}`)}</span>
          {#if page === 'logs' && retryingCount > 0}
            <span class="badge sky" title={t('card.retryingN', { n: retryingCount })}>
              <Icon name="umbrella" size={12} stroke={2.4} />{retryingCount}
            </span>
          {/if}
          {#if page === 'update'}
            <span class="badge butter">{t('nav.updateBadge')}</span>
          {/if}
        </button>
      </li>
    {/each}
  </ul>

  <div class="foot">
    <button class="watchdog" class:off={!store.watchdogOn} onclick={() => ui.go('settings')}>
      <span class="wd-icon"><Icon name={store.watchdogOn ? 'shield' : 'shieldOff'} size={16} /></span>
      <span class="wd-text">
        <span class="wd-title">{t('wd.title')}</span>
        <span class="wd-sub">
          {#if !store.watchdogOn}{t('cfgsum.off')}{:else if watched === 0}{t('wd.nothing')}{:else}{t('wd.watching', { n: watched })}{/if}
        </span>
      </span>
    </button>
    <div class="version">
      <span class="mono">v0.2.0</span>
      <span class="chip lilac">desktop</span>
    </div>
  </div>
</nav>

<style>
  .nav {
    flex: none;
    width: var(--nav-w);
    display: flex;
    flex-direction: column;
    padding: 10px 12px 12px;
    background: var(--chrome-bg);
    overflow-y: auto;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .items {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .item,
  .group-head {
    display: flex;
    align-items: center;
    gap: 11px;
    width: 100%;
    height: 38px;
    padding: 0 14px;
    border-radius: var(--r-pill);
    border: var(--line-w) solid transparent;
    background: transparent;
    color: var(--text-2);
    font-size: var(--fs-md);
    font-weight: 550;
    text-align: left;
    transition:
      background-color var(--dur-fast) linear,
      transform var(--dur-mid) var(--ease-spring),
      box-shadow var(--dur-mid) var(--ease-spring);
  }
  .item:hover {
    background: var(--hover);
    color: var(--text);
  }
  .item:active {
    transform: scale(0.97);
  }
  /* 选中项靠主色填充 + 与填充同色相的柔和投影来分层，不再描边：
     投影颜色取自身填充，换配色预设时阴影也跟着变色，不会出现一块灰影。 */
  .item[aria-current='page'] {
    background: var(--lilac);
    color: var(--on-fill);
    box-shadow:
      0 1px 2px color-mix(in oklab, var(--lilac) 40%, transparent),
      0 6px 16px -6px color-mix(in oklab, var(--lilac) 90%, transparent);
  }
  .label {
    flex: 1;
  }
  .group {
    margin: 6px 0 4px;
  }
  .group-head {
    color: var(--text-3);
    height: 30px;
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .instances {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 2px 0 0 22px;
    padding-left: 10px;
    border-left: var(--line-w) dotted var(--line-soft);
  }
  .inst {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 32px;
    padding: 0 10px;
    border-radius: var(--r-pill);
    border: var(--line-w-thin) solid transparent;
    background: transparent;
    color: var(--text-2);
    font-size: var(--fs-sm);
    text-align: left;
    transition: background-color var(--dur-fast) linear;
  }
  .inst:hover {
    background: var(--hover);
    color: var(--text);
  }
  .inst[aria-current='page'] {
    background: var(--lilac-soft);
    color: var(--text);
    box-shadow: var(--shadow-1);
  }
  .port {
    font-weight: 500;
    color: var(--text);
  }
  .alias {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fo {
    flex: none;
    width: 20px;
    height: 12px;
    border-radius: var(--r-pill);
    /* 斜纹小胶带自身对比已足够，边框用奶黄的深色调收边，而不是墨线 */
    border: var(--line-w) solid var(--butter-deep);
    background: repeating-linear-gradient(-45deg, var(--on-fill) 0 3px, var(--butter) 3px 6px);
  }
  .none {
    padding: 6px 10px;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    height: 20px;
    padding: 0 7px;
    border-radius: var(--r-pill);
    color: var(--on-fill);
    font-size: 11px;
    font-weight: 600;
  }
  .badge.sky {
    background: var(--sky);
  }
  .badge.butter {
    background: var(--butter);
  }
  .foot {
    margin-top: auto;
    padding-top: 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .watchdog {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-bg);
    text-align: left;
    transition:
      border-color var(--dur-fast) linear,
      box-shadow var(--dur-mid) var(--ease-soft);
  }
  .watchdog:hover {
    border-color: var(--line);
    box-shadow: var(--shadow-1);
  }
  .wd-icon {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 50%;
    background: var(--mint);
    color: var(--on-fill);
  }
  .off .wd-icon {
    background: var(--card-2-bg);
    color: var(--text-3);
  }
  .wd-text {
    display: flex;
    flex-direction: column;
    line-height: 1.25;
  }
  .wd-title {
    font-weight: 600;
    font-size: var(--fs-sm);
  }
  .wd-sub {
    color: var(--text-3);
    font-size: var(--fs-xs);
  }
  .version {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 8px;
    color: var(--text-3);
    font-size: var(--fs-xs);
  }
  .version .chip {
    height: 20px;
    font-size: 11px;
  }
</style>

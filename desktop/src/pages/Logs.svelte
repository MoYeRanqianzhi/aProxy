<!--
  请求日志：一条请求一行的结构化列表，点开在右侧抽屉里看详情。

  为将来的虚拟滚动留路（海量日志的性能在审核通过后再做）：
  - 每一行高度固定为 --row-h，任何内容都不能把行撑高（单行、溢出省略、不做行内展开）；
  - 列表只有一个滚动容器（.scroller），表头在它外面，不参与滚动；
  - 详情放在抽屉里而不是插在行之间，选中某行不会改变其它行的位置。
  到时只需把 {#each} 换成按 scrollTop 截取的窗口，并用总行数 × 行高撑出滚动高度。
-->
<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { t, clock, durShort, bytes, fmtNum } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import type { LogEntry } from '../lib/types';
  import { slideIn, pop, fadeOut } from '../lib/motion';
  import Icon from '../components/Icon.svelte';
  import Segmented from '../components/Segmented.svelte';
  import Dropdown from '../components/Dropdown.svelte';
  import Switch from '../components/Switch.svelte';
  import ResultChip from '../components/ResultChip.svelte';
  import MascotView from '../components/MascotView.svelte';
  import { mascots } from '../lib/mascot/registry.svelte';
  import LogDetail from './LogDetail.svelte';

  type StatusFilter = 'all' | 'rescued' | 'retrying' | 'failed' | 'ok';

  let status = $state<StatusFilter>('all');
  let instance = $state<number | 'all'>(ui.logInstanceFilter);
  let model = $state<string>('all');
  let query = $state('');
  let onlyRetried = $state(false);
  let onlyErrors = $state(false);
  let onlyPinned = $state(false);
  let follow = $state(true);
  let newCount = $state(0);
  let scroller: HTMLDivElement | undefined = $state();

  ui.logInstanceFilter = 'all';

  const logs = $derived(store.logs);

  const counts = $derived.by(() => {
    const c = { all: 0, rescued: 0, retrying: 0, failed: 0, ok: 0 };
    for (const e of logs) {
      c.all += 1;
      if (e.outcome === 'rescued') c.rescued += 1;
      else if (e.outcome === 'retrying') c.retrying += 1;
      else if (e.outcome === 'failed') c.failed += 1;
      else if (e.outcome === 'ok') c.ok += 1;
    }
    return c;
  });

  const models = $derived([...new Set(logs.map((e) => e.model ?? ''))].sort());
  const pinnedCount = $derived(logs.filter((e) => e.pinned).length);

  function matches(e: LogEntry): boolean {
    if (status !== 'all' && e.outcome !== status) return false;
    if (instance !== 'all' && e.port !== instance) return false;
    if (model !== 'all' && (e.model ?? '') !== model) return false;
    if (onlyRetried && e.attempts.length < 2 && e.outcome !== 'retrying') return false;
    if (onlyErrors && !e.attempts.some((a) => a.reason)) return false;
    if (onlyPinned && !e.pinned) return false;
    const q = query.trim().toLowerCase().replace(/^#/, '');
    if (q) {
      const hay = `${e.path} ${e.model ?? ''} ${e.seq} ${e.port} ${e.method} ${e.client}`.toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  }

  const filtered = $derived(logs.filter(matches));
  const selected = $derived(ui.selectedLog ? (logs.find((e) => e.key === ui.selectedLog) ?? null) : null);
  const filtersActive = $derived(
    status !== 'all' || instance !== 'all' || model !== 'all' || query !== '' || onlyRetried || onlyErrors || onlyPinned,
  );

  function clearFilters() {
    status = 'all';
    instance = 'all';
    model = 'all';
    query = '';
    onlyRetried = onlyErrors = onlyPinned = false;
  }

  function atBottom() {
    if (!scroller) return true;
    return scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 6;
  }

  async function toBottom(smooth: boolean) {
    await tick();
    scroller?.scrollTo({ top: scroller.scrollHeight, behavior: smooth && !ui.reducedMotion ? 'smooth' : 'auto' });
  }

  // 新条目到达：跟随时滚到底；没跟随时累计成「N 条新日志」胶囊
  let seenArrivals = store.arrivals;
  $effect(() => {
    const n = store.arrivals;
    const delta = n - seenArrivals;
    seenArrivals = n;
    if (delta <= 0) return;
    if (follow) toBottom(true);
    else newCount += delta;
  });

  let lastTop = 0;
  function onScroll() {
    if (!scroller) return;
    // 只有「往上翻」才算用户想停下来看：程序平滑滚到底的过程中也会触发 scroll，但方向总是向下
    const top = scroller.scrollTop;
    if (follow && top < lastTop - 2 && !atBottom()) follow = false;
    lastTop = top;
    if (atBottom()) newCount = 0;
  }

  function resume() {
    follow = true;
    newCount = 0;
    toBottom(true);
  }

  function onFollowChange(v: boolean) {
    if (v) resume();
  }

  // 首次进入：有预选条目（从总览、命令面板跳来）就滚到它，否则停在最新处
  onMount(async () => {
    await tick();
    const key = ui.selectedLog;
    const row = key ? scroller?.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`) : null;
    if (row) {
      follow = false;
      row.scrollIntoView({ block: 'center' });
    } else {
      toBottom(false);
    }
  });

  function resultText(e: LogEntry): string {
    switch (e.outcome) {
      case 'rescued':
        return t('logs.filter.rescued');
      case 'retrying':
        return t('logs.status.retrying');
      case 'streaming':
        return t('logs.status.streaming');
      case 'failed':
        return t('logs.status.failed');
      default:
        return String(e.attempts[0]?.status ?? 200);
    }
  }

  function select(e: LogEntry) {
    ui.selectedLog = ui.selectedLog === e.key ? null : e.key;
  }

  function step(dir: 1 | -1) {
    if (!selected) return;
    const idx = filtered.findIndex((e) => e.key === selected.key);
    const next = filtered[idx + dir];
    if (!next) return;
    ui.selectedLog = next.key;
    tick().then(() =>
      scroller?.querySelector<HTMLElement>(`[data-key="${CSS.escape(next.key)}"]`)?.scrollIntoView({ block: 'nearest' }),
    );
  }

  async function pin(e: LogEntry) {
    const next = !e.pinned;
    await store.setPinned(e, next);
    overlays.toast(t(next ? 'toast.pinned' : 'toast.unpinned'), next ? 'lilac' : 'mint');
  }

  function onKey(ev: KeyboardEvent) {
    if (!selected || (ev.target as HTMLElement).closest('input, textarea')) return;
    if (ev.key === 'ArrowDown' || ev.key === 'j') step(1);
    else if (ev.key === 'ArrowUp' || ev.key === 'k') step(-1);
    else if (ev.key === 'Escape') ui.selectedLog = null;
    else return;
    ev.preventDefault();
  }

  const statusOptions = $derived([
    { value: 'all' as const, label: `${t('logs.filter.all')} ${fmtNum(counts.all)}` },
    { value: 'rescued' as const, label: `${t('logs.filter.rescued')} ${counts.rescued}`, icon: 'umbrella' as const },
    { value: 'retrying' as const, label: `${t('logs.filter.retrying')} ${counts.retrying}` },
    { value: 'failed' as const, label: `${t('logs.filter.failed')} ${counts.failed}` },
    { value: 'ok' as const, label: `${t('logs.filter.ok')} ${fmtNum(counts.ok)}` },
  ]);

  const instanceOptions = $derived([
    { value: 'all' as number | 'all', label: t('logs.allInstances') },
    ...store.instances.map((i) => ({ value: i.port as number | 'all', label: `${i.port}`, hint: i.alias ?? undefined })),
  ]);

  const modelOptions = $derived([
    { value: 'all', label: t('logs.allModels') },
    ...models.map((m) => ({ value: m, label: m || t('logs.noModel') })),
  ]);
</script>

<svelte:window onkeydown={onKey} />

<div class="logs">
  <div class="toolbar">
    <div class="line">
      <label class="search">
        <Icon name="search" size={15} stroke={2.4} />
        <input bind:value={query} placeholder={t('logs.search')} aria-label={t('logs.search')} />
        {#if query}
          <button class="clear" onclick={() => (query = '')} aria-label={t('logs.clearFilters')}><Icon name="x" size={12} stroke={2.8} /></button>
        {/if}
      </label>
      <Segmented options={statusOptions} bind:value={status} label={t('logs.statusFilter')} size="sm" />
    </div>
    <div class="line">
      <Dropdown options={instanceOptions} bind:value={instance} label={t('logs.col.instance')} icon="server" />
      <Dropdown options={modelOptions} bind:value={model} label={t('logs.col.model')} icon="sparkles" />
      <button class="toggle" class:on={onlyRetried} aria-pressed={onlyRetried} onclick={() => (onlyRetried = !onlyRetried)}>
        <Icon name="repeat" size={14} stroke={2.3} />{t('logs.onlyRetried')}
      </button>
      <button class="toggle" class:on={onlyErrors} aria-pressed={onlyErrors} onclick={() => (onlyErrors = !onlyErrors)}>
        <Icon name="alert" size={14} stroke={2.3} />{t('logs.onlyErrors')}
      </button>
      <button class="toggle" class:on={onlyPinned} aria-pressed={onlyPinned} onclick={() => (onlyPinned = !onlyPinned)}>
        <Icon name="pin" size={14} stroke={2.3} />{t('logs.pinned')}<span class="n">{pinnedCount}</span>
      </button>
      <span class="grow"></span>
      <span class="count num">{t('logs.count', { shown: fmtNum(filtered.length), total: fmtNum(logs.length) })}</span>
      <span class="follow">
        <span class="follow-label" class:live={follow}>
          <span class="dot" class:mint={follow} class:gray={!follow} class:breathe={follow}></span>{t('logs.follow')}
        </span>
        <Switch bind:checked={follow} label={t('logs.follow')} onchange={onFollowChange} />
      </span>
    </div>
  </div>

  <div class="table" role="table" aria-label={t('nav.logs')}>
    <div class="thead" role="row">
      <span role="columnheader" class="c-pin"><span class="sr-only">{t('logs.pin')}</span></span>
      <span role="columnheader">{t('logs.col.time')}</span>
      <span role="columnheader">{t('logs.col.request')}</span>
      <span role="columnheader">{t('logs.col.model')}</span>
      <span role="columnheader">{t('logs.col.result')}</span>
      <span role="columnheader" class="r">{t('logs.col.duration')}</span>
      <span role="columnheader" class="r">{t('logs.col.size')}</span>
      <span role="columnheader">{t('logs.col.id')}</span>
      <span role="columnheader">{t('logs.col.instance')}</span>
    </div>

    <div class="scroller" bind:this={scroller} onscroll={onScroll} role="rowgroup">
      {#each filtered as e (e.key)}
        <div
          class="row o-{e.outcome}"
          class:selected={selected?.key === e.key}
          class:fresh={e.fresh}
          class:pinned={e.pinned}
          role="row"
          data-key={e.key}
          in:slideIn={{ enabled: e.fresh }}
        >
          <button class="hit" onclick={() => select(e)} aria-label={t('logs.rowLabel', { method: e.method, path: e.path, result: resultText(e) })}></button>
          <span class="c-pin" role="cell">
            <button class="pin" class:on={e.pinned} onclick={() => pin(e)} aria-label={e.pinned ? t('logs.unpin') : t('logs.pin')} aria-pressed={e.pinned}>
              <Icon name="pin" size={14} stroke={2.2} />
            </button>
          </span>
          <span class="time mono" role="cell">{clock(e.time)}</span>
          <span class="req" role="cell">
            <span class="method m-{e.method}">{e.method}</span>
            <span class="path mono">{e.path}</span>
          </span>
          <span class="model" role="cell">{e.model ?? '—'}</span>
          <span role="cell"><ResultChip entry={e} /></span>
          <span class="r num dur" role="cell">{durShort(e.durationMs)}</span>
          <span class="r num size" role="cell">{e.respBytes ? bytes(e.respBytes) : '—'}</span>
          <span class="seq mono" role="cell">#{e.seq}</span>
          <span role="cell"><span class="port">{e.port}</span></span>
        </div>
      {:else}
        <div class="empty" in:pop>
          {#if logs.length === 0}
            <!-- 真正的空状态才放吉祥物；日志页信息密集，所以它是静止的（still） -->
            {#if mascots.current}<MascotView mood="sleeping" size="hero" px={96} still />{/if}
            <strong>{t('logs.noTraffic')}</strong>
            <span>{t('logs.noTrafficHint')}</span>
          {:else}
            <span class="no-match" aria-hidden="true"><Icon name="filter" size={22} stroke={2.2} /></span>
            <strong>{t('logs.empty')}</strong>
            <span>{t('logs.emptyHint')}</span>
            {#if filtersActive}<button class="btn sm" onclick={clearFilters}>{t('logs.clearFilters')}</button>{/if}
          {/if}
        </div>
      {/each}
    </div>

    {#if newCount > 0 && !follow}
      <button class="new-pill" onclick={resume} in:pop={{ y: 16, scale: 0.85 }} out:fadeOut>
        <Icon name="arrowDown" size={15} stroke={2.6} />{t('logs.newN', { n: newCount })}
      </button>
    {/if}

    {#if selected}
      {#key selected.key}
        <LogDetail entry={selected} onclose={() => (ui.selectedLog = null)} onstep={step} />
      {/key}
    {/if}
  </div>
</div>

<style>
  .logs {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .toolbar {
    display: grid;
    gap: 10px;
    padding: 18px 22px 12px;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .search {
    flex: 1;
    min-width: 220px;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    padding: 0 8px 0 14px;
    border-radius: var(--r-pill);
    /* 搜索框是输入控件，边界与全局 .input 一致用强边，聚焦样式也一致 */
    border: var(--line-w) solid var(--line-strong);
    background: var(--card-bg);
    color: var(--text-3);
    transition:
      box-shadow var(--dur-mid) var(--ease-spring),
      border-color var(--dur-fast) linear;
  }
  .search:focus-within {
    border-color: var(--lilac-deep);
    box-shadow: 0 0 0 3.5px color-mix(in oklab, var(--lilac) 55%, transparent);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--text);
  }
  .search input::placeholder {
    color: var(--text-3);
  }
  .clear {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 0;
    background: var(--card-2-bg);
    color: var(--text-2);
  }
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 12px;
    border-radius: var(--r-pill);
    /* 关着的筛选开关是虚线空框（「可以打开」），打开后变成实心淡丁香块 + 贴身阴影 */
    border: var(--line-w) dashed var(--line-dash);
    background: transparent;
    color: var(--text-2);
    font-size: var(--fs-sm);
    font-weight: 550;
    transition:
      transform var(--dur-mid) var(--ease-spring),
      background-color var(--dur-fast) linear,
      box-shadow var(--dur-mid) var(--ease-soft);
  }
  .toggle:hover {
    border-color: var(--line-strong);
    color: var(--text);
  }
  .toggle:active {
    transform: scale(0.95);
  }
  .toggle.on {
    border-style: solid;
    border-color: transparent;
    background: var(--lilac-soft);
    color: var(--lilac-deep);
    box-shadow: var(--shadow-1);
  }
  .toggle .n {
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: var(--r-pill);
    background: var(--card-2-bg);
    font-size: 11px;
    line-height: 18px;
    text-align: center;
  }
  .grow {
    flex: 1;
  }
  .count {
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .follow {
    display: inline-flex;
    align-items: center;
    gap: 9px;
    padding-left: 12px;
    border-left: var(--line-w-thin) solid var(--line-soft);
  }
  .follow-label {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    color: var(--text-2);
    font-size: var(--fs-sm);
    font-weight: 550;
  }
  .follow-label.live {
    color: var(--mint-deep);
  }

  /* ---------- 表格 ---------- */
  .table {
    --cols: 30px 74px minmax(180px, 1fr) 142px 150px 66px 66px 68px 62px;
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    margin: 0 12px 0 12px;
    border-top: var(--line-w) solid var(--line);
  }
  .thead,
  .row {
    display: grid;
    grid-template-columns: var(--cols);
    align-items: center;
    column-gap: 10px;
    padding: 0 10px;
  }
  .thead {
    height: 34px;
    color: var(--text-3);
    font-size: var(--fs-xs);
    font-weight: 600;
    border-bottom: var(--line-w-thin) solid var(--line-soft);
  }
  .r {
    text-align: right;
  }
  .scroller {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    padding-bottom: 60px;
    scroll-padding-bottom: 60px;
  }
  .row {
    position: relative;
    height: var(--row-h);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    transition: background-color var(--dur-mid) var(--ease-soft);
  }
  .row:nth-child(even) {
    background: color-mix(in oklab, var(--card-2) 45%, transparent);
  }
  .row:hover {
    background: var(--hover);
  }
  .row.o-retrying {
    background: color-mix(in oklab, var(--sky-soft) 75%, transparent);
  }
  .row.selected {
    background: var(--lilac-soft);
    box-shadow: inset 3px 0 0 var(--lilac-deep);
  }
  /* 新到的条目：奶黄底色闪一下再褪去 */
  .row.fresh {
    animation: fresh 1.6s var(--ease-soft);
  }
  @keyframes fresh {
    0%, 30% { background: var(--butter-soft); }
    100% { background: transparent; }
  }
  .row > * {
    min-width: 0;
    position: relative;
    pointer-events: none;
  }
  .row > .hit {
    position: absolute;
    inset: 0;
    z-index: 0;
    pointer-events: auto;
    border: 0;
    border-radius: inherit;
    background: transparent;
  }
  .row .pin {
    pointer-events: auto;
  }
  .c-pin {
    display: flex;
    justify-content: center;
  }
  .pin {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 0;
    background: transparent;
    color: var(--text-3);
    opacity: 0;
    transition:
      opacity var(--dur-fast) linear,
      transform var(--dur-mid) var(--ease-spring);
  }
  .row:hover .pin,
  .pin:focus-visible {
    opacity: 1;
  }
  .pin:hover {
    background: var(--card-bg);
    color: var(--text);
  }
  .pin.on {
    opacity: 1;
    color: var(--pink-deep);
    transform: rotate(-20deg);
  }
  .pin.on :global(svg) {
    fill: var(--pink);
  }
  .time {
    color: var(--text-2);
  }
  .req {
    display: flex;
    align-items: center;
    gap: 8px;
    overflow: hidden;
  }
  .method {
    flex: none;
    padding: 2px 6px;
    border-radius: 6px;
    background: var(--lilac-soft);
    color: var(--lilac-deep);
    font-size: 10.5px;
    font-weight: 650;
    letter-spacing: 0.02em;
  }
  .method.m-GET {
    background: var(--mint-soft);
    color: var(--mint-deep);
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
  .model {
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dur,
  .size {
    color: var(--text-2);
  }
  .seq {
    padding-left: 6px;
    color: var(--text-3);
  }
  .port {
    padding: 2px 7px;
    border-radius: var(--r-pill);
    border: var(--line-w-thin) solid var(--line-soft);
    color: var(--text-2);
    font-size: var(--fs-xs);
    font-weight: 550;
  }
  .empty {
    display: grid;
    justify-items: center;
    gap: 6px;
    padding: 64px 0;
    color: var(--text-2);
    text-align: center;
  }
  .no-match {
    display: grid;
    place-items: center;
    width: 52px;
    height: 52px;
    border-radius: 50%;
    background: var(--lilac-soft);
    color: var(--lilac-deep);
  }
  .empty strong {
    margin-top: 8px;
    color: var(--text);
    font-size: var(--fs-lg);
  }
  .empty .btn {
    margin-top: 10px;
  }

  .new-pill {
    position: absolute;
    left: 50%;
    bottom: 18px;
    z-index: 5;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 36px;
    padding: 0 18px 0 14px;
    border-radius: var(--r-pill);
    border: 0;
    background: var(--butter);
    color: var(--on-fill);
    font-weight: 600;
    /* 浮在表格上的提示：一层通用浮层阴影托起，再叠一圈奶黄柔光让它像在「发亮」 */
    box-shadow:
      var(--shadow-2),
      0 6px 18px -6px color-mix(in oklab, var(--butter) 90%, transparent);
    translate: -50% 0;
    transition: transform var(--dur-mid) var(--ease-spring);
  }
  .new-pill:hover {
    transform: translateY(-2px);
  }
  .new-pill:active {
    transform: translateY(1px) scale(0.97);
  }
</style>

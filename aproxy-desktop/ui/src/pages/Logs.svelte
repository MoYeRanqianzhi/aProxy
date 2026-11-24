<!--
  请求日志：一条请求一行的结构化列表，点开在右侧抽屉里看详情。

  数据怎么来（契约第 2 节的翻页规矩）：
  - 第一页（query_logs 不带游标）只读各实例内存里最近的那些，最多 100 条；更早的用它给的游标往前翻，
    游标只能用一次，再取一次第一页就把之前的游标作废。
  - 跟随时，请求一变（requests-changed）就重取第一页，之前翻出来的更早的页一并丢掉——它们的游标已经作废了。
    不跟随时（用户往上翻着看），不动列表，只亮出「有新的变化」，点了才刷新：免得正看着的行被换掉。
  - 列表按时间从早到晚往下排，最新的在底部（跟随时停在底部）。

  为将来的虚拟滚动留路：每一行高度固定为 --row-h，任何内容都不能把行撑高；列表只有一个滚动容器（.scroller），
  表头在它外面；详情放在抽屉里而不是插在行之间，选中某行不会改变其它行的位置。
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { t, clock, durShort, bytes, fmtNum } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { refKey, sameRef, store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import type { Bookmark, Outcome, Ref, Row, Run, Token } from '../lib/backend/contract';
  import { toFailure, type Failure } from '../lib/backend/link';
  import { faultText } from '../lib/faults';
  import { aliasOf, durationOf, rescued, retrying } from '../lib/view';
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

  /** 一页取多少条：契约允许 1 到 100 */
  const PAGE = 100;
  /** 新到的行高亮多久 */
  const FRESH_MS = 1_600;

  let status = $state<StatusFilter>('all');
  let instance = $state<number | 'all'>(ui.logInstanceFilter);
  let model = $state<string>('all');
  let query = $state('');
  let onlyRetried = $state(false);
  let onlyErrors = $state(false);
  let onlyPinned = $state(false);
  let follow = $state(true);
  /** 不跟随时攒下的变化：只说「有新的」，不说几条（一次通知可能是一条新请求，也可能是一条旧请求的进展） */
  let stale = $state(false);
  let scroller: HTMLDivElement | undefined = $state();

  ui.logInstanceFilter = 'all';

  // ---------------------------------------------------------------------------
  // 取数据
  // ---------------------------------------------------------------------------

  /** 已经取到的行，新的在前（契约给的顺序）；显示时倒过来 */
  let rows = $state.raw<Row[]>([]);
  let cursor = $state<Token | null>(null);
  let loaded = $state(false);
  let loadingOlder = $state(false);
  let fault = $state.raw<Failure | null>(null);
  /** 翻页时没能读全的运行 */
  let missing = $state(0);
  /** 新到的行高亮到什么时候（界面时钟） */
  let freshUntil = $state.raw<ReadonlyMap<string, number>>(new Map());

  /** 筛选的实例这次运行；「全部实例」是 null。卡片每次重读都是新对象，所以按键比较 */
  const scopeCard = $derived(instance === 'all' ? null : store.card(instance));
  const scopeKey = $derived(instance === 'all' ? 'all' : scopeCard?.run ? `${scopeCard.run.port}/${scopeCard.run.instanceId}` : 'none');
  /** 这个范围里的请求变过几次 */
  const scopeChanges = $derived(instance === 'all' ? store.changes : store.revision(scopeCard?.run ?? null));

  /** 当前请求的编号：范围换了、或者又取了一次第一页之后，之前还没回来的结果作废 */
  let fetchId = 0;

  async function loadHead() {
    const id = ++fetchId;
    const run: Run | null = instance === 'all' ? null : (scopeCard?.run ?? null);
    if (instance !== 'all' && !run) {
      rows = [];
      cursor = null;
      loaded = true;
      return;
    }
    try {
      const page = await store.queryLogs(run, null, PAGE);
      if (id !== fetchId) return;
      // 第一次加载不高亮；之后出现的新行亮一下
      if (loaded) {
        const known = new Set(rows.map((r) => refKey(r.ref)));
        const until = store.now + FRESH_MS;
        const next = new Map([...freshUntil].filter(([, at]) => at > store.now));
        for (const r of page.entries) if (!known.has(refKey(r.ref))) next.set(refKey(r.ref), until);
        freshUntil = next;
      }
      rows = page.entries;
      cursor = page.next;
      missing = page.missing.length;
      fault = null;
      stale = false;
      const first = !loaded;
      loaded = true;
      if (first) await scrollToStart();
      else if (follow) await toBottom(true);
    } catch (raw) {
      if (id === fetchId) {
        fault = toFailure(raw);
        loaded = true;
      }
    }
  }

  async function loadOlder() {
    if (!cursor || loadingOlder) return;
    loadingOlder = true;
    const id = fetchId;
    const run: Run | null = instance === 'all' ? null : (scopeCard?.run ?? null);
    // 往前翻会把更早的行加在列表顶上：记住离底部多远，加完之后保持用户看着的那几行不动
    const fromBottom = scroller ? scroller.scrollHeight - scroller.scrollTop : 0;
    try {
      const page = await store.queryLogs(run, cursor, PAGE);
      if (id !== fetchId) return;
      const have = new Set(rows.map((r) => refKey(r.ref)));
      rows = [...rows, ...page.entries.filter((r) => !have.has(refKey(r.ref)))];
      cursor = page.next;
      missing = page.missing.length;
      await tick();
      if (scroller) scroller.scrollTop = scroller.scrollHeight - fromBottom;
    } catch (raw) {
      if (id !== fetchId) return;
      const f = toFailure(raw);
      // 游标过期了（又取过一次第一页、或者通道重新接上了）：从第一页重新来
      if (f.code === 'desktop_cursor') void loadHead();
      else overlays.toast(faultText(f), 'coral');
    } finally {
      loadingOlder = false;
    }
  }

  // 范围换了、通道重新接上：从第一页重新取，之前的一切作废
  $effect(() => {
    void scopeKey;
    void store.epoch;
    untrack(() => {
      rows = [];
      cursor = null;
      loaded = false;
      void loadHead();
    });
  });

  // 这个范围里的请求变了：跟随时重取第一页，否则只亮出提示
  let seenChanges = untrack(() => scopeChanges);
  $effect(() => {
    const n = scopeChanges;
    if (n === seenChanges) return;
    seenChanges = n;
    untrack(() => {
      if (follow) void loadHead();
      else stale = true;
    });
  });

  // ---------------------------------------------------------------------------
  // 筛选与显示
  // ---------------------------------------------------------------------------

  function failed(r: Row): boolean {
    return r.outcome !== 'ok' && r.outcome !== 'in-flight';
  }

  const counts = $derived.by(() => {
    const c = { all: 0, rescued: 0, retrying: 0, failed: 0, ok: 0 };
    for (const r of rows) {
      c.all += 1;
      if (rescued(r)) c.rescued += 1;
      else if (retrying(r)) c.retrying += 1;
      else if (failed(r)) c.failed += 1;
      else if (r.outcome === 'ok') c.ok += 1;
    }
    return c;
  });

  const models = $derived([...new Set(rows.map((r) => r.model ?? ''))].sort());

  function matches(r: Row): boolean {
    if (status === 'rescued' && !rescued(r)) return false;
    if (status === 'retrying' && !retrying(r)) return false;
    if (status === 'failed' && !failed(r)) return false;
    if (status === 'ok' && !(r.outcome === 'ok' && r.attempts <= 1)) return false;
    if (model !== 'all' && (r.model ?? '') !== model) return false;
    if (onlyRetried && r.attempts < 2 && !retrying(r)) return false;
    if (onlyErrors && r.code === null && !r.tries.some((x) => x.code !== null)) return false;
    const q = query.trim().toLowerCase().replace(/^#/, '');
    if (q) {
      const hay = `${r.path} ${r.model ?? ''} ${r.seq} ${r.ref.run.port} ${r.method} ${r.userAgent ?? ''}`.toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  }

  /** 一行：一般是取到的请求；只看已固定时也可能是一条只剩摘要的置顶（原记录可能已经被清理了） */
  type Item = { key: string; ref: Ref; row: Row | null; mark: Bookmark | null };

  const items = $derived.by((): Item[] => {
    if (onlyPinned) {
      // 置顶是新的在前，显示时和列表一样从早到晚
      return store.bookmarks
        .filter((b) => instance === 'all' || b.ref.run.port === instance)
        .map((b) => ({ key: refKey(b.ref), ref: b.ref, row: rows.find((r) => sameRef(r.ref, b.ref)) ?? null, mark: b }))
        .filter((it) => !it.row || matches(it.row))
        .sort((a, b) => (a.row?.startedAt ?? a.mark!.startedAt) - (b.row?.startedAt ?? b.mark!.startedAt));
    }
    return rows
      .filter(matches)
      .map((r) => ({ key: refKey(r.ref), ref: r.ref, row: r, mark: null }))
      .reverse();
  });

  const selectedKey = $derived(ui.selectedRef ? refKey(ui.selectedRef) : null);
  const selectedRow = $derived(ui.selectedRef ? (rows.find((r) => sameRef(r.ref, ui.selectedRef)) ?? null) : null);
  const filtersActive = $derived(status !== 'all' || instance !== 'all' || model !== 'all' || query !== '' || onlyRetried || onlyErrors || onlyPinned);

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

  let lastTop = 0;
  /** 用户最近一次自己动列表（滚轮、触摸、按下滚动条、按键）的时刻 */
  let userAt = 0;
  function onUser() {
    userAt = Date.now();
  }
  // 只是旁听用户有没有动列表，不处理这些事件本身，所以挂成被动监听而不是元素上的交互处理
  $effect(() => {
    const el = scroller;
    if (!el) return;
    const kinds = ['wheel', 'touchmove', 'pointerdown', 'keydown'] as const;
    for (const k of kinds) el.addEventListener(k, onUser, { passive: true });
    return () => kinds.forEach((k) => el.removeEventListener(k, onUser));
  });
  function onScroll() {
    if (!scroller) return;
    // 只有用户自己「往上翻」才算想停下来看。列表刷新时顶上的旧行被拿掉、底部的新行还在做进入动画，
    // 浏览器会把滚动位置夹回去，也是一次向上的 scroll，但那不是用户干的；程序平滑滚到底的方向总是向下
    const top = scroller.scrollTop;
    if (follow && top < lastTop - 2 && !atBottom() && Date.now() - userAt < 800) follow = false;
    lastTop = top;
  }

  function resume() {
    follow = true;
    if (stale) void loadHead();
    else void toBottom(true);
  }

  function onFollowChange(v: boolean) {
    if (v) resume();
  }

  /** 第一次取到数据：有预选的那条（从总览、命令面板跳来）就滚到它，否则停在最新处 */
  async function scrollToStart() {
    await tick();
    const key = selectedKey;
    const el = key ? scroller?.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`) : null;
    if (el) {
      follow = false;
      el.scrollIntoView({ block: 'center' });
    } else {
      void toBottom(false);
    }
  }

  const OUTCOME_KEY: Record<Outcome, 'logs.status.inflight' | 'd.success' | 'logs.status.failed' | 'logs.status.clientGone'> = {
    'in-flight': 'logs.status.inflight',
    ok: 'd.success',
    'upstream-error': 'logs.status.failed',
    'aproxy-error': 'logs.status.failed',
    'format-response': 'logs.status.failed',
    'client-gone': 'logs.status.clientGone',
    unknown: 'logs.status.failed',
  };

  function resultText(it: Item): string {
    const r = it.row;
    if (!r) return t(OUTCOME_KEY[it.mark!.outcome]);
    if (rescued(r)) return t('logs.filter.rescued');
    if (retrying(r)) return t('logs.status.retrying');
    return r.clientStatus !== null ? String(r.clientStatus) : t(OUTCOME_KEY[r.outcome]);
  }

  function select(it: Item) {
    ui.selectedRef = selectedKey === it.key ? null : it.ref;
  }

  function step(dir: 1 | -1) {
    if (!selectedKey) return;
    const idx = items.findIndex((it) => it.key === selectedKey);
    const next = items[idx + dir];
    if (!next) return;
    ui.selectedRef = next.ref;
    tick().then(() => scroller?.querySelector<HTMLElement>(`[data-key="${CSS.escape(next.key)}"]`)?.scrollIntoView({ block: 'nearest' }));
  }

  async function pin(ref: Ref) {
    const next = !store.pinned(ref);
    try {
      await store.setPinned(ref, next);
      overlays.toast(t(next ? 'toast.pinned' : 'toast.unpinned'), next ? 'lilac' : 'mint');
    } catch (raw) {
      overlays.toast(faultText(toFailure(raw)), 'coral');
    }
  }

  function onKey(ev: KeyboardEvent) {
    if (!selectedKey || (ev.target as HTMLElement).closest('input, textarea')) return;
    if (ev.key === 'ArrowDown' || ev.key === 'j') step(1);
    else if (ev.key === 'ArrowUp' || ev.key === 'k') step(-1);
    else if (ev.key === 'Escape') ui.selectedRef = null;
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
    ...store.cards.map((c) => ({ value: c.port as number | 'all', label: `${c.port}`, hint: aliasOf(c) ?? undefined })),
  ]);

  const modelOptions = $derived([{ value: 'all', label: t('logs.allModels') }, ...models.map((m) => ({ value: m, label: m || t('logs.noModel') }))]);

  /** 筛选的实例为什么没有请求可看 */
  const scopeNote = $derived.by(() => {
    if (instance === 'all' || !store.connected) return null;
    if (!scopeCard) return t('inst.notFound', { port: instance });
    if (scopeCard.historyUnavailable === 'source-redaction-missing') return t('card.oldVersionHint');
    if (!scopeCard.run || !scopeCard.reads.historyFeed) return t('logs.noHistory');
    return null;
  });
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
        <Icon name="pin" size={14} stroke={2.3} />{t('logs.pinned')}<span class="n">{store.bookmarks.length}</span>
      </button>
      <span class="grow"></span>
      <span class="count num">{t('logs.count', { shown: fmtNum(items.length), total: fmtNum(rows.length) })}</span>
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
      <span role="columnheader" class="r c-size">{t('logs.col.size')}</span>
      <span role="columnheader" class="c-id">{t('logs.col.id')}</span>
      <span role="columnheader" class="c-inst">{t('logs.col.instance')}</span>
    </div>

    <div class="scroller" bind:this={scroller} onscroll={onScroll} role="rowgroup">
      {#if fault}
        <p class="list-note err"><Icon name="alert" size={14} />{faultText(fault)}<button class="btn ghost sm" onclick={loadHead}>{t('body.reload')}</button></p>
      {/if}
      {#if !onlyPinned && cursor && items.length > 0}
        <div class="older">
          <button class="btn ghost sm" disabled={loadingOlder} onclick={loadOlder}>
            <Icon name="chevronUp" size={14} stroke={2.4} />{loadingOlder ? t('d.loading') : t('logs.older')}
          </button>
        </div>
      {/if}
      {#if missing > 0}
        <p class="list-note"><Icon name="info" size={14} />{t('logs.missing', { n: missing })}</p>
      {/if}
      {#each items as it (it.key)}
        {@const r = it.row}
        {@const isPinned = store.pinned(it.ref)}
        {@const fresh = (freshUntil.get(it.key) ?? 0) > store.now}
        <div
          class="row o-{r ? r.outcome : it.mark!.outcome}"
          class:selected={selectedKey === it.key}
          class:fresh
          class:pinned={isPinned}
          role="row"
          data-key={it.key}
          in:slideIn={{ enabled: fresh }}
        >
          <button class="hit" onclick={() => select(it)} aria-label={t('logs.rowLabel', { method: r?.method ?? it.mark!.method, path: r?.path ?? it.mark!.path, result: resultText(it) })}></button>
          <span class="c-pin" role="cell">
            <button class="pin" class:on={isPinned} onclick={() => pin(it.ref)} aria-label={isPinned ? t('logs.unpin') : t('logs.pin')} aria-pressed={isPinned}>
              <Icon name="pin" size={14} stroke={2.2} />
            </button>
          </span>
          <span class="time mono" role="cell">{clock(r?.startedAt ?? it.mark!.startedAt)}</span>
          <span class="req" role="cell">
            <span class="method m-{r?.method ?? it.mark!.method}">{r?.method ?? it.mark!.method}</span>
            <span class="path mono">{r?.path ?? it.mark!.path}</span>
          </span>
          <span class="model" role="cell">{(r ? r.model : it.mark!.model) ?? '—'}</span>
          <span role="cell">
            {#if r}<ResultChip row={r} />{:else}<span class="mark-result">{resultText(it)}{#if it.mark!.attempts > 1}<span class="times">×{it.mark!.attempts - 1}</span>{/if}</span>{/if}
          </span>
          <span class="r num dur" role="cell">{r ? durShort(durationOf(r, store.now)) : '—'}</span>
          <span class="r num size c-size" role="cell">{r?.responseBytes ? bytes(r.responseBytes) : '—'}</span>
          <span class="seq mono c-id" role="cell">{r ? `#${r.seq}` : ''}</span>
          <span class="c-inst" role="cell"><span class="port">{it.ref.run.port}</span></span>
        </div>
      {:else}
        <div class="empty" in:pop>
          {#if !loaded}
            <span>{t('d.loading')}</span>
          {:else if scopeNote}
            <span class="no-match" aria-hidden="true"><Icon name="info" size={22} stroke={2.2} /></span>
            <span>{scopeNote}</span>
          {:else if rows.length === 0 && !onlyPinned}
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

    {#if stale && !follow}
      <button class="new-pill" onclick={resume} in:pop={{ y: 16, scale: 0.85 }} out:fadeOut>
        <Icon name="arrowDown" size={15} stroke={2.6} />{t('logs.updates')}
      </button>
    {/if}

    {#if ui.selectedRef}
      {#key selectedKey}
        <LogDetail target={ui.selectedRef} row={selectedRow} onclose={() => (ui.selectedRef = null)} onstep={step} />
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
  /* 九列最少要约 940px：窗口窄时多出来的列原本被 overflow 悄悄裁掉。这里按宽度分级收列，留下的列
     始终完整可见：先收编号与实例（详情里都有），再收大小；耗时对判断重试最要紧，一直留着 */
  @media (max-width: 1200px) {
    .table {
      --cols: 30px 74px minmax(160px, 1fr) 132px 146px 66px 66px;
    }
    .c-id,
    .c-inst {
      display: none;
    }
  }
  @media (max-width: 1050px) {
    .table {
      --cols: 30px 74px minmax(140px, 1fr) 118px 140px 62px;
    }
    .c-size {
      display: none;
    }
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

  .older {
    display: flex;
    justify-content: center;
    padding: 8px 0;
  }
  .list-note {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 6px 22px;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .list-note.err {
    color: var(--coral-deep);
  }
  /* 原记录已不在列表里的置顶：只剩摘要，结果写成字 */
  .mark-result {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--text-2);
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .mark-result .times {
    color: var(--text-3);
  }
</style>

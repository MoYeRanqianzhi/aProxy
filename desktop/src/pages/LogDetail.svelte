<!--
  日志详情抽屉：从右侧弹出，覆盖在列表上（不挤压列表，行的位置不变）。
  分页签：概览（重试时间线）、请求（头与 JSON 体）、响应（可在多次尝试间切换、并排对比；SSE 逐条列出）、
  转换（format 改写前后）、原始。密钥一律是打码后的样子。
-->
<script lang="ts">
  import { t, stamp, durShort, durLong, bytes, ago } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { overlays, copyText } from '../lib/overlays.svelte';
  import type { Attempt, LogEntry, RequestDetail } from '../lib/types';
  import { drawer, fadeOut, pop } from '../lib/motion';
  import Icon from '../components/Icon.svelte';
  import ResultChip from '../components/ResultChip.svelte';
  import JsonView from '../components/JsonView.svelte';

  let {
    entry,
    onclose,
    onstep,
  }: { entry: LogEntry; onclose: () => void; onstep: (dir: 1 | -1) => void } = $props();

  type Tab = 'overview' | 'request' | 'response' | 'transform' | 'raw';
  const tabs: Tab[] = ['overview', 'request', 'response', 'transform', 'raw'];

  let tab = $state<Tab>('overview');
  // 默认看最后一次尝试（成功的那次或最近的失败）；打开详情后固定，不随新尝试跳走。
  // 父组件按条目 key 重建本组件，所以只需在创建时取一次
  const lastIndex = () => Math.max(0, entry.attempts.length - 1);
  let attemptIdx = $state(lastIndex());
  let compare = $state(false);
  let compareIdx = $state(0);
  let openEvent = $state<number | null>(null);
  let copyOpen = $state(false);
  let copyWrap: HTMLDivElement | undefined = $state();
  let tabEls: HTMLButtonElement[] = $state([]);
  let indicator = $state({ left: 0, width: 0 });

  const inst = $derived(store.instance(entry.port));
  const keys = $derived(inst?.transform?.keys ?? 1);

  // 请求与响应的正文按需取（可能很大，也是最敏感的内容）：打开时取一次；在途的请求每多一次尝试、
  // 或者结束时再取一次。新的到手之前旧的先留着，免得每拍都闪一下空白
  let detail = $state<RequestDetail | null>(null);
  let detailFailed = $state(false);
  $effect(() => {
    const key = entry.key;
    void entry.attempts.length;
    void entry.outcome;
    let alive = true;
    store.requestDetail(key).then(
      (d) => {
        if (!alive) return;
        detail = d;
        detailFailed = false;
      },
      () => {
        if (alive) detailFailed = true;
      },
    );
    return () => {
      alive = false;
    };
  });

  $effect(() => {
    const el = tabEls[tabs.indexOf(tab)];
    void ui.lang;
    if (el) indicator = { left: el.offsetLeft, width: el.offsetWidth };
  });

  // 实例卡住时只剩最后一份快照，倒计时停在快照那一刻（与实例详情页同一口径）
  const clock = $derived(inst?.status === 'unresponsive' && inst.silentSince !== null ? inst.silentSince : store.now);

  function countdown(): number {
    return Math.max(0, Math.ceil(((entry.phaseEndsAt ?? clock) - clock) / 1000));
  }

  function hearts(a: Attempt): number {
    if (!entry.stream || !inst || inst.forwardOnly || inst.keepalive.interval === 0) return 0;
    return Math.floor(a.backoffMs / (inst.keepalive.interval * 1000));
  }

  function failNote(): string | null {
    if (entry.outcome !== 'failed') return null;
    const r = entry.attempts[entry.attempts.length - 1]?.reason;
    if (r === 'notFound') return t('d.passthrough');
    if (r === 'forwardedError') return t('d.forwarded');
    if (r === 'clientGone') return t('d.clientGone');
    if (r === 'stopped') return t('reason.stopped');
    return null;
  }

  function peek(i: number) {
    attemptIdx = i;
    compare = false;
    tab = 'response';
  }

  async function pin() {
    const next = !entry.pinned;
    await store.setPinned(entry, next);
    overlays.toast(t(next ? 'toast.pinned' : 'toast.unpinned'), next ? 'lilac' : 'mint');
  }

  async function copyCurl() {
    copyOpen = false;
    if (!detail) return;
    await copyText(detail.request.curl);
    overlays.toast(t('toast.copiedCurl'));
  }

  async function copyBody() {
    copyOpen = false;
    if (!detail) return;
    await copyText(JSON.stringify(detail.request.body, null, 2));
    overlays.toast(t('toast.copiedBody'));
  }

  function onWindowPointer(ev: PointerEvent) {
    if (copyOpen && copyWrap && !copyWrap.contains(ev.target as Node)) copyOpen = false;
  }

  function onTabKey(ev: KeyboardEvent) {
    if (ev.key !== 'ArrowRight' && ev.key !== 'ArrowLeft') return;
    ev.preventDefault();
    ev.stopPropagation();
    const i = (tabs.indexOf(tab) + (ev.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length;
    tab = tabs[i];
    tabEls[i]?.focus();
  }

  const instName = $derived(inst?.alias ? `${entry.port} ${inst.alias}` : String(entry.port));
  const waitedExtra = $derived(
    entry.attempts.slice(0, -1).reduce((s, a) => s + a.durationMs + a.backoffMs, 0),
  );
</script>

{#snippet attemptPicker(current: number, set: (i: number) => void, label: string)}
  <div class="picker" role="radiogroup" aria-label={label}>
    {#each entry.attempts as a, i (a.n)}
      <button
        class="pick"
        class:on={i === current}
        class:fail={!!a.reason}
        role="radio"
        aria-checked={i === current}
        onclick={() => set(i)}
      >
        {t('d.attempt', { n: a.n })}<span class="pick-code">{a.status ?? 'ERR'}</span>
      </button>
    {/each}
  </div>
{/snippet}

{#snippet headerList(lines: { name: string; value: string; secret?: boolean }[])}
  <dl class="headers">
    {#each lines as h (h.name)}
      <dt class="mono">{h.name}</dt>
      <dd class="mono" class:secret={h.secret}>
        {#if h.secret}<Icon name="lock" size={11} stroke={2.4} />{/if}{h.value}
      </dd>
    {/each}
  </dl>
{/snippet}

{#snippet loading()}
  <p class="note">{detailFailed ? t('d.loadFailed') : t('d.loading')}</p>
{/snippet}

{#snippet response(i: number)}
  {@const a: Attempt | undefined = entry.attempts[i]}
  {@const ad = detail?.attempts[i]}
  {#if !a}
    <p class="note">{t('d.pending')}</p>
  {:else if !ad}
    {@render loading()}
  {:else if a.status === null}
    <div class="noresp">
      <Icon name="plug" size={18} />
      <div>
        <strong>{t('d.noResponse')}</strong>
        <code class="mono">{ad.transportError}</code>
      </div>
    </div>
  {:else}
    <div class="statusline">
      <span class="code-chip" class:fail={!!a.reason}>{a.status}</span>
      <span class="muted">{a.reason ? t(`reason.${a.reason}`) : t('d.success')}</span>
      <span class="muted">{durShort(a.durationMs)}</span>
      {#if a.respBytes}<span class="muted">{bytes(a.respBytes)}</span>{/if}
    </div>
    <h4>{t('d.respHeaders')}</h4>
    {@render headerList(ad.headers)}
    <h4>{t('d.body')}</h4>
    {#if a.reason}
      {@const b = ad.errorBody}
      {#if typeof b === 'string'}
        <pre class="pre mono">{b}</pre>
      {:else}
        <JsonView value={b} openDepth={3} />
      {/if}
    {:else if entry.outcome === 'streaming'}
      <p class="note">{t('logs.status.streaming')}…</p>
    {:else if ad.sse}
      {@const events = ad.sse}
      <div class="assembled">
        <span class="label">{t('d.textPreview')}</span>
        <p>
          {events
            .filter((ev) => ev.event.endsWith('delta'))
            .map((ev) => {
              const m = ev.data.match(/"(?:text|delta)":("(?:[^"\\]|\\.)*")/);
              return m ? JSON.parse(m[1]) : '';
            })
            .join('')}
        </p>
      </div>
      <div class="events-head">
        <span>{t('d.events')}</span><span class="chip">{t('d.eventsN', { n: events.length })}</span>
      </div>
      <ol class="events">
        {#if entry.heartbeats > 0}
          <li class="ev beat">
            <span class="ev-name"><Icon name="heart" size={12} stroke={2.4} />{t('d.heartbeatRow', { n: entry.heartbeats })}</span>
            <span class="ev-data">{t('d.heartbeatNote')}</span>
          </li>
        {/if}
        {#each events as ev, i (i)}
          <li class="ev" class:open={openEvent === i}>
            <button class="ev-row" onclick={() => (openEvent = openEvent === i ? null : i)} aria-expanded={openEvent === i}>
              <span class="ev-name">{ev.event}</span>
              <span class="ev-data mono">{ev.data}</span>
            </button>
            {#if openEvent === i}
              <div class="ev-json" in:pop={{ y: -4, scale: 0.99, duration: 360 }}>
                <JsonView value={JSON.parse(ev.data)} openDepth={4} />
              </div>
            {/if}
          </li>
        {/each}
      </ol>
    {:else}
      <JsonView value={ad.body} openDepth={3} />
    {/if}
  {/if}
{/snippet}

<svelte:window onpointerdown={onWindowPointer} />

<aside class="drawer" aria-label={`#${entry.seq} ${entry.path}`} in:drawer out:fadeOut={{ duration: 140 }}>
  <header class="d-head">
    <div class="d-title">
      <span class="method">{entry.method}</span>
      <code class="mono path" title={entry.path}>{entry.path}</code>
    </div>
    <div class="d-tools">
      <button class="icon-btn" class:pinned={entry.pinned} onclick={pin} aria-pressed={entry.pinned} title={entry.pinned ? t('logs.unpin') : t('logs.pin')} aria-label={entry.pinned ? t('logs.unpin') : t('logs.pin')}>
        <Icon name="pin" size={16} />
      </button>
      <button class="icon-btn" onclick={() => onstep(-1)} title={t('d.prev')} aria-label={t('d.prev')}><Icon name="chevronUp" size={16} stroke={2.4} /></button>
      <button class="icon-btn" onclick={() => onstep(1)} title={t('d.next')} aria-label={t('d.next')}><Icon name="chevronDown" size={16} stroke={2.4} /></button>
      <button class="icon-btn close" onclick={onclose} title={t('d.close')} aria-label={t('d.close')}><Icon name="x" size={16} stroke={2.4} /></button>
    </div>
    <div class="d-sub">
      <span class="mono seq">#{entry.seq}</span>
      <span class="chip">{instName}</span>
      {#if entry.model}<span class="chip lilac">{entry.model}</span>{/if}
      <ResultChip {entry} />
    </div>
    <div class="tabs" role="tablist" aria-label={`#${entry.seq}`} tabindex="-1" onkeydown={onTabKey}>
      <span class="tab-ind" style="transform: translateX({indicator.left}px); width: {indicator.width}px"></span>
      {#each tabs as tb, i (tb)}
        <button
          role="tab"
          id="tab-{tb}"
          aria-selected={tab === tb}
          aria-controls="panel-{tb}"
          tabindex={tab === tb ? 0 : -1}
          bind:this={tabEls[i]}
          onclick={() => (tab = tb)}
        >
          {t(`d.tab.${tb}`)}
          {#if tb === 'transform' && entry.transformed}<span class="tab-dot"></span>{/if}
        </button>
      {/each}
      <span class="grow"></span>
      <!-- 复制收进一个小菜单：页签一行放不下两个带字的按钮（英文尤其长） -->
      <div class="copy-wrap" bind:this={copyWrap}>
        <button class="copy" aria-haspopup="menu" aria-expanded={copyOpen} onclick={() => (copyOpen = !copyOpen)}>
          <Icon name="copy" size={14} />{t('d.copy')}<Icon name="chevronDown" size={12} stroke={2.6} />
        </button>
        {#if copyOpen}
          <div class="copy-menu" role="menu" in:pop={{ y: -6, scale: 0.96, duration: 380 }} out:fadeOut={{ duration: 100 }}>
            <button role="menuitem" onclick={copyCurl}><Icon name="terminal" size={14} />{t('d.copyCurl')}</button>
            {#if entry.method === 'POST'}
              <button role="menuitem" onclick={copyBody}><Icon name="braces" size={14} />{t('d.copyBody')}</button>
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </header>

  <div class="d-body" role="tabpanel" id="panel-{tab}" aria-labelledby="tab-{tab}">
    {#key tab}
      <div class="pane" in:pop={{ y: 6, scale: 1, duration: 380 }}>
        {#if tab === 'overview'}
          {#if entry.outcome === 'rescued'}
            <div class="banner mint">
              <Icon name="umbrella" size={18} />
              <div>
                <p>{t('d.rescuedNote', { n: entry.attempts.length, d: durLong(waitedExtra) })}</p>
                {#if entry.heartbeats > 0}<p class="sub">{t('d.heartbeats', { n: entry.heartbeats })}</p>{/if}
              </div>
            </div>
          {:else if entry.outcome === 'ok'}
            <div class="banner mint"><Icon name="check" size={18} stroke={2.4} /><p>{t('d.firstTry')}</p></div>
          {:else if failNote()}
            <div class="banner coral"><Icon name="info" size={18} /><p>{failNote()}</p></div>
          {:else if inst?.status === 'unresponsive' && entry.phase !== null}
            <!-- 实例卡住：下面的倒计时停在最后一份快照上，先说清楚这是过期数据 -->
            <div class="banner coral"><Icon name="clock" size={18} /><p>{t('inst.stale', { ago: ago(store.now - clock) })}</p></div>
          {/if}

          <h3>{t('d.timeline')}</h3>
          <ol class="timeline">
            {#each entry.attempts as a, i (a.n)}
              <li class="att" class:fail={!!a.reason} class:ok={!a.reason && entry.outcome !== 'streaming'}>
                <span class="node">{#if !a.reason && entry.outcome !== 'streaming'}<Icon name="check" size={14} stroke={3} />{:else}{a.n}{/if}</span>
                <div class="att-main">
                  <span class="code-chip" class:fail={!!a.reason}>{a.status ?? 'ERR'}</span>
                  <span class="att-reason">{a.reason ? t(`reason.${a.reason}`) : entry.outcome === 'streaming' ? t('logs.status.streaming') : t('d.success')}</span>
                  <span class="att-dur num">{durShort(a.durationMs)}</span>
                  <button class="peek" onclick={() => peek(i)}>{t('d.peek')}<Icon name="chevronRight" size={12} stroke={2.6} /></button>
                </div>
              </li>
              {#if i < entry.attempts.length - 1 || (entry.outcome === 'retrying' && entry.phase === 'waiting')}
                <li class="gap">
                  <span class="gap-pill" class:live={i === entry.attempts.length - 1}>
                    <Icon name="clock" size={12} stroke={2.4} />
                    {#if i === entry.attempts.length - 1}
                      {t('d.nextIn', { s: countdown(), n: entry.attempts.length + 1 })}
                    {:else}
                      {a.backoffMs === 0 ? t('d.immediate') : t('d.waited', { d: durLong(a.backoffMs) })}
                    {/if}
                    {#if hearts(a) > 0 && i < entry.attempts.length - 1}
                      <span class="hearts"><Icon name="heart" size={11} stroke={2.4} />×{hearts(a)}</span>
                    {/if}
                  </span>
                </li>
              {/if}
            {/each}
            {#if entry.outcome === 'retrying' && entry.phase === 'attempting'}
              <li class="att pending">
                <span class="node">{entry.attempts.length + 1}</span>
                <div class="att-main"><span class="att-reason">{t('d.trying', { n: entry.attempts.length + 1 })}</span></div>
              </li>
            {/if}
          </ol>

          <h3>{t('d.facts')}</h3>
          <dl class="facts">
            <dt>{t('d.received')}</dt>
            <dd class="mono">{stamp(entry.time)}</dd>
            <dt>{t('d.client')}</dt>
            <dd class="mono">{entry.client}</dd>
            <dt>{t('d.total')}</dt>
            <dd>{durLong(entry.durationMs)}</dd>
            <dt>{t('d.reqSize')}</dt>
            <dd>{entry.reqBytes ? bytes(entry.reqBytes) : '—'}</dd>
            <dt>{t('d.respSize')}</dt>
            <dd>{entry.respBytes ? bytes(entry.respBytes) : '—'}</dd>
            <dt>{t('d.keepalive')}</dt>
            <dd>{entry.heartbeats > 0 ? t('d.heartbeatRow', { n: entry.heartbeats }) : '—'}</dd>
            {#if entry.keyIndex}
              <dt>{t('d.keyUsed')}</dt>
              <dd>{t('d.keyN', { i: entry.keyIndex, n: keys })}</dd>
            {/if}
            <dt>{t('d.requestId')}</dt>
            <dd class="mono">#{entry.seq}</dd>
          </dl>
        {:else if tab === 'request'}
          <div class="sec-head">
            <h3>{t('d.headers')}</h3>
            <span class="chip pink"><Icon name="lock" size={11} stroke={2.4} />{t('d.secretNote')}</span>
          </div>
          {#if detail}
            {@render headerList(detail.request.headers)}
          {:else}
            {@render loading()}
          {/if}
          <h3>{t('d.body')}</h3>
          {#if entry.method === 'POST'}
            {#if detail}<JsonView value={detail.request.body} />{:else}{@render loading()}{/if}
          {:else}
            <p class="note">{t('d.noBody')}</p>
          {/if}
        {:else if tab === 'response'}
          <div class="sec-head">
            {@render attemptPicker(attemptIdx, (i) => (attemptIdx = i), t('d.whichAttempt'))}
            {#if entry.attempts.length > 1}
              <button class="toggle" class:on={compare} aria-pressed={compare} onclick={() => {
                compare = !compare;
                if (compare && compareIdx === attemptIdx) compareIdx = attemptIdx === 0 ? entry.attempts.length - 1 : 0;
              }}>
                <Icon name="compare" size={14} />{t('d.compare')}
              </button>
            {/if}
          </div>
          {#if compare}
            <div class="compare">
              <div class="col">
                {@render response(attemptIdx)}
              </div>
              <div class="col">
                {@render attemptPicker(compareIdx, (i) => (compareIdx = i), t('d.compareWith', { n: compareIdx + 1 }))}
                {@render response(compareIdx)}
              </div>
            </div>
          {:else}
            {@render response(entry.attempts[attemptIdx] ? attemptIdx : entry.attempts.length - 1)}
          {/if}
        {:else if tab === 'transform'}
          {#if !entry.transformed}
            <p class="note">{t('d.transform.none')}</p>
          {:else}
            <h3>{t('d.transform.title')}</h3>
            <ul class="changes">
              {#each detail?.transform ?? [] as c (c.field)}
                <li>
                  <div class="ch-head">
                    <code class="mono field">{c.field}</code>
                    {#if c.note === 'key'}<span class="chip lilac"><Icon name="key" size={11} stroke={2.4} />{t('d.transform.key', { i: entry.keyIndex ?? 1 })}<span class="of">/ {keys}</span></span>{/if}
                    {#if c.note === 'channel'}<span class="chip lilac"><Icon name="shuffle" size={11} stroke={2.4} />{t('d.transform.channel')}</span>{/if}
                    {#if c.note === 'model'}<span class="chip lilac">{t('d.transform.model')}</span>{/if}
                    {#if !c.before}<span class="chip mint">{t('d.transform.added')}</span>{/if}
                  </div>
                  <div class="ch-body">
                    {#if c.before}<code class="mono before" title={t('d.transform.before')}>{c.before}</code>{/if}
                    <code class="mono after" title={t('d.transform.after')}>{c.after}</code>
                  </div>
                </li>
              {/each}
            </ul>
            <p class="note"><Icon name="info" size={14} />{t('d.transform.replay')}</p>
          {/if}
        {:else}
          <div class="sec-head">
            {@render attemptPicker(attemptIdx, (i) => (attemptIdx = i), t('d.whichAttempt'))}
          </div>
          <p class="note">{t('d.raw.note')}</p>
          {#if detail}
            <pre class="pre mono raw">{detail.attempts[attemptIdx]?.raw ?? detail.request.raw}</pre>
          {:else}
            {@render loading()}
          {/if}
        {/if}
      </div>
    {/key}
  </div>
</aside>

<style>
  .drawer {
    position: absolute;
    top: 8px;
    right: 0;
    bottom: 0;
    z-index: 10;
    width: min(580px, 60%);
    display: flex;
    flex-direction: column;
    border: var(--line-w) solid var(--line);
    border-right: 0;
    border-bottom: 0;
    border-radius: var(--r-lg) 0 0 0;
    background: var(--overlay-bg);
    box-shadow: var(--shadow-3);
  }
  .d-head {
    padding: 14px 16px 0 18px;
    border-bottom: var(--line-w-thin) solid var(--line-soft);
  }
  .d-title {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
    padding-right: 150px;
  }
  .method {
    flex: none;
    padding: 2px 7px;
    border-radius: var(--r-xs);
    background: var(--lilac);
    color: var(--on-fill);
    font-size: 11px;
    font-weight: 650;
  }
  .path {
    font-size: 15px;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .d-tools {
    position: absolute;
    top: 10px;
    right: 12px;
    display: flex;
    gap: 2px;
  }
  .icon-btn {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    border: 0;
    background: transparent;
    color: var(--text-2);
    transition:
      background-color var(--dur-fast) linear,
      transform var(--dur-mid) var(--ease-spring);
  }
  .icon-btn:hover {
    background: var(--hover);
    color: var(--text);
  }
  .icon-btn:active {
    transform: scale(0.9);
  }
  .icon-btn.pinned {
    color: var(--pink-deep);
  }
  .icon-btn.pinned :global(svg) {
    fill: var(--pink);
    transform: rotate(-20deg);
  }
  .icon-btn.close:hover {
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  .d-sub {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 7px;
    margin-top: 10px;
  }
  .seq {
    color: var(--text-3);
  }
  .grow {
    flex: 1;
  }
  .tabs {
    position: relative;
    display: flex;
    gap: 4px;
    margin-top: 12px;
  }
  .tabs button {
    position: relative;
    height: 36px;
    padding: 0 12px;
    border: 0;
    background: transparent;
    color: var(--text-2);
    font-weight: 550;
  }
  .tabs button[aria-selected='true'] {
    color: var(--text);
  }
  .tabs button:hover {
    color: var(--text);
  }
  /* 选中页签下方的胖下划线，切换时弹簧滑动。
     不描边以后，纯丁香色在白底上偏淡（不到 2:1），所以往深色调里掺一点，
     让这条「你在这里」的指示对底色接近 3:1，同时仍是丁香色而不是变成深紫 */
  .tab-ind {
    position: absolute;
    left: 0;
    bottom: -1px;
    height: 4px;
    border-radius: var(--r-pill) var(--r-pill) 0 0;
    background: color-mix(in oklab, var(--lilac) 55%, var(--lilac-deep));
    transition:
      transform var(--dur-slow) var(--ease-spring),
      width var(--dur-slow) var(--ease-spring);
  }
  .tabs .copy {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    align-self: center;
    height: 28px;
    padding: 0 10px;
    border-radius: var(--r-pill);
    border: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-bg);
    color: var(--text-2);
    font-size: var(--fs-xs);
    font-weight: 600;
    transition: transform var(--dur-mid) var(--ease-spring);
  }
  .tabs .copy:hover {
    border-color: var(--line);
    color: var(--text);
    box-shadow: var(--shadow-1);
  }
  .tabs .copy:active {
    transform: scale(0.94);
  }
  .copy-wrap {
    position: relative;
    align-self: center;
  }
  .copy-menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 5;
    display: grid;
    min-width: 170px;
    padding: 5px;
    border-radius: var(--r-md);
    border: var(--line-w) solid var(--line);
    background: var(--overlay-bg);
    box-shadow: var(--shadow-2);
    transform-origin: top right;
  }
  .copy-menu button {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 550;
    text-align: left;
    white-space: nowrap;
  }
  .copy-menu button:hover {
    background: var(--hover);
  }
  .tab-dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    margin-left: 4px;
    border-radius: 50%;
    background: var(--lilac-deep);
    vertical-align: 2px;
  }
  .d-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 16px 18px 28px;
  }
  .pane {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  h3 {
    margin: 6px 0 0;
    font-size: var(--fs-md);
    font-weight: 620;
  }
  h4 {
    margin: 4px 0 0;
    font-size: var(--fs-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .sec-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    flex-wrap: wrap;
  }
  .sec-head h3 {
    margin: 0;
  }
  .note {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .muted {
    color: var(--text-3);
  }

  .banner {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    padding: 12px 14px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--banner-edge, var(--line));
  }
  .banner p {
    margin: 0;
    font-weight: 550;
  }
  .banner .sub {
    margin-top: 2px;
    font-weight: 450;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  /* 横幅的细边取自身色相：底色的深一档，而不是中性灰框 */
  .banner.mint {
    --banner-edge: color-mix(in oklab, var(--mint-soft) 55%, var(--mint));
    background: var(--mint-soft);
    color: var(--text);
  }
  .banner.mint > :global(svg) {
    color: var(--mint-deep);
    margin-top: 2px;
  }
  .banner.coral {
    --banner-edge: color-mix(in oklab, var(--coral-soft) 55%, var(--coral));
    background: var(--coral-soft);
  }
  .banner.coral > :global(svg) {
    color: var(--coral-deep);
    margin-top: 2px;
  }

  /* ---------- 时间线 ---------- */
  .timeline {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .att {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 40px;
  }
  .node {
    position: relative;
    z-index: 1;
    display: grid;
    place-items: center;
    flex: none;
    width: 30px;
    height: 30px;
    border-radius: 50%;
    /* 中性节点是白底圆圈，要靠强边认出来；彩色节点的边换成自身色的深色调（同状态点） */
    border: var(--line-w) solid var(--line-strong);
    background: var(--card);
    font-size: var(--fs-sm);
    font-weight: 650;
  }
  .att.fail .node,
  .att.ok .node,
  .att.pending .node {
    background: var(--node);
    border-color: color-mix(in oklab, var(--node) 55%, var(--text));
    color: var(--on-fill);
  }
  .att.fail .node {
    --node: var(--coral);
  }
  /* 成功那一步是整条时间线的落点：再加一圈同色柔光 */
  .att.ok .node {
    --node: var(--mint);
    box-shadow: 0 3px 10px -2px color-mix(in oklab, var(--node) 90%, transparent);
  }
  .att.pending .node {
    --node: var(--sky);
    animation: pending 1.4s ease-in-out infinite;
  }
  @keyframes pending {
    0%, 100% { box-shadow: 0 0 0 0 color-mix(in oklab, var(--sky) 70%, transparent); }
    50% { box-shadow: 0 0 0 7px color-mix(in oklab, var(--sky) 0%, transparent); }
  }
  .att-main {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    padding: 6px 6px 6px 10px;
    border-radius: var(--r-pill);
    background: var(--card-2-bg);
  }
  .att-reason {
    flex: 1;
    min-width: 0;
    font-size: var(--fs-sm);
    font-weight: 550;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .att-dur {
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .code-chip {
    display: inline-flex;
    align-items: center;
    height: 22px;
    padding: 0 8px;
    border-radius: var(--r-pill);
    background: var(--mint-soft);
    color: var(--mint-deep);
    font-size: var(--fs-sm);
    font-weight: 650;
    font-variant-numeric: tabular-nums;
  }
  .code-chip.fail {
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  .peek {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: 24px;
    padding: 0 6px 0 10px;
    border-radius: var(--r-pill);
    border: 0;
    background: transparent;
    color: var(--lilac-deep);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .peek:hover {
    background: var(--lilac-soft);
  }
  /* 两次尝试之间：竖向虚线 + 退避胶囊（等了多久、期间发了几次心跳） */
  .gap {
    position: relative;
    display: flex;
    align-items: center;
    min-height: 34px;
    padding-left: 42px;
  }
  .gap::before {
    content: '';
    position: absolute;
    left: 14px;
    top: -6px;
    bottom: -6px;
    border-left: var(--line-w) dashed var(--line-soft);
  }
  .gap-pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 24px;
    padding: 0 10px;
    border-radius: var(--r-pill);
    background: var(--sky-soft);
    color: var(--sky-deep);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .gap-pill.live {
    background: var(--sky);
    color: var(--on-fill);
    box-shadow: 0 2px 8px -2px color-mix(in oklab, var(--sky) 90%, transparent);
  }
  .hearts {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    margin-left: 4px;
    padding-left: 8px;
    border-left: 1.5px solid color-mix(in oklab, currentColor 30%, transparent);
    color: var(--pink-deep);
  }
  .hearts :global(svg) {
    fill: var(--pink);
  }

  .facts {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0 18px;
    margin: 0;
  }
  .facts dt,
  .facts dd {
    padding: 7px 0;
    border-top: var(--line-w-thin) dashed var(--line-soft);
    font-size: var(--fs-sm);
  }
  .facts dt {
    color: var(--text-3);
    white-space: nowrap;
  }
  .facts dd {
    margin: 0;
    min-width: 0;
    /* 先在空格、连字符处换行，实在放不下才在词中间断：UA 串、长值都更好读 */
    overflow-wrap: anywhere;
  }

  /* ---------- 头与体 ---------- */
  .headers {
    display: grid;
    grid-template-columns: fit-content(46%) 1fr;
    gap: 0 14px;
    margin: 0;
    padding: 8px 12px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-2-bg);
    font-size: 12.5px;
  }
  .headers dt {
    color: var(--text-3);
    padding: 3px 0;
    /* 头名多由连字符连成（anthropic-ratelimit-…），在连字符处折行比硬拆单词好读 */
    overflow-wrap: anywhere;
  }
  .of {
    opacity: 0.7;
  }
  .headers dd {
    margin: 0;
    padding: 3px 0;
    overflow-wrap: anywhere;
  }
  .headers dd.secret {
    display: flex;
    align-items: center;
    gap: 4px;
    width: fit-content;
    padding: 1px 7px;
    margin: 2px 0;
    border-radius: 6px;
    background: var(--pink-soft);
    color: var(--pink-deep);
  }
  .pre {
    margin: 0;
    padding: 12px 14px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-2-bg);
    font-size: 12.5px;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .raw {
    max-height: none;
  }
  .statusline {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: var(--fs-sm);
  }
  .noresp {
    display: flex;
    gap: 12px;
    padding: 14px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) dashed var(--coral-deep);
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  .noresp div {
    display: grid;
    gap: 4px;
  }
  .noresp code {
    color: var(--text-2);
    font-size: 12px;
    word-break: break-word;
  }

  .picker {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }
  .pick {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 4px 0 10px;
    border-radius: var(--r-pill);
    border: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-bg);
    color: var(--text-2);
    font-size: var(--fs-xs);
    font-weight: 550;
    transition: transform var(--dur-mid) var(--ease-spring);
  }
  .pick:active {
    transform: scale(0.94);
  }
  .pick-code {
    padding: 2px 6px;
    border-radius: var(--r-pill);
    background: var(--mint-soft);
    color: var(--mint-deep);
    font-weight: 650;
  }
  .pick.fail .pick-code {
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  .pick.on {
    border-color: transparent;
    background: var(--lilac-soft);
    color: var(--text);
    box-shadow: var(--shadow-1);
  }
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px;
    border-radius: var(--r-pill);
    /* 与日志页的筛选开关同一套：关着是虚线空框，打开是实心淡丁香块 */
    border: var(--line-w) dashed var(--line-dash);
    background: transparent;
    color: var(--text-2);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .toggle:hover {
    border-color: var(--line-strong);
    color: var(--text);
  }
  .toggle.on {
    border-style: solid;
    border-color: transparent;
    background: var(--lilac-soft);
    color: var(--lilac-deep);
    box-shadow: var(--shadow-1);
  }
  .compare {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }
  .col + .col {
    padding-left: 14px;
    border-left: var(--line-w-thin) dashed var(--line-soft);
  }

  .assembled {
    padding: 10px 14px;
    border-radius: var(--r-md);
    background: var(--lilac-soft);
  }
  .assembled .label {
    color: var(--lilac-deep);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .assembled p {
    margin: 4px 0 0;
  }
  .events-head {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .events {
    list-style: none;
    margin: 0;
    padding: 0;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line-soft);
    overflow: hidden;
  }
  .ev + .ev {
    border-top: var(--line-w-thin) solid var(--line-soft);
  }
  .ev-row,
  .ev.beat {
    display: grid;
    grid-template-columns: 170px 1fr;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 32px;
    padding: 0 12px;
    border: 0;
    background: transparent;
    text-align: left;
    font-size: 12px;
  }
  .ev-row:hover {
    background: var(--hover);
  }
  .ev.beat {
    background: var(--pink-soft);
    color: var(--pink-deep);
  }
  .ev-name {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-weight: 600;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ev.beat .ev-name {
    color: var(--pink-deep);
  }
  .ev.beat :global(svg) {
    fill: var(--pink);
  }
  .ev-data {
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ev-json {
    padding: 0 10px 10px;
  }

  .changes {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 10px;
  }
  .changes li {
    padding: 10px 12px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-2-bg);
  }
  .ch-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }
  .field {
    font-weight: 600;
  }
  .ch-body {
    display: grid;
    gap: 5px;
  }
  .before,
  .after {
    display: block;
    padding: 4px 10px;
    border-radius: var(--r-sm);
    font-size: 12px;
    word-break: break-all;
  }
  .before {
    background: var(--coral-soft);
    color: var(--coral-deep);
    text-decoration: line-through;
    text-decoration-color: color-mix(in oklab, currentColor 50%, transparent);
  }
  .before::before {
    content: '− ';
  }
  .after {
    background: var(--mint-soft);
    color: var(--mint-deep);
  }
  .after::before {
    content: '+ ';
  }
</style>

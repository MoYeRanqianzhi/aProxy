<!--
  请求详情抽屉：从右侧弹出，覆盖在列表上（不挤压列表，行的位置不变）。
  分页签：概览（每一次尝试的时间线与这条请求的事实）、请求（客户端发来的头与体）、
  响应（每一次尝试发出去的与收到的，可以并排对比）、format（format 程序报告自己做了什么）。

  打开时向 Rust 要一次详情（request_detail），这是一次「选择」；在途的请求变了（列表里那一行的 revision 或结局变了）
  就重新选一次，拿到新的之前旧的先留着。体按块另取（BodyView），关掉抽屉时取消这次选择，Rust 就不再交数据。
  密钥一律是打码后的样子：头里的密钥只给一个标记，体里的密钥已被等长的 * 盖住。
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { t, stamp, durShort, durLong, bytes } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { overlays, copyText } from '../lib/overlays.svelte';
  import type { Action, Detail, Header, Ref, Report, Row, Selection, Try, TryDetail } from '../lib/backend/contract';
  import { toFailure, type Failure } from '../lib/backend/link';
  import { codeText, faultText } from '../lib/faults';
  import { aliasOf, durationOf, rescued } from '../lib/view';
  import { drawer, fadeOut, pop } from '../lib/motion';
  import Icon from '../components/Icon.svelte';
  import ResultChip from '../components/ResultChip.svelte';
  import BodyView from '../components/BodyView.svelte';

  let {
    target,
    row,
    onclose,
    onstep,
  }: { target: Ref; row: Row | null; onclose: () => void; onstep: (dir: 1 | -1) => void } = $props();

  type Tab = 'overview' | 'request' | 'response' | 'format';
  const tabs: Tab[] = ['overview', 'request', 'response', 'format'];

  let tab = $state<Tab>('overview');
  let compare = $state(false);
  /** 看的是第几次尝试（尝试的编号，不是下标）；null 是最后一次 */
  let attemptNo = $state<number | null>(null);
  let compareNo = $state<number | null>(null);
  let copyOpen = $state(false);
  let copyWrap: HTMLDivElement | undefined = $state();
  let tabEls: HTMLButtonElement[] = $state([]);
  let indicator = $state({ left: 0, width: 0 });

  let detail = $state.raw<Detail | null>(null);
  let selection = $state.raw<Selection | null>(null);
  let failure = $state.raw<Failure | null>(null);

  // 在途请求的进展从列表那一行看出来：revision 或结局变了就重新选一次。
  // 父组件按 Ref 重建本组件，所以换了请求不会走到这里
  const changeMark = $derived(row ? `${row.revision}/${row.outcome}` : '');
  $effect(() => {
    void changeMark;
    void store.epoch;
    let live = true;
    untrack(() =>
      store.openDetail(target).then(
        (r) => {
          if (!live) {
            void store.cancelDetail(r.selection).catch(() => {});
            return;
          }
          detail = r.detail;
          selection = r.selection;
          failure = null;
        },
        (raw) => {
          if (live) failure = toFailure(raw);
        },
      ),
    );
    return () => (live = false);
  });

  // 关掉抽屉（组件销毁）时取消最后那次选择：还在读的块就此作废
  $effect(() => () => {
    const sel = untrack(() => selection);
    if (sel) void store.cancelDetail(sel).catch(() => {});
  });

  /** 头部与概览用的记录：详情到了用详情里的（它和体是同一刻读的），之前先用列表那一行 */
  const record = $derived(detail?.record ?? row);
  const card = $derived(store.card(target.run.port));
  const pinned = $derived(store.pinned(target));

  $effect(() => {
    const el = tabEls[tabs.indexOf(tab)];
    void ui.lang;
    if (el) indicator = { left: el.offsetLeft, width: el.offsetWidth };
  });

  /** 有记录的尝试，按编号升序。tries 可能省略了中间几次（triesOmitted） */
  const tries = $derived(record ? [...record.tries].sort((a, b) => a.attempt - b.attempt) : []);
  const lastNo = $derived(tries.length ? tries[tries.length - 1].attempt : null);
  const shownNo = $derived(attemptNo ?? lastNo);
  const otherNo = $derived(compareNo ?? (tries.find((x) => x.attempt !== shownNo)?.attempt ?? null));
  const anyReports = $derived(!!record && (record.reports.length > 0 || record.tries.some((x) => x.reports.length > 0)));

  function tryDetail(n: number | null): TryDetail | undefined {
    return n === null ? undefined : detail?.tries.find((x) => x.attempt === n);
  }

  function countdown(r: Row): number {
    return Math.max(0, Math.ceil(((r.nextAttemptAt ?? store.now) - store.now) / 1000));
  }

  /** 失败的结局说明一下发生了什么；成功与在途的另有说法 */
  function failNote(r: Row): string | null {
    switch (r.outcome) {
      case 'client-gone':
        return t('d.clientGone');
      case 'format-response':
        return t('d.formatResponse');
      case 'upstream-error':
        return r.delivery === 'forward' ? t('d.forwarded') : r.code ? codeText(r.code) : t('d.upstreamError');
      case 'aproxy-error':
        return r.code ? codeText(r.code) : t('code.internal');
      case 'unknown':
        return t('d.unknownOutcome');
    }
    return null;
  }

  /** 救回的请求比第一次就成功多等了多久：最后一次尝试开始时，离收到请求过去了多久 */
  function waitedExtra(r: Row): number {
    const last = r.tries.find((x) => x.attempt === r.attempts);
    return last ? Math.max(0, last.startedAt - r.startedAt) : 0;
  }

  function peek(n: number) {
    attemptNo = n;
    compare = false;
    tab = 'response';
  }

  async function pin() {
    const next = !pinned;
    try {
      await store.setPinned(target, next);
      overlays.toast(t(next ? 'toast.pinned' : 'toast.unpinned'), next ? 'lilac' : 'mint');
    } catch (raw) {
      overlays.toast(faultText(toFailure(raw)), 'coral');
    }
  }

  async function copyPath() {
    copyOpen = false;
    if (!record) return;
    await copyText(record.path);
    overlays.toast(t('toast.copied'));
  }

  async function copyId() {
    copyOpen = false;
    if (!record) return;
    await copyText(`${record.ref.run.port}#${record.seq}`);
    overlays.toast(t('toast.copied'));
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

  const instName = $derived(card && aliasOf(card) ? `${target.run.port} ${aliasOf(card)}` : String(target.run.port));

  function headerValue(h: Header): { text: string; secret: boolean } {
    return typeof h.value === 'string' ? { text: h.value, secret: false } : { text: t('d.hidden'), secret: true };
  }

  function actionText(a: Action): string {
    if (a.label) return a.label;
    switch (a.type) {
      case 'protocol-convert':
        return t('d.act.convert', { from: a.from, to: a.to });
      case 'key-select':
        return t('d.act.key', { key: a.keyId });
      case 'route':
        return a.modelFrom && a.modelTo ? t('d.act.routeModel', { channel: a.channel, from: a.modelFrom, to: a.modelTo }) : t('d.act.route', { channel: a.channel });
      case 'header-edit':
        return t(a.op === 'set' ? 'd.act.headerSet' : 'd.act.headerRemove', { name: a.name });
      case 'body-edit':
        return t(a.op === 'set' ? 'd.act.bodySet' : 'd.act.bodyRemove', { path: a.path });
      default:
        return t('d.act.other');
    }
  }

  function tryLabel(x: Try): string {
    return x.upstreamStatus !== null ? String(x.upstreamStatus) : 'ERR';
  }
</script>

{#snippet attemptPicker(current: number | null, set: (n: number) => void, label: string)}
  <div class="picker" role="radiogroup" aria-label={label}>
    {#each tries as x (x.attempt)}
      <button class="pick" class:on={x.attempt === current} class:fail={!!x.code} role="radio" aria-checked={x.attempt === current} onclick={() => set(x.attempt)}>
        {t('d.attempt', { n: x.attempt })}<span class="pick-code">{x.durationMs === null ? '…' : tryLabel(x)}</span>
      </button>
    {/each}
  </div>
{/snippet}

{#snippet headerList(lines: Header[])}
  {#if lines.length === 0}
    <p class="note">{t('d.noHeaders')}</p>
  {:else}
    <dl class="headers">
      {#each lines as h, i (i)}
        {@const v = headerValue(h)}
        <dt class="mono">{h.name}</dt>
        <dd class="mono" class:secret={v.secret}>
          {#if v.secret}<Icon name="lock" size={11} stroke={2.4} />{/if}{v.text}
        </dd>
      {/each}
    </dl>
  {/if}
{/snippet}

{#snippet loadingNote()}
  <p class="note">{failure ? faultText(failure) : t('d.loading')}</p>
{/snippet}

{#snippet reportList(list: Report[])}
  <ul class="changes">
    {#each list as rep, i (i)}
      <li>
        <div class="ch-head">
          <code class="mono field">{rep.format}</code>
          <span class="chip lilac">{t(`d.stage.${rep.stage}`)}</span>
        </div>
        {#if rep.summary}<p class="ch-summary">{rep.summary}</p>{/if}
        {#if rep.actions.length}
          <div class="ch-body">
            {#each rep.actions as a, j (j)}
              <span class="chip">{actionText(a)}</span>
            {/each}
          </div>
        {/if}
      </li>
    {/each}
  </ul>
{/snippet}

{#snippet response(n: number | null)}
  {@const x = tries.find((y) => y.attempt === n)}
  {@const td = tryDetail(n)}
  {#if !x}
    <p class="note">{t('d.pending')}</p>
  {:else if !detail || !selection}
    {@render loadingNote()}
  {:else if !td}
    <p class="note">{t('d.tryOmitted')}</p>
  {:else}
    <div class="statusline">
      <span class="code-chip" class:fail={!!x.code}>{x.durationMs === null ? '…' : tryLabel(x)}</span>
      <span class="muted">{x.code ? codeText(x.code, x.upstreamStatus) : x.durationMs === null ? t('d.trying', { n: x.attempt }) : t('d.success')}</span>
      {#if x.durationMs !== null}<span class="muted">{durShort(x.durationMs)}</span>{/if}
      {#if x.responseBytes}<span class="muted">{bytes(x.responseBytes)}</span>{/if}
    </div>
    {#if td.error}
      <div class="noresp">
        <Icon name="plug" size={18} />
        <div>
          <strong>{codeText(td.error.code)}</strong>
          {#if td.error.message}<code class="mono">{td.error.message}</code>{/if}
        </div>
      </div>
    {/if}
    {#if td.response}
      <h4>{t('d.respHeaders')}</h4>
      {@render headerList(td.response.headers)}
      <h4>{t('d.body')}</h4>
      {#key `${selection.generation}/${x.attempt}`}
        <BodyView {selection} {target} part={{ kind: 'response', attempt: x.attempt }} info={td.response.body} />
      {/key}
    {:else if !td.error}
      <p class="note">{t('d.noResponse')}</p>
    {/if}
    {#if td.sent}
      <details class="sent">
        <summary>{t('d.sent')}<code class="mono">{td.sent.method} {td.sent.url}</code></summary>
        {@render headerList(td.sent.headers)}
        <h4>{t('d.body')}</h4>
        {#key `${selection.generation}/${x.attempt}`}
          <BodyView {selection} {target} part={{ kind: 'sent', attempt: x.attempt }} info={td.sent.body} />
        {/key}
      </details>
    {/if}
  {/if}
{/snippet}

<svelte:window onpointerdown={onWindowPointer} />

<aside class="drawer" aria-label={record ? `#${record.seq} ${record.path}` : t('d.loading')} in:drawer out:fadeOut={{ duration: 140 }}>
  <header class="d-head">
    <div class="d-title">
      {#if record}
        <span class="method">{record.method}</span>
        <code class="mono path" title={record.path}>{record.path}</code>
      {/if}
    </div>
    <div class="d-tools">
      <button class="icon-btn" class:pinned onclick={pin} aria-pressed={pinned} title={pinned ? t('logs.unpin') : t('logs.pin')} aria-label={pinned ? t('logs.unpin') : t('logs.pin')}>
        <Icon name="pin" size={16} />
      </button>
      <button class="icon-btn" onclick={() => onstep(-1)} title={t('d.prev')} aria-label={t('d.prev')}><Icon name="chevronUp" size={16} stroke={2.4} /></button>
      <button class="icon-btn" onclick={() => onstep(1)} title={t('d.next')} aria-label={t('d.next')}><Icon name="chevronDown" size={16} stroke={2.4} /></button>
      <button class="icon-btn close" onclick={onclose} title={t('d.close')} aria-label={t('d.close')}><Icon name="x" size={16} stroke={2.4} /></button>
    </div>
    <div class="d-sub">
      {#if record}<span class="mono seq">#{record.seq}</span>{/if}
      <span class="chip">{instName}</span>
      {#if record?.model}<span class="chip lilac">{record.model}</span>{/if}
      {#if record}<ResultChip row={record} />{/if}
    </div>
    <div class="tabs" role="tablist" aria-label={record ? `#${record.seq}` : ''} tabindex="-1" onkeydown={onTabKey}>
      <span class="tab-ind" style="transform: translateX({indicator.left}px); width: {indicator.width}px"></span>
      {#each tabs as tb, i (tb)}
        <button role="tab" id="tab-{tb}" aria-selected={tab === tb} aria-controls="panel-{tb}" tabindex={tab === tb ? 0 : -1} bind:this={tabEls[i]} onclick={() => (tab = tb)}>
          {t(`d.tab.${tb}`)}
          {#if tb === 'format' && anyReports}<span class="tab-dot"></span>{/if}
        </button>
      {/each}
      <span class="grow"></span>
      <div class="copy-wrap" bind:this={copyWrap}>
        <button class="copy" aria-haspopup="menu" aria-expanded={copyOpen} onclick={() => (copyOpen = !copyOpen)}>
          <Icon name="copy" size={14} />{t('d.copy')}<Icon name="chevronDown" size={12} stroke={2.6} />
        </button>
        {#if copyOpen}
          <div class="copy-menu" role="menu" in:pop={{ y: -6, scale: 0.96, duration: 380 }} out:fadeOut={{ duration: 100 }}>
            <button role="menuitem" onclick={copyPath}><Icon name="terminal" size={14} />{t('d.copyPath')}</button>
            <button role="menuitem" onclick={copyId}><Icon name="braces" size={14} />{t('d.copyId')}</button>
          </div>
        {/if}
      </div>
    </div>
  </header>

  <div class="d-body" role="tabpanel" id="panel-{tab}" aria-labelledby="tab-{tab}">
    {#key tab}
      <div class="pane" in:pop={{ y: 6, scale: 1, duration: 380 }}>
        {#if !record}
          {@render loadingNote()}
        {:else if tab === 'overview'}
          {#if failure}
            <div class="banner coral"><Icon name="info" size={18} /><p>{faultText(failure)}</p></div>
          {/if}
          {#if rescued(record)}
            <div class="banner mint">
              <Icon name="umbrella" size={18} />
              <div>
                <p>{t('d.rescuedNote', { n: record.attempts, d: durLong(waitedExtra(record)) })}</p>
                {#if record.heartbeats > 0}<p class="sub">{t('d.heartbeats', { n: record.heartbeats })}</p>{/if}
              </div>
            </div>
          {:else if record.outcome === 'ok'}
            <div class="banner mint"><Icon name="check" size={18} stroke={2.4} /><p>{t('d.firstTry')}</p></div>
          {:else if failNote(record)}
            <div class="banner coral"><Icon name="info" size={18} /><p>{failNote(record)}</p></div>
          {:else if card?.liveness === 'unresponsive' && record.outcome === 'in-flight'}
            <div class="banner coral"><Icon name="clock" size={18} /><p>{t('inst.stale')}</p></div>
          {/if}

          <h3>{t('d.timeline')}</h3>
          <ol class="timeline">
            {#if record.triesOmitted > 0}
              <li class="gap"><span class="gap-pill"><Icon name="info" size={12} stroke={2.4} />{t('d.omitted', { n: record.triesOmitted })}</span></li>
            {/if}
            {#each tries as x, i (x.attempt)}
              {@const running = x.durationMs === null}
              {@const ok = !x.code && !running}
              <li class="att" class:fail={!!x.code} class:ok class:pending={running}>
                <span class="node">{#if ok}<Icon name="check" size={14} stroke={3} />{:else}{x.attempt}{/if}</span>
                <div class="att-main">
                  {#if running}
                    <span class="att-reason">{t('d.trying', { n: x.attempt })}</span>
                  {:else}
                    <span class="code-chip" class:fail={!!x.code}>{tryLabel(x)}</span>
                    <span class="att-reason">{x.code ? codeText(x.code, x.upstreamStatus) : t('d.success')}</span>
                    <span class="att-dur num">{durShort(x.durationMs ?? 0)}</span>
                  {/if}
                  <button class="peek" onclick={() => peek(x.attempt)}>{t('d.peek')}<Icon name="chevronRight" size={12} stroke={2.6} /></button>
                </div>
              </li>
              {#if i < tries.length - 1 || (record.outcome === 'in-flight' && record.phase === 'backoff')}
                {@const live = i === tries.length - 1}
                <li class="gap">
                  <span class="gap-pill" class:live>
                    <Icon name="clock" size={12} stroke={2.4} />
                    {#if live}
                      {t('d.nextIn', { s: countdown(record), n: record.attempts + 1 })}
                    {:else if x.decision?.delayMs === 0}
                      {t('d.immediate')}
                    {:else if x.decision?.delayMs}
                      {t('d.waited', { d: durLong(x.decision.delayMs) })}
                    {:else}
                      {t('d.retried')}
                    {/if}
                    {#if x.decision?.by}<span class="by">{t('d.decidedBy', { by: x.decision.by })}</span>{/if}
                  </span>
                </li>
              {/if}
            {/each}
          </ol>

          <h3>{t('d.facts')}</h3>
          <dl class="facts">
            <dt>{t('d.received')}</dt>
            <dd class="mono">{stamp(record.startedAt)}</dd>
            <dt>{t('d.client')}</dt>
            <dd class="mono">{record.userAgent ?? '—'}</dd>
            <dt>{t('d.total')}</dt>
            <dd>{durLong(durationOf(record, store.now))}</dd>
            <dt>{t('d.delivery')}</dt>
            <dd>{record.delivery === 'forward' ? t('mode.forwardOnly') : record.upgraded ? t('d.upgraded') : t('cfgsum.retryForever')}{record.stream ? ` · ${t('d.stream')}` : ''}</dd>
            <dt>{t('d.reqSize')}</dt>
            <dd>{record.requestBytes ? bytes(record.requestBytes) : '—'}</dd>
            <dt>{t('d.respSize')}</dt>
            <dd>{record.responseBytes ? bytes(record.responseBytes) : '—'}</dd>
            <dt>{t('d.keepalive')}</dt>
            <dd>{record.heartbeats > 0 ? t('d.heartbeatRow', { n: record.heartbeats }) : '—'}</dd>
            <dt>{t('d.requestId')}</dt>
            <dd class="mono">#{record.seq}</dd>
          </dl>
          {#if record.truncated}<p class="note"><Icon name="info" size={14} />{t('d.truncated')}</p>{/if}
        {:else if tab === 'request'}
          <div class="sec-head">
            <h3>{t('d.headers')}</h3>
            <span class="chip pink"><Icon name="lock" size={11} stroke={2.4} />{t('d.secretNote')}</span>
          </div>
          {#if detail && selection}
            {@render headerList(detail.request.headers)}
            <h3>{t('d.body')}</h3>
            {#key selection.generation}
              <BodyView {selection} {target} part={{ kind: 'request' }} info={detail.request.body} />
            {/key}
          {:else}
            {@render loadingNote()}
          {/if}
        {:else if tab === 'response'}
          {#if tries.length === 0}
            <p class="note">{t('d.pending')}</p>
          {:else}
            <div class="sec-head">
              {@render attemptPicker(shownNo, (n) => (attemptNo = n), t('d.whichAttempt'))}
              {#if tries.length > 1}
                <button class="toggle" class:on={compare} aria-pressed={compare} onclick={() => (compare = !compare)}>
                  <Icon name="compare" size={14} />{t('d.compare')}
                </button>
              {/if}
            </div>
            {#if compare}
              <div class="compare">
                <div class="col">
                  {@render response(shownNo)}
                </div>
                <div class="col">
                  {@render attemptPicker(otherNo, (n) => (compareNo = n), t('d.compareWith', { n: otherNo ?? '' }))}
                  {@render response(otherNo)}
                </div>
              </div>
            {:else}
              {@render response(shownNo)}
            {/if}
          {/if}
        {:else if !anyReports}
          <p class="note">{t('d.format.none')}</p>
        {:else}
          {#if record.reports.length}
            <h3>{t('d.format.request')}</h3>
            {@render reportList(record.reports)}
          {/if}
          {#each tries.filter((x) => x.reports.length) as x (x.attempt)}
            <h3>{t('d.attempt', { n: x.attempt })}</h3>
            {@render reportList(x.reports)}
          {/each}
          <p class="note"><Icon name="info" size={14} />{t('d.format.note')}</p>
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
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }
  /* format 的名字是用户在 config.toml 里起的，可能很长：放不下时在任意处折行，不撑宽抽屉 */
  .field {
    min-width: 0;
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  /* 每个动作一条胶囊。动作文字可能很长（「走渠道 …，模型 … → …」带完整的模型名），
     窄窗口下抽屉只有三百多像素：列宽写成 minmax(0, 1fr) 不让最长的一条撑开，
     胶囊本身也改成可以折行（取消全局 .chip 的单行与固定高度），长模型名在任意处断开 */
  .ch-body {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 5px;
  }
  .ch-body .chip {
    justify-self: start;
    max-width: 100%;
    height: auto;
    min-height: 24px;
    padding-block: 4px;
    line-height: 1.35;
    white-space: normal;
    overflow-wrap: anywhere;
  }

  /* format 的报告摘要：来自 format 程序的文字，只当纯文本显示 */
  .ch-summary {
    margin: 0;
    color: var(--text-2);
    font-size: var(--fs-sm);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .by {
    margin-left: 4px;
    opacity: 0.75;
  }
  /* 发给上游的那一份默认收起：多数时候看收到的就够了 */
  .sent {
    margin-top: 10px;
    padding: 8px 12px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line-soft);
  }
  .sent summary {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    color: var(--text-2);
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .sent summary code {
    font-weight: 400;
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>

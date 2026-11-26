<!--
  实例详情：头部（端口、别名、状态、打码后的上游）、操作、在途请求与配置摘要。
  重启与停止都要二次确认；故意不提供「全部停止」之类的批量危险按钮——
  用户自己的 agent 会话很可能就经某个实例转发，误停等于切断自己。
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { t, ago, durLong, uptime, fmtNum, hasMessage } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { refKey, runKey, store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import type { Card, HistoryCode, Row, Run } from '../lib/backend/contract';
  import { toFailure, type Failure } from '../lib/backend/link';
  import { codeText, faultText } from '../lib/faults';
  import { aliasOf, controllable, durationOf, forwardOnly, hourBuckets, lastHour, retrying, statusOf } from '../lib/view';
  import { pop } from '../lib/motion';
  import Icon from '../components/Icon.svelte';
  import MascotView from '../components/MascotView.svelte';
  import { mascots } from '../lib/mascot/registry.svelte';
  import FlipNumber from '../components/FlipNumber.svelte';
  import JellyBar from '../components/JellyBar.svelte';
  import Sparkline from '../components/Sparkline.svelte';
  import ExtensionMounts from '../components/ExtensionMounts.svelte';
  import FormatsPanel from '../components/FormatsPanel.svelte';

  const inst = $derived(store.card(ui.instancePort));
  const status = $derived(inst ? statusOf(inst) : null);
  const busy = $derived(status === 'starting' || status === 'restarting' || status === 'stopping');

  // ---------------------------------------------------------------------------
  // 最近一次启动：实例自己记下的来由与备注。来由是固定拼写，这里认得的才翻译，更新版本的实例
  // 可能带来界面不认识的拼写，原样显示。看门狗与升级的备注是实例用固定英文写的，认得的句式翻译成
  // 界面语言；其余备注是启动它的人或 agent 写的，原样显示
  // ---------------------------------------------------------------------------

  const launch = $derived.by(() => {
    const reason = inst?.launchReason;
    if (!inst?.run || !reason) return null;
    const key = `launch.${reason}`;
    return { label: hasMessage(key) ? t(key) : reason, note: launchNote(reason, inst.launchNote) };
  });

  function launchNote(reason: string, note: string | null): string | null {
    if (!note) return null;
    if (reason === 'watchdog') {
      const code = /^previous process exited with code (-?\d+)$/.exec(note);
      if (code) return t('launch.exitedCode', { code: code[1] });
      if (note === 'previous process exited') return t('launch.exited');
      if (note === 'previous process stopped responding') return t('launch.hung');
    }
    if (reason === 'upgrade') {
      const v = /^(\S+) -> (\S+)$/.exec(note);
      if (v) return t('launch.versions', { from: v[1], to: v[2] });
    }
    return note;
  }

  // ---------------------------------------------------------------------------
  // 这次运行的请求：在途的那几条从这里来。快照里的最近 50 条是整个 home 的，不能当成这次运行的列表，
  // 所以单独按运行取第一页。运行换了（重启）、这次运行的请求变了（revision）、通道重新接上（epoch）就重取
  // ---------------------------------------------------------------------------

  /** 卡片每次重读都是新对象；只认运行的键，免得每次重读快照都重取一遍请求 */
  const runId = $derived(inst?.run ? runKey(inst.run) : null);
  const canRead = $derived(inst?.reads.historyFeed ?? false);
  let rows = $state.raw<Row[]>([]);
  let rowsFault = $state.raw<Failure | null>(null);

  $effect(() => {
    const id = runId;
    const ok = canRead;
    const rev = untrack(() => inst?.run ?? null);
    void store.revision(rev);
    void store.epoch;
    if (!id || !ok || !rev) {
      rows = [];
      return;
    }
    let live = true;
    load(rev).then(
      (page) => {
        if (!live) return;
        rows = page;
        rowsFault = null;
      },
      (raw) => {
        if (live) rowsFault = toFailure(raw);
      },
    );
    return () => (live = false);
  });

  async function load(run: Run): Promise<Row[]> {
    return (await store.queryLogs(run, null, 50)).entries;
  }

  const inflight = $derived(rows.filter((r) => r.outcome === 'in-flight'));
  const retryingHere = $derived(inst?.activity?.retrying ?? 0);

  /** 这次运行以来失败的尝试按原因分组：一眼看出上游到底怎么了 */
  const reasons = $derived(
    (Object.entries(inst?.activity?.byCode ?? {}) as [HistoryCode, number][]).filter(([, n]) => n > 0).sort((a, b) => b[1] - a[1]),
  );
  const spark = $derived(hourBuckets(inst?.activity?.buckets));

  const displayName = $derived.by(() => {
    if (!inst) return '';
    const alias = aliasOf(inst);
    if (!alias) return String(inst.port);
    return ui.lang === 'en' ? `${inst.port} (${alias})` : `${inst.port}（${alias}）`;
  });
  // 「第 N 次尝试」里的 N 要换成翻牌数字：按占位符把译文切成前后两段
  const attemptParts = $derived(t('inst.attemptN', { n: '@@' }).split('@@'));

  function fail(raw: unknown) {
    overlays.toast(faultText(toFailure(raw)), 'coral');
  }

  function askRestart(c: Card) {
    const run = c.run;
    if (!run) return;
    overlays.ask({
      title: t('confirm.restart.title', { name: displayName }),
      body: () => t(inflight.length ? 'confirm.restart.body' : 'confirm.restart.bodyIdle', { n: inst?.activity?.inFlight ?? inflight.length }),
      okLabel: t('confirm.restart.ok'),
      danger: false,
      onOk: async () => {
        try {
          await store.restart(run);
          overlays.toast(t('toast.restarted', { name: displayName }));
        } catch (raw) {
          fail(raw);
        }
      },
    });
  }

  function askStop(c: Card) {
    const run = c.run;
    if (!run) return;
    overlays.ask({
      title: t('confirm.stop.title', { name: displayName }),
      body: () => t(inflight.length ? 'confirm.stop.body' : 'confirm.stop.bodyIdle', { n: inst?.activity?.inFlight ?? inflight.length }),
      okLabel: t('confirm.stop.ok'),
      danger: true,
      onOk: async () => {
        try {
          await store.stop(run);
          overlays.toast(t('toast.stopped', { name: displayName }));
        } catch (raw) {
          fail(raw);
        }
      },
    });
  }

  function askForce(c: Card) {
    const run = c.run;
    if (!run) return;
    overlays.ask({
      title: t('confirm.force.title', { name: displayName }),
      body: t('confirm.force.body', { pid: c.pid ?? '' }),
      okLabel: t('confirm.force.ok'),
      danger: true,
      onOk: async () => {
        try {
          await store.forceStop(run);
          overlays.toast(t('toast.stopped', { name: displayName }));
        } catch (raw) {
          fail(raw);
        }
      },
    });
  }

  async function restore(c: Card) {
    if (!c.expectedRestore) return;
    try {
      await store.restore(c.port, c.expectedRestore);
      overlays.toast(t('toast.started', { name: displayName }));
    } catch (raw) {
      fail(raw);
    }
  }

  function forget(c: Card) {
    const token = c.expectedRestore;
    if (!token) return;
    overlays.ask({
      title: t('card.forgetTitle', { name: displayName }),
      body: t('card.forgetBody'),
      okLabel: t('card.forget'),
      danger: false,
      onOk: () => void store.forgetRestore(c.port, token).then(() => ui.go('overview'), fail),
    });
  }

  /** 让 Rust 马上重新找一遍实例：卡住的实例缓过来了，卡片会随之更新 */
  async function reconnect(c: Card) {
    overlays.toast(t('toast.reconnecting', { port: c.port }), 'lilac');
    try {
      await store.rescan();
    } catch (raw) {
      fail(raw);
    }
  }

  /**
   * 退避等待的进度：只知道下一次在什么时候、这次等多久（最后一次尝试的决定里的延迟）。
   * 不知道等多久时没有进度可言，条就是满的、只显示倒计时
   */
  function progress(r: Row): number {
    if (r.nextAttemptAt === null) return 1;
    const delay = r.tries.find((x) => x.attempt === r.attempts)?.decision?.delayMs ?? null;
    if (!delay) return 1;
    return 1 - (r.nextAttemptAt - store.now) / delay;
  }

  function remaining(r: Row): number {
    return Math.max(0, Math.ceil(((r.nextAttemptAt ?? store.now) - store.now) / 1000));
  }

  /** 最后一次尝试为什么失败 */
  function lastReason(r: Row): string | null {
    const last = r.tries.find((x) => x.attempt === r.attempts) ?? r.tries[r.tries.length - 1];
    return last?.code ? codeText(last.code, last.upstreamStatus) : null;
  }

  function openLog(r: Row) {
    ui.selectedRef = r.ref;
    ui.go('logs');
  }

  function openLogs() {
    ui.logInstanceFilter = ui.instancePort;
    ui.go('logs');
  }
</script>

{#if !inst}
  <section class="missing">
    {#if mascots.current}<MascotView mood="sleeping" size="hero" px={110} />{/if}
    <p>{store.connected ? t('inst.notFound', { port: ui.instancePort }) : t('ov.connecting')}</p>
    <button class="btn" onclick={() => ui.go('overview')}>{t('nav.overview')}</button>
  </section>
{:else}
  <header class="ihead">
    <div class="id">
      <span class="port">{inst.port}</span>
      {#if aliasOf(inst)}<span class="alias" title={inst.metadata?.aliases.join(', ')}>{aliasOf(inst)}</span>{/if}
      {#if status === 'restore'}
        <span class="chip butter"><Icon name="clock" size={13} stroke={2.4} />{t('status.restore')}</span>
      {:else if status === 'unresponsive'}
        <span class="chip solid coral"><Icon name="alert" size={13} stroke={2.4} />{t('status.unresponsive')}</span>
      {:else if busy}
        <span class="chip butter">{t(`status.${status!}`)}</span>
      {:else if status === 'down' || status === 'incompatible' || status === 'unknown'}
        <span class="chip"><span class="dot gray"></span>{t(`status.${status}`)}</span>
      {:else}
        <span class="chip mint"><span class="dot mint" class:breathe={status === 'active'}></span>{t(`status.${status!}`)}</span>
      {/if}
      {#if retryingHere > 0}
        <span class="chip solid sky"><Icon name="umbrella" size={13} stroke={2.4} />{t('card.retryingN', { n: retryingHere })}</span>
      {/if}
      {#if forwardOnly(inst)}
        <span class="hazard"><span class="tape"></span>{t('mode.forwardOnly')}<span class="sep">/</span>{t('mode.noRetry')}</span>
      {/if}
    </div>
    <div class="meta">
      {#if inst.metadata}<span class="url"><Icon name="upstream" size={14} stroke={2.2} /><code class="mono">{inst.metadata.baseUrl}</code></span>{/if}
      {#if inst.pid}<span class="chip">pid {inst.pid}</span>{/if}
      {#if inst.version}<span class="chip">v{inst.version}</span>{/if}
      {#if inst.startedAt !== null && inst.run}<span class="chip">{t('card.uptime', { d: uptime(store.now - inst.startedAt) })}</span>{/if}
    </div>
    {#if launch}
      <p class="launch">
        <Icon name="clock" size={14} stroke={2.2} /><span class="launch-label">{t('launch.title')}</span><span
          class="launch-reason">{launch.label}</span>{#if launch.note}<span class="launch-note">{launch.note}</span>{/if}
      </p>
    {/if}
    <div class="actions">
      <button class="btn sm" onclick={openLogs}><Icon name="list" size={14} />{t('inst.openLogs')}</button>
      <button class="btn sm" disabled={!inst.run} onclick={() => ui.go('config', inst.port)}><Icon name="sliders" size={14} />{t('inst.editConfig')}</button>
      {#if status === 'restore'}
        {#if inst.expectedRestore}
          <span class="divider" aria-hidden="true"></span>
          <button class="btn sm mint" onclick={() => restore(inst)}><Icon name="play" size={13} />{t('card.restore')}</button>
        {/if}
      {:else if status === 'unresponsive' && inst.run}
        <span class="divider" aria-hidden="true"></span>
        <button class="btn sm danger" onclick={() => askForce(inst)}><Icon name="power" size={14} />{t('err.forceStop')}</button>
      {:else if controllable(inst)}
        <span class="divider" aria-hidden="true"></span>
        <button class="btn sm" disabled={busy} onclick={() => askRestart(inst)}><Icon name="restart" size={14} />{t('inst.restart')}</button>
        <button class="btn sm danger-outline" disabled={busy} onclick={() => askStop(inst)}><Icon name="stop" size={13} />{t('inst.stop')}</button>
      {/if}
    </div>
  </header>
  <ExtensionMounts point="instance.top" ctx={{ port: inst.port }} />

  {#if forwardOnly(inst)}
    <div class="callout hazard-callout">
      <span class="tape-big" aria-hidden="true"></span>
      <p>{t('inst.fwdWarn')}</p>
    </div>
  {/if}

  {#if status === 'unresponsive'}
    <!-- 错误状态：说清发生了什么、会自动怎样、用户现在能做什么 -->
    <section class="trouble" in:pop>
      <span class="t-badge" aria-hidden="true"><Icon name="alert" size={26} stroke={2.4} /></span>
      <div>
        <h2>{t('err.unresponsive.title', { port: inst.port })}</h2>
        <p>{t('err.unresponsive.body', { pid: inst.pid ?? '' })}</p>
        <div class="trouble-act">
          <button class="btn sm" onclick={() => reconnect(inst)}><Icon name="restart" size={14} />{t('err.reconnect')}</button>
          {#if inst.run}<button class="btn sm danger" onclick={() => askForce(inst)}><Icon name="power" size={14} />{t('err.forceStop')}</button>{/if}
        </div>
      </div>
    </section>
  {:else if status === 'restore'}
    <section class="trouble calm" in:pop>
      <span class="t-badge" aria-hidden="true"><Icon name="power" size={24} stroke={2.4} /></span>
      <div>
        <h2>{t('stopped.title', { name: displayName })}</h2>
        <p>{t(inst.restoreReason === 'config-missing' ? 'card.restoreMissing' : 'card.restoreHint')}</p>
        {#if inst.expectedRestore}
          <div class="trouble-act">
            <button class="btn sm mint" onclick={() => restore(inst)}><Icon name="play" size={13} />{t('card.restore')}</button>
            <button class="btn sm ghost" onclick={() => forget(inst)}>{t('card.forget')}</button>
          </div>
        {/if}
      </div>
    </section>
  {:else if inst.historyUnavailable === 'source-redaction-missing'}
    <section class="trouble calm" in:pop>
      <span class="t-badge" aria-hidden="true"><Icon name="restart" size={24} stroke={2.4} /></span>
      <div>
        <h2>{t('card.oldVersion')}</h2>
        <p>{t('card.oldVersionHint')}</p>
        {#if controllable(inst)}
          <div class="trouble-act">
            <button class="btn sm" disabled={busy} onclick={() => askRestart(inst)}><Icon name="restart" size={14} />{t('inst.restart')}</button>
          </div>
        {/if}
      </div>
    </section>
  {/if}

  <div class="grid">
    <section class="inflight" class:stale={status === 'unresponsive'}>
      <h2 class="section-title">{t('inst.inflight')}<span class="count">{inst.activity?.inFlight ?? inflight.length}</span></h2>
      {#if status === 'unresponsive'}
        <!-- 控制通道沉默时拿不到新数据：照旧显示最后读到的，但明确标成可能过期的 -->
        <p class="stale-note"><Icon name="clock" size={14} />{t('inst.stale')}</p>
      {:else if rowsFault}
        <p class="stale-note"><Icon name="alert" size={14} />{faultText(rowsFault)}</p>
      {/if}
      {#each inflight as r (refKey(r.ref))}
        {@const again = retrying(r)}
        <article class="req" class:retrying={again} in:pop={{ y: 8 }}>
          <button class="req-hit" onclick={() => openLog(r)} aria-label="#{r.seq} {r.path}"></button>
          <div class="r1">
            <span class="method">{r.method}</span>
            <code class="path mono">{r.path}</code>
            {#if r.model}<span class="model">{r.model}</span>{/if}
            <span class="seq mono">#{r.seq}</span>
          </div>
          {#if again}
            <div class="r2">
              <span class="attempt">
                {attemptParts[0]}<span class="attempt-n"><FlipNumber value={r.phase === 'backoff' ? r.attempts + 1 : r.attempts} /></span>{attemptParts[1]}
              </span>
              {#if lastReason(r)}
                <span class="chip coral"><Icon name="cloudRain" size={13} />{lastReason(r)}</span>
              {/if}
            </div>
            <div class="r3">
              <JellyBar value={r.phase === 'backoff' ? progress(r) : 1} label={t('inst.retryIn', { s: remaining(r) })} tone={r.phase === 'backoff' ? 'sky' : 'lilac'} />
              <span class="countdown">
                {#if r.phase === 'backoff'}
                  {remaining(r) > 0 ? t('inst.retryIn', { s: remaining(r) }) : t('inst.retryNow')}
                {:else}
                  {t('inst.attempting')}
                {/if}
              </span>
            </div>
          {:else}
            <div class="r3">
              <JellyBar value={1} label={t('inst.receiving')} tone="mint" />
              <span class="countdown">{t(r.phase === 'forwarding' || r.phase === 'replaying' ? 'inst.receiving' : 'inst.attempting')}</span>
            </div>
          {/if}
          <div class="r4">
            {#if r.keepalive}
              <span class="beat">
                {#key r.heartbeats}
                  <span class="heart" in:pop={{ y: 0, scale: 0.4, duration: 600 }}><Icon name="heart" size={14} stroke={2.2} /></span>
                {/key}
                {t('inst.heartbeat', { n: r.heartbeats })}
              </span>
            {/if}
            <span class="waited">{t('inst.waited', { d: durLong(durationOf(r, store.now)) })}</span>
            {#if r.userAgent}<span class="client">{r.userAgent.split(' ')[0]}</span>{/if}
          </div>
        </article>
      {:else}
        <div class="quiet">
          <span class="q-icon" aria-hidden="true"><Icon name="check" size={18} stroke={2.6} /></span>
          <span>{t('inst.inflightEmpty')}</span>
          {#if inst.activity && inst.activity.requestsTotal > 0}<span class="quiet-sub">{ago(store.now - inst.activity.lastRequestAt)}</span>{/if}
        </div>
      {/each}

      {#if inst.activity}
        {@const req = lastHour(inst.activity.buckets, 'requests')}
        {@const retries = lastHour(inst.activity.buckets, 'retries')}
        <section class="panel hour">
          <div class="hour-head">
            <h2 class="section-title">{t('card.lastHour')}</h2>
            <span class="legend"><i class="k k-req"></i>{t('card.requests')} <b class="num">{req === null ? '—' : fmtNum(req)}</b></span>
            <span class="legend"><i class="k k-retry"></i>{t('card.retries')} <b class="num">{forwardOnly(inst) || retries === null ? '—' : fmtNum(retries)}</b></span>
          </div>
          {#if spark}
            <Sparkline req={spark.req} retry={spark.retry} height={64} label={t('card.sparkLabel', { m: Math.max(1, Math.round(inst.activity.buckets.widthSecs / 60)) })} />
          {/if}
          <div class="run-totals">
            <span class="sub">{t('inst.sinceStart')}</span>
            <span class="legend">{t('card.requests')} <b class="num">{fmtNum(inst.activity.requestsTotal)}</b></span>
            <span class="legend">{t('card.retries')} <b class="num">{fmtNum(inst.activity.retriesTotal)}</b></span>
            <span class="legend"><Icon name="umbrella" size={13} />{t('logs.filter.rescued')} <b class="num">{fmtNum(inst.activity.rescuedTotal)}</b></span>
          </div>
          {#if reasons.length}
            <div class="reasons">
              {#each reasons as [code, n] (code)}
                <span class="chip coral"><Icon name="cloudRain" size={12} />{codeText(code)}<b class="num">×{n}</b></span>
              {/each}
            </div>
          {/if}
        </section>
      {/if}
    </section>

    <div class="side">
      {#if inst.metadata}
        {@const m = inst.metadata}
        <section class="panel summary">
          <h2 class="section-title">{t('inst.config')}</h2>
          <dl>
            <dt>{t('cfgsum.upstream')}</dt>
            <dd><code class="mono">{m.baseUrl}</code></dd>
            <dt>{t('cfgsum.listen')}</dt>
            <dd><code class="mono">{m.listen}</code></dd>
            <dt>{t('cfgsum.delivery')}</dt>
            <dd>{#if m.delivery === 'forward'}<span class="muted">{t('mode.noRetry')}</span>{:else if m.delivery === 'retry'}{t('cfgsum.retryForever')}{:else}<span class="muted">{t('status.unknown')}</span>{/if}</dd>
            <dt>{t('cfgsum.rules')}</dt>
            <dd>{#if m.retryRuleCount > 0}{t('cfgsum.ruleCount', { n: m.retryRuleCount })}{:else}<span class="muted">{t('cfgsum.none')}</span>{/if}</dd>
            <dt>{t('cfgsum.keepalive')}</dt>
            <dd>
              {#if m.delivery === 'forward' || m.keepaliveIntervalSecs === 0}
                <span class="muted">{t('cfgsum.keepaliveOff')}</span>
              {:else}
                {t('cfgsum.everyN', { n: m.keepaliveIntervalSecs })}
                {#if m.keepaliveTrigger !== 'unknown'}<span class="chip">{t(`cfg.o.${m.keepaliveTrigger === 'body-stream' ? 'body_stream' : m.keepaliveTrigger}`)}</span>{/if}
              {/if}
            </dd>
            <dt>{t('cfgsum.diskCache')}</dt>
            <dd>{m.diskCache ? t('cfgsum.on') : t('cfgsum.off')}</dd>
            {#if m.aliases.length > 1}
              <dt>{t('cfgsum.aliases')}</dt>
              <dd>{m.aliases.join(', ')}</dd>
            {/if}
            <dt>{t('cfgsum.file')}</dt>
            <dd>{#if m.configPath}<code class="mono">{m.configPath}</code>{:else}<span class="muted">{t('cfgsum.none')}</span>{/if}</dd>
          </dl>
        </section>
      {/if}
      <!-- 不答复的实例问不到条目；启动、停止、重启进行中时不让单独重启 format，免得和整个实例的操作交错 -->
      <FormatsPanel run={status === 'unresponsive' ? null : inst.run} controllable={controllable(inst) && !busy} />
    </div>
  </div>
{/if}

<style>
  .missing {
    display: grid;
    justify-items: center;
    gap: 12px;
    padding: 80px 0;
    color: var(--text-2);
  }
  .ihead {
    display: grid;
    gap: 10px;
    margin-bottom: 18px;
  }
  .id {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
  }
  .port {
    font-size: var(--fs-3xl);
    font-weight: 640;
    line-height: 1;
    font-variant-numeric: tabular-nums;
  }
  .alias {
    font-size: var(--fs-xl);
    font-weight: 550;
    color: var(--text-2);
    margin-right: 4px;
  }
  .sep {
    opacity: 0.5;
  }
  .meta {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }
  .url {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-2);
    margin-right: 4px;
  }
  /* 最近一次启动：备注可能是一整句话，按文字自然折行、不截断；悬挂缩进让折下来的行与文字对齐 */
  .launch {
    margin: 0;
    padding-left: 22px;
    text-indent: -22px;
    color: var(--text-2);
    font-size: var(--fs-sm);
    line-height: 1.5;
  }
  .launch :global(svg) {
    display: inline-block;
    vertical-align: -2px;
    margin-right: 8px;
  }
  .launch-label {
    color: var(--text-3);
    margin-right: 8px;
  }
  .launch-reason {
    color: var(--text);
    font-weight: 560;
  }
  .launch-note {
    overflow-wrap: anywhere;
  }
  .launch-note::before {
    content: '·';
    margin: 0 8px;
    color: var(--text-3);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 2px;
  }
  .divider {
    width: 1.5px;
    height: 20px;
    margin: 0 4px;
    background: var(--line-soft);
  }
  /* 停止是危险操作，但不是「主要」操作：只用珊瑚色描边字，不用实心填充抢眼 */
  :global(.btn.danger-outline) {
    color: var(--coral-deep);
  }
  :global(.btn.danger-outline:hover:not(:disabled)) {
    background: var(--coral-soft);
  }

  .callout {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-bottom: 18px;
    padding: 10px 16px 10px 10px;
    border-radius: var(--r-md);
    border: var(--line-w) solid color-mix(in oklab, var(--butter-soft) 60%, var(--butter-deep));
    background: var(--butter-soft);
    font-weight: 550;
  }
  .callout p {
    margin: 0;
  }
  .tape-big {
    flex: none;
    width: 46px;
    height: 30px;
    border-radius: var(--r-sm);
    border: var(--line-w) solid var(--butter-deep);
    background: repeating-linear-gradient(-45deg, var(--on-fill) 0 6px, var(--butter) 6px 12px);
  }

  .trouble {
    display: flex;
    align-items: center;
    gap: 20px;
    margin-bottom: 20px;
    padding: 18px 22px;
    border-radius: var(--r-lg);
    border: var(--line-w) solid color-mix(in oklab, var(--coral-soft) 65%, var(--coral));
    background: var(--coral-soft);
    box-shadow: var(--shadow-2);
  }
  /* 已停止（主动停的，不是故障）：虚线、无阴影，和总览里已停止的卡片同一种读法 */
  .trouble.calm {
    background: var(--card-2-bg);
    box-shadow: none;
    border-style: dashed;
    border-color: var(--line-dash);
  }
  /* 横幅里的状态徽章：吉祥物只出现在总览、空状态等少数几处，实例页的状态横幅用图标表达 */
  .t-badge {
    flex: none;
    display: grid;
    place-items: center;
    width: 56px;
    height: 56px;
    border-radius: 50%;
    background: var(--coral);
    color: var(--on-fill);
    box-shadow: 0 4px 14px -4px color-mix(in oklab, var(--coral) 90%, transparent);
  }
  .calm .t-badge {
    background: var(--card-bg);
    color: var(--text-3);
    box-shadow: var(--shadow-1);
  }
  .trouble h2 {
    margin: 0 0 4px;
    font-size: var(--fs-lg);
    font-weight: 620;
  }
  .trouble p {
    margin: 0 0 12px;
    max-width: 70ch;
    color: var(--text-2);
  }
  .trouble-act {
    display: flex;
    gap: 8px;
  }

  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1.55fr) minmax(280px, 1fr);
    gap: 20px;
    align-items: start;
  }
  .inflight {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  /* 窗口窄（接近最小宽度 900px，或屏幕小、窗口被缩进屏幕）时，右栏有 280px 的下限，左边的在途请求卡片
     只剩三百多像素：英文的心跳数与等待时长被拆成两行，路径截得只剩几个字；再窄些，文字会挤出卡片、
     压到配置摘要上。这时两栏改成上下排，在途请求拿到整行宽度，配置摘要与 format 程序排在下面 */
  @media (max-width: 1000px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .req {
    position: relative;
    display: grid;
    gap: 9px;
    padding: 13px 16px 12px;
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    box-shadow: var(--shadow-2);
    transition:
      transform var(--dur-mid) var(--ease-spring),
      box-shadow var(--dur-mid) var(--ease-soft);
  }
  .req:hover {
    transform: translateY(var(--lift));
    box-shadow: var(--shadow-2-hover);
  }
  .req.retrying {
    background: color-mix(in oklab, var(--sky-soft) 70%, var(--card-bg));
  }
  .req-hit {
    position: absolute;
    inset: 0;
    border: 0;
    border-radius: inherit;
    background: transparent;
  }
  .req > :not(.req-hit) {
    position: relative;
    pointer-events: none;
  }
  .r1,
  .r2,
  .r4 {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
  }
  .method {
    padding: 2px 7px;
    border-radius: var(--r-xs);
    background: var(--lilac);
    color: var(--on-fill);
    font-size: 11px;
    font-weight: 650;
  }
  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .model {
    color: var(--text-2);
    font-size: var(--fs-sm);
    white-space: nowrap;
  }
  .seq {
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .attempt {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
  }
  .attempt-n {
    font-size: 17px;
  }
  .r3 {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    gap: 12px;
  }
  .countdown {
    min-width: 96px;
    text-align: right;
    font-size: var(--fs-sm);
    font-weight: 600;
    color: var(--sky-deep);
    font-variant-numeric: tabular-nums;
  }
  /* 第四行放不下时按条目整体换行，不在「6 heartbeats」「waiting 1 min 40 s」这样的短语中间断开 */
  .r4 {
    flex-wrap: wrap;
    row-gap: 4px;
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
  .r4 > span {
    white-space: nowrap;
  }
  .beat {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--pink-deep);
    font-weight: 600;
  }
  .heart {
    display: inline-flex;
    color: var(--pink-deep);
  }
  .heart :global(svg) {
    fill: var(--pink);
  }
  /* 客户端名取自 User-Agent，长度不受控：单独占一行时也最多一行宽，超出用省略号 */
  .client {
    margin-left: auto;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .stale-note {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: -4px 0 0;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .stale .req {
    opacity: 0.55;
    filter: saturate(0.4);
  }
  .quiet {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 18px;
    border-radius: var(--r-lg);
    border: var(--line-w-thin) dashed var(--line-dash);
    color: var(--text-2);
  }
  .q-icon {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: 50%;
    background: var(--mint-soft);
    color: var(--mint-deep);
  }
  .quiet-sub {
    color: var(--text-3);
    font-size: var(--fs-sm);
  }

  .hour {
    display: grid;
    gap: 12px;
    margin-top: 8px;
    padding: 14px 16px 14px;
  }
  .hour-head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 14px;
  }
  .hour-head .section-title {
    margin-right: auto;
    font-size: var(--fs-md);
  }
  .legend {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .legend b {
    color: var(--text);
    font-weight: 620;
  }
  .k {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 3px;
  }
  .k-req {
    background: var(--chart-req);
  }
  .k-retry {
    background: var(--chart-retry);
  }
  /* 这次启动以来的总数：实例重启后从头算，与上面「近一小时」的分桶分开写 */
  .run-totals {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 14px;
    margin: 6px 0 10px;
  }
  .run-totals .sub {
    margin-right: auto;
    color: var(--text-2);
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .reasons {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .reasons b {
    margin-left: 2px;
  }
  .side {
    display: grid;
    gap: 20px;
    min-width: 0;
  }
  .summary {
    padding: 16px 18px 8px;
  }
  .summary dl {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 16px;
    margin: 12px 0 0;
  }
  .summary dt,
  .summary dd {
    padding: 8px 0;
    border-top: var(--line-w-thin) dashed var(--line-soft);
    font-size: var(--fs-sm);
  }
  .summary dt {
    color: var(--text-3);
    white-space: nowrap;
  }
  .summary dd {
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-width: 0;
    word-break: break-all;
  }
  .muted {
    color: var(--text-3);
  }
</style>

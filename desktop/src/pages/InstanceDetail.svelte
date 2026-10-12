<!--
  实例详情：头部（端口、别名、状态、打码后的上游）、操作、在途请求与配置摘要。
  重启与停止都要二次确认；故意不提供「全部停止」之类的批量危险按钮——
  用户自己的 agent 会话很可能就经某个实例转发，误停等于切断自己。
-->
<script lang="ts">
  import { t, ago, durLong, uptime } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import type { Instance, LogEntry } from '../lib/types';
  import { pop } from '../lib/motion';
  import Icon from '../components/Icon.svelte';
  import MascotView from '../components/MascotView.svelte';
  import { mascots } from '../lib/mascot/registry.svelte';
  import FlipNumber from '../components/FlipNumber.svelte';
  import JellyBar from '../components/JellyBar.svelte';
  import Sparkline from '../components/Sparkline.svelte';
  import type { Reason } from '../lib/types';

  const inst = $derived(store.instance(ui.instancePort));
  const inflight = $derived(store.inflight.filter((e) => e.port === ui.instancePort));
  const retryingHere = $derived(inflight.filter((e) => e.outcome === 'retrying').length);

  /** 近一小时这个实例的失败尝试按原因分组：一眼看出上游到底怎么了 */
  const reasons = $derived.by(() => {
    const counts = new Map<Reason, number>();
    for (const e of store.logs) {
      if (e.port !== ui.instancePort || store.now - e.time > 3_600_000) continue;
      for (const a of e.attempts) if (a.reason) counts.set(a.reason, (counts.get(a.reason) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1]);
  });
  const rescuedHere = $derived(
    store.logs.filter((e) => e.port === ui.instancePort && e.outcome === 'rescued' && store.now - e.time < 3_600_000).length,
  );
  const displayName = $derived.by(() => {
    if (!inst) return '';
    if (!inst.alias) return String(inst.port);
    return ui.lang === 'en' ? `${inst.port} (${inst.alias})` : `${inst.port}（${inst.alias}）`;
  });
  // 「第 N 次尝试」里的 N 要换成翻牌数字：按占位符把译文切成前后两段
  const attemptParts = $derived(t('inst.attemptN', { n: '@@' }).split('@@'));
  // 在途请求的进度、倒计时、已等时长都按这个时钟算。控制通道沉默时界面手里只有最后一份快照，
  // 时钟就停在快照那一刻：否则倒计时会在过期数据上继续走，和「这是 N 秒前的数据」自相矛盾
  const clock = $derived(inst?.status === 'unresponsive' && inst.silentSince !== null ? inst.silentSince : store.now);
  const silentFor = $derived(inst?.silentSince != null ? store.now - inst.silentSince : 0);

  function askRestart(i: Instance) {
    overlays.ask({
      title: t('confirm.restart.title', { name: displayName }),
      body: () => t(inflight.length ? 'confirm.restart.body' : 'confirm.restart.bodyIdle', { n: inflight.length }),
      okLabel: t('confirm.restart.ok'),
      danger: false,
      onOk: async () => {
        await store.restart(i.port);
        overlays.toast(t('toast.restarted', { name: displayName }));
      },
    });
  }

  function askStop(i: Instance) {
    overlays.ask({
      title: t('confirm.stop.title', { name: displayName }),
      body: () => t(inflight.length ? 'confirm.stop.body' : 'confirm.stop.bodyIdle', { n: inflight.length }),
      okLabel: t('confirm.stop.ok'),
      danger: true,
      onOk: async () => {
        await store.stop(i.port);
        overlays.toast(t('toast.stopped', { name: displayName }));
      },
    });
  }

  function askForce(i: Instance) {
    overlays.ask({
      title: t('confirm.force.title', { name: displayName }),
      body: t('confirm.force.body', { pid: i.pid ?? '' }),
      okLabel: t('confirm.force.ok'),
      danger: true,
      onOk: async () => {
        await store.forceStop(i.port);
        overlays.toast(t('toast.stopped', { name: displayName }));
      },
    });
  }

  async function start(i: Instance) {
    await store.start(i.port);
    overlays.toast(t('toast.started', { name: displayName }));
  }

  function reconnect(i: Instance) {
    overlays.toast(t('toast.reconnecting', { port: i.port }), 'lilac');
    store.reconnect(i.port);
  }

  function progress(e: LogEntry): number {
    if (e.phaseStartedAt === null || e.phaseEndsAt === null) return 0;
    const span = e.phaseEndsAt - e.phaseStartedAt;
    return span <= 0 ? 1 : (clock - e.phaseStartedAt) / span;
  }

  function remaining(e: LogEntry): number {
    return Math.max(0, Math.ceil(((e.phaseEndsAt ?? clock) - clock) / 1000));
  }

  function lastReason(e: LogEntry) {
    return e.attempts[e.attempts.length - 1]?.reason ?? null;
  }

  function openLog(e: LogEntry) {
    ui.selectedLog = e.key;
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
    <p>{t('inst.notFound', { port: ui.instancePort })}</p>
    <button class="btn" onclick={() => ui.go('overview')}>{t('nav.overview')}</button>
  </section>
{:else}
  <header class="ihead">
    <div class="id">
      <span class="port">{inst.port}</span>
      {#if inst.alias}<span class="alias">{inst.alias}</span>{/if}
      {#if inst.status === 'stopped'}
        <span class="chip"><span class="dot gray"></span>{t('status.stopped')}</span>
      {:else if inst.status === 'unresponsive'}
        <span class="chip solid coral"><Icon name="alert" size={13} stroke={2.4} />{t('status.unresponsive')}</span>
      {:else if inst.status === 'restarting'}
        <span class="chip butter">{t('status.restarting')}</span>
      {:else}
        <span class="chip mint"><span class="dot mint" class:breathe={inst.status === 'active'}></span>{t(`status.${inst.status}`)}</span>
      {/if}
      {#if retryingHere > 0}
        <span class="chip solid sky"><Icon name="umbrella" size={13} stroke={2.4} />{t('card.retryingN', { n: retryingHere })}</span>
      {/if}
      {#if inst.forwardOnly}
        <span class="hazard"><span class="tape"></span>{t('mode.forwardOnly')}<span class="sep">/</span>{t('mode.noRetry')}</span>
      {/if}
    </div>
    <div class="meta">
      <span class="url"><Icon name="upstream" size={14} stroke={2.2} /><code class="mono">{inst.baseUrl}</code></span>
      {#if inst.pid}<span class="chip">pid {inst.pid}</span>{/if}
      <span class="chip">v{inst.version}</span>
      {#if inst.status !== 'stopped'}<span class="chip">{t('card.uptime', { d: uptime(store.now - inst.startedAt) })}</span>{/if}
    </div>
    <div class="actions">
      <button class="btn sm" onclick={openLogs}><Icon name="list" size={14} />{t('inst.openLogs')}</button>
      <button class="btn sm" onclick={() => ui.go('config', inst.port)}><Icon name="sliders" size={14} />{t('inst.editConfig')}</button>
      <span class="divider" aria-hidden="true"></span>
      {#if inst.status === 'stopped'}
        <button class="btn sm mint" onclick={() => start(inst)}><Icon name="play" size={13} />{t('inst.start')}</button>
      {:else if inst.status === 'unresponsive'}
        <button class="btn sm danger" onclick={() => askForce(inst)}><Icon name="power" size={14} />{t('err.forceStop')}</button>
      {:else}
        <button class="btn sm" disabled={inst.status === 'restarting'} onclick={() => askRestart(inst)}><Icon name="restart" size={14} />{t('inst.restart')}</button>
        <button class="btn sm danger-outline" disabled={inst.status === 'restarting'} onclick={() => askStop(inst)}><Icon name="stop" size={13} />{t('inst.stop')}</button>
      {/if}
    </div>
  </header>

  {#if inst.forwardOnly}
    <div class="callout hazard-callout">
      <span class="tape-big" aria-hidden="true"></span>
      <p>{t('inst.fwdWarn')}</p>
    </div>
  {/if}

  {#if inst.status === 'unresponsive'}
    <!-- 错误状态：说清发生了什么、会自动怎样、用户现在能做什么 -->
    <section class="trouble" in:pop>
      <span class="t-badge" aria-hidden="true"><Icon name="alert" size={26} stroke={2.4} /></span>
      <div>
        <h2>{t('err.unresponsive.title', { port: inst.port })}</h2>
        <p>{t('err.unresponsive.body', { pid: inst.pid ?? '', secs: Math.round(silentFor / 1000) })}</p>
        <div class="trouble-act">
          <button class="btn sm" onclick={() => reconnect(inst)}><Icon name="restart" size={14} />{t('err.reconnect')}</button>
          <button class="btn sm danger" onclick={() => askForce(inst)}><Icon name="power" size={14} />{t('err.forceStop')}</button>
        </div>
      </div>
    </section>
  {:else if inst.status === 'stopped'}
    <section class="trouble calm" in:pop>
      <span class="t-badge" aria-hidden="true"><Icon name="power" size={24} stroke={2.4} /></span>
      <div>
        <h2>{t('stopped.title', { name: displayName })}</h2>
        <p>{t('stopped.body', { path: inst.configPath })}</p>
        <div class="trouble-act">
          <button class="btn sm mint" onclick={() => start(inst)}><Icon name="play" size={13} />{t('inst.start')}</button>
        </div>
      </div>
    </section>
  {/if}

  <div class="grid">
    <section class="inflight" class:stale={inst.status === 'unresponsive'}>
      <h2 class="section-title">{t('inst.inflight')}<span class="count">{inflight.length}</span></h2>
      {#if inst.status === 'unresponsive'}
        <!-- 控制通道沉默时拿不到新数据：照旧显示最后一次的快照，但明确标成过期的 -->
        <p class="stale-note"><Icon name="clock" size={14} />{t('inst.stale', { ago: ago(silentFor) })}</p>
      {/if}
      {#each inflight as e (e.key)}
        <article class="req" class:retrying={e.outcome === 'retrying'} in:pop={{ y: 8 }}>
          <button class="req-hit" onclick={() => openLog(e)} aria-label="#{e.seq} {e.path}"></button>
          <div class="r1">
            <span class="method">{e.method}</span>
            <code class="path mono">{e.path}</code>
            {#if e.model}<span class="model">{e.model}</span>{/if}
            <span class="seq mono">#{e.seq}</span>
          </div>
          {#if e.outcome === 'retrying'}
            <div class="r2">
              <span class="attempt">
                {attemptParts[0]}<span class="attempt-n"><FlipNumber value={e.attempts.length + (e.phase === 'attempting' ? 1 : 0)} /></span>{attemptParts[1]}
              </span>
              {#if lastReason(e)}
                {@const reason = lastReason(e)!}
                <span class="chip coral"><Icon name="cloudRain" size={13} />{t(`reason.${reason}`)}</span>
              {/if}
            </div>
            <div class="r3">
              <JellyBar value={e.phase === 'waiting' ? progress(e) : 1} label={t('inst.retryIn', { s: remaining(e) })} tone={e.phase === 'waiting' ? 'sky' : 'lilac'} />
              <span class="countdown">
                {#if e.phase === 'waiting'}
                  {remaining(e) > 0 ? t('inst.retryIn', { s: remaining(e) }) : t('inst.retryNow')}
                {:else}
                  {t('inst.attempting')}
                {/if}
              </span>
            </div>
          {:else}
            <div class="r3">
              <JellyBar value={progress(e)} label={t('inst.receiving')} tone="mint" />
              <span class="countdown">{t('inst.receiving')}</span>
            </div>
          {/if}
          <div class="r4">
            {#if e.stream && !inst.forwardOnly && inst.keepalive.interval > 0 && e.outcome === 'retrying'}
              <span class="beat">
                {#key e.heartbeats}
                  <span class="heart" in:pop={{ y: 0, scale: 0.4, duration: 600 }}><Icon name="heart" size={14} stroke={2.2} /></span>
                {/key}
                {t('inst.heartbeat', { n: e.heartbeats })}
              </span>
            {/if}
            <span class="waited">{t('inst.waited', { d: durLong(clock - e.time) })}</span>
            <span class="client">{e.client.split(' ')[0]}</span>
          </div>
        </article>
      {:else}
        <div class="quiet">
          <span class="q-icon" aria-hidden="true"><Icon name="check" size={18} stroke={2.6} /></span>
          <span>{t('inst.inflightEmpty')}</span>
          {#if inst.lastRequestAt}<span class="quiet-sub">{ago(store.now - inst.lastRequestAt)}</span>{/if}
        </div>
      {/each}

      {#if inst.status !== 'stopped'}
        <section class="panel hour">
          <div class="hour-head">
            <h2 class="section-title">{t('card.lastHour')}</h2>
            <span class="legend"><i class="k k-req"></i>{t('card.requests')} <b class="num">{inst.req1h}</b></span>
            <span class="legend"><i class="k k-retry"></i>{t('card.retries')} <b class="num">{inst.forwardOnly ? '—' : inst.retry1h}</b></span>
            <span class="legend"><Icon name="umbrella" size={13} />{t('logs.filter.rescued')} <b class="num">{rescuedHere}</b></span>
          </div>
          <Sparkline req={inst.spark.req} retry={inst.spark.retry} height={64} label={t('card.sparkLabel')} />
          {#if reasons.length}
            <div class="reasons">
              {#each reasons as [reason, n] (reason)}
                <span class="chip coral"><Icon name="cloudRain" size={12} />{t(`reason.${reason}`)}<b class="num">×{n}</b></span>
              {/each}
            </div>
          {/if}
        </section>
      {/if}
    </section>

    <section class="panel summary">
      <h2 class="section-title">{t('inst.config')}</h2>
      <dl>
        <dt>{t('cfgsum.upstream')}</dt>
        <dd><code class="mono">{inst.baseUrl}</code></dd>
        <dt>{t('cfgsum.listen')}</dt>
        <dd><code class="mono">{inst.listen}</code></dd>
        <dt>{t('cfgsum.proxy')}</dt>
        <dd>{#if inst.proxy}<code class="mono">{inst.proxy}</code>{:else}<span class="muted">{t('cfgsum.none')}</span>{/if}</dd>
        <dt>{t('cfgsum.backoff')}</dt>
        <dd>{#if inst.forwardOnly}<span class="muted">{t('mode.noRetry')}</span>{:else}{inst.backoffCap} {t('cfg.seconds')}{/if}</dd>
        <dt>{t('cfgsum.keepalive')}</dt>
        <dd>
          {#if inst.forwardOnly || inst.keepalive.interval === 0}
            <span class="muted">{t('cfgsum.keepaliveOff')}</span>
          {:else}
            {t('cfgsum.everyN', { n: inst.keepalive.interval })}<span class="chip">{t(`cfg.o.${inst.keepalive.trigger}`)}</span>
          {/if}
        </dd>
        <dt>{t('cfgsum.transform')}</dt>
        <dd>
          {#if inst.transform}
            <code class="mono">{inst.transform.program}</code>
            <span class="chip lilac">{t(`cfg.o.${inst.transform.mode}`)}</span>
            <span class="chip lilac">{t('mode.keys', { n: inst.transform.keys })}</span>
            <span class="chip lilac">{t('mode.channels', { n: inst.transform.channels.length })}</span>
          {:else}
            <span class="muted">{t('cfgsum.none')}</span>
          {/if}
        </dd>
        <dt>{t('cfgsum.bounded')}</dt>
        <dd>
          {#each inst.boundedPaths as p (p)}<code class="mono pattern">{p}</code>{:else}<span class="muted">{t('cfgsum.none')}</span>{/each}
        </dd>
        <dt>{t('cfgsum.diskCache')}</dt>
        <dd>{inst.diskCache ? t('cfgsum.on') : t('cfgsum.off')}</dd>
        <dt>{t('cfgsum.file')}</dt>
        <dd><code class="mono">{inst.configPath}</code></dd>
      </dl>
    </section>
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
  .r4 {
    font-size: var(--fs-xs);
    color: var(--text-3);
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
  .client {
    margin-left: auto;
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
  .reasons {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .reasons b {
    margin-left: 2px;
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
  .pattern {
    padding: 1px 7px;
    border-radius: var(--r-xs);
    background: var(--card-2-bg);
  }
</style>

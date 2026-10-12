<!--
  总览：一条状态横幅回答最要紧的问题——现在一切正常吗？
  吉祥物坐在横幅左缘、半个身子探出框外；右边是使命指标：无限重试一共救回了多少请求（翻牌计数）。
  下面是实例卡片、看护者与最近救回的请求。
-->
<script lang="ts">
  import { t, fmtNum, ago, uptime } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import type { Instance, LogEntry } from '../lib/types';
  import MascotView from '../components/MascotView.svelte';
  import { mascots } from '../lib/mascot/registry.svelte';
  import FlipNumber from '../components/FlipNumber.svelte';
  import Sparkline from '../components/Sparkline.svelte';
  import Icon from '../components/Icon.svelte';
  import ResultChip from '../components/ResultChip.svelte';

  const mood = $derived(store.mood);
  const running = $derived(store.instances.filter((i) => i.status !== 'stopped'));
  const req1h = $derived(store.instances.reduce((s, i) => s + i.req1h, 0));
  const retry1h = $derived(store.instances.reduce((s, i) => s + i.retry1h, 0));
  const unresponsive = $derived(store.instances.find((i) => i.status === 'unresponsive'));
  const rescuedEntry = $derived(store.rescuedKey ? store.logs.find((e) => e.key === store.rescuedKey) : undefined);
  const quietMins = $derived.by(() => {
    const last = Math.max(0, ...store.instances.map((i) => i.lastRequestAt ?? 0));
    return Math.max(1, Math.round((store.now - last) / 60_000));
  });

  const retryWhere = $derived.by(() => {
    const ports = [...new Set(store.retrying.map((e) => e.port))];
    const en = ui.lang === 'en';
    return ports
      .map((p) => {
        const alias = store.instance(p)?.alias;
        if (!alias) return String(p);
        return en ? `${alias} (${p})` : `${alias}（${p}）`;
      })
      .join(en ? ', ' : '、');
  });

  const headline = $derived.by(() => {
    switch (mood) {
      case 'worried':
        return t('ov.hero.worried', { port: unresponsive?.port ?? '' });
      case 'retrying':
        return t('ov.hero.retrying', { n: store.retrying.length });
      case 'rescued':
        return t('ov.hero.rescued');
      case 'working':
        return t('ov.hero.working', { n: store.inflight.length });
      case 'sleeping':
        return t('ov.hero.sleeping');
      default:
        return t('ov.hero.idle');
    }
  });

  const sub = $derived.by(() => {
    switch (mood) {
      case 'worried':
        return t('ov.hero.worriedSub', { secs: Math.round((store.now - (unresponsive?.silentSince ?? store.now)) / 1000) });
      case 'retrying':
        return t('ov.hero.retryingSub', { where: retryWhere });
      case 'rescued':
        return rescuedEntry
          ? t('ov.hero.rescuedSub', { path: rescuedEntry.path.split('?')[0], n: rescuedEntry.attempts.length })
          : t('ov.hero.rescuedSubPlain');
      case 'working':
        return t('ov.hero.workingSub');
      case 'sleeping':
        return t('ov.hero.sleepingSub', { mins: quietMins });
      default:
        return t('ov.hero.idleSub', { n: running.length });
    }
  });

  const recent = $derived(
    store.logs
      .filter((e) => e.outcome === 'rescued')
      .slice(-4)
      .reverse(),
  );

  function retryingOn(port: number) {
    return store.retrying.filter((e) => e.port === port).length;
  }
  function inflightOn(port: number) {
    return store.inflight.filter((e) => e.port === port).length;
  }

  function openLog(e: LogEntry) {
    ui.selectedLog = e.key;
    ui.go('logs');
  }

  async function startInstance(inst: Instance) {
    await store.start(inst.port);
    overlays.toast(t('toast.started', { name: inst.alias ?? inst.port }));
  }

  // 不给端口：按默认配置启动第一个实例
  async function startFirst() {
    await store.start();
    overlays.toast(t('toast.started', { name: 'default' }));
  }
</script>

{#if store.instances.length === 0}
  <!-- 空状态：没有任何实例。吉祥物在睡觉，给出唯一的下一步 -->
  <section class="empty">
    {#if mascots.current}<MascotView mood="sleeping" size="hero" px={150} />{/if}
    <h1>{t('empty.instances.title')}</h1>
    <p>{t('empty.instances.body')}</p>
    <button class="btn primary big" onclick={startFirst}>
      <Icon name="play" size={16} />{t('empty.instances.cta')}
    </button>
    <span class="hint">{t('empty.instances.hint', { path: '' })}<code class="mono">~/.aproxy/config.toml</code></span>
  </section>
{:else}
  <section class="hero mood-{mood}" class:bare={!mascots.current}>
    {#if mood === 'retrying'}<div class="rain" aria-hidden="true"></div>{/if}
    {#if mascots.current}
      <div class="hero-mascot">
        <MascotView {mood} retries={store.maxRetries} size="hero" px={128} />
      </div>
    {/if}
    <div class="hero-text">
      <h1>{headline}</h1>
      <p>{sub}</p>
      <div class="stats">
        <span class="stat"><Icon name="hourglass" size={15} /><span class="lbl">{t('ov.inflight')}</span><b class="num">{store.inflight.length}</b></span>
        <span class="stat"><Icon name="list" size={15} /><span class="lbl">{t('ov.req1h')}</span><b class="num">{fmtNum(req1h)}</b></span>
        <span class="stat"><Icon name="umbrella" size={15} /><span class="lbl">{t('ov.retry1h')}</span><b class="num">{fmtNum(retry1h)}</b></span>
      </div>
    </div>
    <div class="rescued">
      <span class="r-label">{t('ov.rescued')}</span>
      <span class="r-count"><FlipNumber value={store.rescuedTotal} /></span>
      <span class="r-unit">{t('ov.rescuedUnit')}</span>
      <span class="r-today chip mint"><Icon name="sparkles" size={13} />{t('ov.rescuedToday', { n: store.rescuedToday })}</span>
    </div>
  </section>

  <div class="section-head">
    <h2 class="section-title">{t('ov.instances')}<span class="count">{store.instances.length}</span></h2>
    <button class="btn sm" onclick={() => overlays.toast(t('toast.protoOnly'), 'lilac')}>
      <Icon name="plus" size={14} stroke={2.4} />{t('ov.startInstance')}
    </button>
  </div>

  <div class="cards">
    {#each store.instances as inst (inst.port)}
      {@const retrying = retryingOn(inst.port)}
      <article class="icard st-{inst.status}" class:retrying={retrying > 0}>
        <button class="hit" onclick={() => ui.go('instance', inst.port)} aria-label="{inst.port} {inst.alias ?? ''}"></button>
        <header>
          <span class="port">{inst.port}</span>
          {#if inst.alias}<span class="alias">{inst.alias}</span>{/if}
        </header>
        <div class="badges">
          {#if inst.status === 'stopped'}
            <span class="chip"><span class="dot gray"></span>{t('status.stopped')}</span>
          {:else if inst.status === 'unresponsive'}
            <span class="chip solid coral"><Icon name="alert" size={13} stroke={2.4} />{t('status.unresponsive')}</span>
          {:else if inst.status === 'restarting'}
            <span class="chip butter"><span class="spin"></span>{t('status.restarting')}</span>
          {:else if retrying > 0}
            <span class="chip solid sky"><Icon name="umbrella" size={13} stroke={2.4} />{t('card.retryingN', { n: retrying })}</span>
          {:else}
            <span class="chip mint"><span class="dot mint" class:breathe={inst.status === 'active'}></span>{t(`status.${inst.status}`)}</span>
          {/if}
          {#if inst.status !== 'stopped'}
            {#if inst.forwardOnly}
              <span class="hazard"><span class="tape"></span>{t('mode.forwardOnly')}<span class="sep">/</span>{t('mode.noRetry')}</span>
            {:else if inst.transform}
              <span class="chip lilac"><Icon name="shuffle" size={12} stroke={2.4} />{t('mode.keys', { n: inst.transform.keys })}</span>
            {:else}
              <span class="chip"><Icon name="upstream" size={12} stroke={2.4} /><span class="host">{inst.baseUrl.replace(/^https?:\/\//, '')}</span></span>
            {/if}
          {/if}
        </div>

        {#if inst.status === 'stopped'}
          <p class="stopped-hint">{t('card.stoppedHint', { path: '' })}<code class="mono">{inst.configPath}</code></p>
          <div class="stopped-act">
            <button class="btn sm mint" onclick={() => startInstance(inst)}><Icon name="play" size={13} />{t('card.start')}</button>
          </div>
        {:else}
          <Sparkline req={inst.spark.req} retry={inst.spark.retry} label={t('card.sparkLabel')} />
          <dl class="metrics">
            <div><dt><i class="k req"></i>{t('card.requests')}</dt><dd class="num">{fmtNum(inst.req1h)}</dd></div>
            <div><dt><i class="k retry"></i>{t('card.retries')}</dt><dd class="num">{inst.forwardOnly ? '—' : fmtNum(inst.retry1h)}</dd></div>
            <div><dt>{t('card.inflight')}</dt><dd class="num">{inflightOn(inst.port)}</dd></div>
          </dl>
          <footer>
            {#if inst.status === 'unresponsive'}
              <span class="err"><Icon name="plug" size={13} /><span class="ell">{t('ov.hero.worried', { port: inst.port })}</span></span>
            {:else if inst.lastError && store.now - inst.lastError.at < 3_600_000}
              <span class="err" title={t('card.lastError')}>
                <Icon name="cloudRain" size={13} /><span class="ell">{t(`reason.${inst.lastError.reason}`)}</span><span class="when">{ago(store.now - inst.lastError.at)}</span>
              </span>
            {:else}
              <span class="ok"><Icon name="check" size={13} stroke={2.4} />{t('card.noErrors')}</span>
            {/if}
            <span class="up">
              {inst.status === 'idle' && inst.lastRequestAt
                ? t('card.idleFor', { d: uptime(store.now - inst.lastRequestAt) })
                : t('card.uptime', { d: uptime(store.now - inst.startedAt) })}
            </span>
          </footer>
        {/if}
      </article>
    {/each}
  </div>

  <div class="lower">
    <section class="panel watchdog" class:off={!store.watchdogOn}>
      <div class="wd-badge"><Icon name={store.watchdogOn ? 'shield' : 'shieldOff'} size={22} /></div>
      <div class="wd-body">
        <h3>{t('wd.title')}</h3>
        {#if store.watchdogOn}
          <p class="wd-main">{running.length ? t('wd.watching', { n: running.length }) : t('wd.nothing')}</p>
          {#if store.watchdog}
            <p class="wd-meta">{t('wd.lastScan', { ago: ago(store.now - store.watchdog.lastScanAt) })}</p>
            {#each store.watchdog.respawns as r (r.at)}
              <p class="wd-event"><Icon name="restart" size={13} />{t('wd.respawned', { port: r.port, ago: ago(store.now - r.at) })}</p>
            {/each}
          {/if}
        {:else}
          <p class="wd-main">{t('wd.disabled')}</p>
          <button class="btn sm mint" onclick={() => store.updateSettings({ watchdog: true })}>{t('wd.enable')}</button>
        {/if}
      </div>
    </section>

    <section class="panel rescues">
      <div class="rs-head">
        <h3>{t('ov.recentRescues')}</h3>
        <button class="btn ghost sm" onclick={() => ui.go('logs')}>{t('ov.viewAll')}<Icon name="chevronRight" size={14} /></button>
      </div>
      <ul>
        {#each recent as e (e.key)}
          <li>
            <button onclick={() => openLog(e)}>
              <ResultChip entry={e} />
              <span class="rs-path mono">{e.path.split('?')[0]}</span>
              <span class="rs-model">{e.model ?? ''}</span>
              <span class="rs-line">{t('ov.rescueLine', { n: e.attempts.length })}</span>
              <span class="rs-when">{ago(store.now - e.time)}</span>
            </button>
          </li>
        {/each}
      </ul>
    </section>
  </div>
{/if}

<style>
  /* ---------- 状态横幅 ---------- */
  .hero {
    --tone: var(--mint-soft);
    position: relative;
    display: grid;
    grid-template-columns: 132px 1fr auto;
    align-items: center;
    gap: 22px;
    min-height: 148px;
    margin: 10px 0 26px 26px;
    padding: 18px 26px 18px 0;
    border-radius: var(--r-xl);
    /* 横幅靠状态底色分层；细边取底色本身再压深一点，换状态时边也跟着换色相，
       不会在粉彩底上框一圈中性灰 */
    border: var(--line-w) solid color-mix(in oklab, var(--tone) 72%, var(--line-strong));
    background: var(--tone);
    box-shadow: var(--shadow-2);
    transition:
      background-color var(--dur-slow) var(--ease-soft),
      border-color var(--dur-slow) var(--ease-soft);
  }
  /* 横幅底色跟着整体状态：空闲薄荷、忙碌丁香、重试天蓝、救回奶黄、出错珊瑚、睡着素白 */
  .hero.mood-retrying {
    --tone: var(--sky-soft);
  }
  .hero.mood-worried {
    --tone: var(--coral-soft);
  }
  .hero.mood-working {
    --tone: var(--lilac-soft);
  }
  .hero.mood-rescued {
    --tone: var(--butter-soft);
  }
  .hero.mood-sleeping {
    --tone: var(--card-2);
  }
  /* 设置里选了「不显示」吉祥物：让出左边那一栏，文字回到正常的内边距 */
  .hero.bare {
    grid-template-columns: 1fr auto;
    margin-left: 0;
    padding-left: 26px;
  }
  /* 重试时横幅里飘着很淡的斜雨丝：天气就是状态 */
  .rain {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    overflow: hidden;
    pointer-events: none;
    background: repeating-linear-gradient(
      105deg,
      transparent 0 26px,
      color-mix(in oklab, var(--sky-deep) 13%, transparent) 26px 27.5px
    );
    background-size: 100% 200%;
    animation: rain 1.6s linear infinite;
    mask-image: linear-gradient(90deg, transparent, #000 20%, #000 70%, transparent);
  }
  @keyframes rain {
    from { background-position: 0 0; }
    to { background-position: -40px 120px; }
  }
  .hero-mascot {
    /* 吉祥物探出横幅左缘：打破边框，是整页唯一「出格」的元素；落地阴影由吉祥物自己画 */
    margin-left: -40px;
    margin-top: -26px;
  }
  .hero-text h1 {
    margin: 0 0 6px;
    font-size: var(--fs-2xl);
    font-weight: 620;
    line-height: 1.15;
    letter-spacing: -0.005em;
  }
  .hero-text p {
    margin: 0 0 14px;
    max-width: 36em;
    color: var(--text-2);
  }
  .stats {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .stat {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px 0 10px;
    border-radius: var(--r-pill);
    border: var(--line-w-thin) solid var(--line);
    background: var(--card-bg);
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .stat b {
    color: var(--text);
    font-weight: 620;
  }
  .rescued {
    display: grid;
    justify-items: center;
    gap: 6px;
    padding: 14px 18px 14px;
    border-radius: var(--r-lg);
    /* 虚线框像一张「票根」 */
    border: var(--line-w) dashed var(--line-dash);
    background: color-mix(in oklab, var(--card) 55%, transparent);
    text-align: center;
  }
  .r-label,
  .r-unit {
    color: var(--text-2);
    font-size: var(--fs-sm);
    font-weight: 500;
  }
  .r-count {
    font-size: 34px;
    font-weight: 640;
    color: var(--text);
  }
  .r-today {
    margin-top: 2px;
  }

  /* ---------- 空状态 ---------- */
  .empty {
    display: grid;
    justify-items: center;
    text-align: center;
    gap: 6px;
    padding: 70px 24px;
  }
  .empty h1 {
    margin: 18px 0 0;
    font-size: var(--fs-2xl);
    font-weight: 620;
  }
  .empty p {
    margin: 0 0 18px;
    max-width: 30em;
    color: var(--text-2);
  }
  .btn.big {
    --btn-h: 42px;
    padding: 0 22px;
    font-size: var(--fs-lg);
  }
  .hint {
    margin-top: 10px;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }

  /* ---------- 实例卡片 ---------- */
  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 0 2px 12px;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(228px, 1fr));
    gap: 16px;
    margin-bottom: 26px;
  }
  .icard {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 15px 12px;
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    box-shadow: var(--shadow-2);
    transition:
      transform var(--dur-mid) var(--ease-spring),
      box-shadow var(--dur-mid) var(--ease-soft);
  }
  .icard:hover {
    transform: translateY(var(--lift));
    box-shadow: var(--shadow-2-hover);
  }
  .icard:has(.hit:active) {
    transform: translateY(0) scale(0.99);
    box-shadow: var(--shadow-1);
    transition-duration: 80ms;
  }
  /* 已停止：去掉底色与阴影，只剩一圈虚线，读作「这里本该有个实例」 */
  .icard.st-stopped {
    background: transparent;
    border-style: dashed;
    border-color: var(--line-dash);
    box-shadow: none;
  }
  .icard.st-stopped:hover {
    box-shadow: var(--shadow-1);
  }
  .icard.st-unresponsive {
    background: var(--coral-soft);
  }
  /* 整张卡可点（进入实例详情），卡内的其它按钮叠在它上面 */
  .hit {
    position: absolute;
    inset: 0;
    z-index: 0;
    border: 0;
    border-radius: inherit;
    background: transparent;
  }
  .icard > :not(.hit) {
    position: relative;
    z-index: 1;
    pointer-events: none;
  }
  .icard :global(button:not(.hit)),
  .icard :global(.spark) {
    pointer-events: auto;
  }
  .icard header {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .port {
    font-size: var(--fs-xl);
    font-weight: 640;
    letter-spacing: 0.01em;
    font-variant-numeric: tabular-nums;
  }
  .alias {
    color: var(--text-2);
    font-weight: 550;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .grow {
    flex: 1;
  }
  .badges {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    min-height: 26px;
    align-items: center;
  }
  .host {
    max-width: 120px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hazard .sep {
    opacity: 0.5;
  }
  .metrics {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 4px;
    margin: 2px 0 0;
  }
  .metrics div {
    display: flex;
    flex-direction: column;
  }
  .metrics dt {
    display: flex;
    align-items: center;
    gap: 5px;
    color: var(--text-3);
    font-size: var(--fs-xs);
  }
  .metrics dd {
    margin: 0;
    font-size: var(--fs-lg);
    font-weight: 620;
  }
  .k {
    width: 8px;
    height: 8px;
    border-radius: 3px;
  }
  /* 图例色块与走势图同色（图表专用令牌） */
  .k.req {
    background: var(--chart-req);
  }
  .k.retry {
    background: var(--chart-retry);
  }
  .icard footer {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding-top: 9px;
    border-top: var(--line-w-thin) dashed var(--line-soft);
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
  .err,
  .ok {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
  }
  .err {
    color: var(--coral-deep);
    font-weight: 550;
  }
  .ok {
    color: var(--mint-deep);
  }
  /* 原因可以被截断，时间不能：时间是判断「还在不在出错」的关键 */
  .ell {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .when {
    flex: none;
    color: var(--text-3);
    font-weight: 450;
    margin-left: 2px;
  }
  .up {
    white-space: nowrap;
    padding-left: 18px;
  }
  .stopped-hint {
    margin: 4px 0 0;
    color: var(--text-3);
    font-size: var(--fs-sm);
    line-height: 1.6;
  }
  .stopped-hint code {
    color: var(--text-2);
  }
  .stopped-act {
    margin-top: auto;
  }
  .spin {
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

  /* ---------- 看护者与最近救回 ---------- */
  .lower {
    display: grid;
    grid-template-columns: minmax(240px, 0.8fr) 1.6fr;
    gap: 16px;
  }
  .panel h3 {
    margin: 0;
    font-size: var(--fs-md);
    font-weight: 620;
  }
  .watchdog {
    display: flex;
    gap: 14px;
    padding: 16px;
  }
  .wd-badge {
    display: grid;
    place-items: center;
    flex: none;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    --badge: var(--mint);
    background: var(--badge);
    color: var(--on-fill);
    box-shadow: 0 3px 10px -3px color-mix(in oklab, var(--badge) 90%, transparent);
  }
  .watchdog.off .wd-badge {
    --badge: var(--butter);
  }
  .wd-body p {
    margin: 2px 0 0;
  }
  .wd-main {
    color: var(--text);
    font-weight: 550;
  }
  .wd-meta {
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .wd-event {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 8px !important;
    padding: 4px 10px;
    border-radius: var(--r-pill);
    background: var(--card-2-bg);
    color: var(--text-2);
    font-size: var(--fs-xs);
    width: fit-content;
  }
  .watchdog.off .btn {
    margin-top: 10px;
  }
  .rescues {
    padding: 12px 10px 8px 16px;
  }
  .rs-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 4px;
  }
  .rescues ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .rescues li button {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto auto auto;
    align-items: center;
    gap: 12px;
    width: 100%;
    height: 38px;
    padding: 0 10px 0 4px;
    border: 0;
    border-radius: var(--r-sm);
    background: transparent;
    text-align: left;
    font-size: var(--fs-sm);
  }
  .rescues li button:hover {
    background: var(--hover);
  }
  .rs-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
  .rs-model,
  .rs-when {
    color: var(--text-3);
    white-space: nowrap;
  }
  .rs-line {
    color: var(--sky-deep);
    font-weight: 550;
    white-space: nowrap;
  }
</style>

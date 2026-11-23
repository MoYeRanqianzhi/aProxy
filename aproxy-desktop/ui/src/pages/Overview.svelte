<!--
  总览：一条状态横幅回答最要紧的问题——现在一切正常吗？
  吉祥物坐在横幅左缘、半个身子探出框外；右边是使命指标：无限重试一共救回了多少请求（翻牌计数）。
  下面是实例卡片、看护者与最近救回的请求。
-->
<script lang="ts">
  import { t, fmtNum, ago, uptime } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { refKey, sameRef, store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import type { Card, Row, Watchdog } from '../lib/backend/contract';
  import { toFailure } from '../lib/backend/link';
  import { codeText, faultText } from '../lib/faults';
  import { aliasOf, controllable, forwardOnly, hourBuckets, lastHour, rescued, statusOf } from '../lib/view';
  import MascotView from '../components/MascotView.svelte';
  import { mascots } from '../lib/mascot/registry.svelte';
  import FlipNumber from '../components/FlipNumber.svelte';
  import Sparkline from '../components/Sparkline.svelte';
  import Icon from '../components/Icon.svelte';
  import ResultChip from '../components/ResultChip.svelte';
  import StartMenu from '../components/StartMenu.svelte';
  import ExtensionMounts from '../components/ExtensionMounts.svelte';

  const mood = $derived(store.mood);
  /** 在运行的：有可信的运行记录、进程还在 */
  const running = $derived(store.cards.filter((c) => c.run !== null && c.liveness !== 'down'));

  /** 近一小时的请求或重试：各实例分桶之和；没有一个实例提供分桶时为 null，显示「—」 */
  function hourSum(key: 'requests' | 'retries'): number | null {
    const parts = store.cards.map((c) => lastHour(c.activity?.buckets, key)).filter((v) => v !== null);
    return parts.length ? parts.reduce((s, v) => s + v, 0) : null;
  }
  const req1h = $derived(hourSum('requests'));
  const retry1h = $derived(hourSum('retries'));
  /** 各实例这次运行以来救回的请求之和：实例重启后从头算，所以界面写明「自各实例这次启动以来」 */
  const rescuedTotal = $derived(store.cards.reduce((s, c) => s + (c.activity?.rescuedTotal ?? 0), 0));
  const unresponsive = $derived(store.cards.find((c) => c.liveness === 'unresponsive'));
  /** 正在庆祝的那条；不在最近 50 条里就只说「救回了一个」 */
  const rescuedRow = $derived(store.recent.find((r) => sameRef(r.ref, store.rescuedRef)));
  const quietMins = $derived.by(() => {
    const last = Math.max(0, ...store.cards.map((c) => c.activity?.lastRequestAt ?? 0));
    return Math.max(1, Math.round((store.now - last) / 60_000));
  });

  /** 某个实例的名字：有别名写「别名（端口）」，没有就是端口 */
  function nameOf(c: Card): string {
    const alias = aliasOf(c);
    if (!alias) return String(c.port);
    return ui.lang === 'en' ? `${alias} (${c.port})` : `${alias}（${c.port}）`;
  }

  const retryWhere = $derived(
    store.cards
      .filter((c) => (c.activity?.retrying ?? 0) > 0)
      .map(nameOf)
      .join(ui.lang === 'en' ? ', ' : '、'),
  );

  const headline = $derived.by(() => {
    switch (mood) {
      case 'worried':
        return t('ov.hero.worried', { port: unresponsive?.port ?? '' });
      case 'retrying':
        return t('ov.hero.retrying', { n: store.retrying });
      case 'rescued':
        return t('ov.hero.rescued');
      case 'working':
        return t('ov.hero.working', { n: store.inflight });
      case 'sleeping':
        return t('ov.hero.sleeping');
      default:
        return t('ov.hero.idle');
    }
  });

  const sub = $derived.by(() => {
    switch (mood) {
      case 'worried':
        return t('ov.hero.worriedSub');
      case 'retrying':
        return t('ov.hero.retryingSub', { where: retryWhere });
      case 'rescued':
        return rescuedRow ? t('ov.hero.rescuedSub', { path: rescuedRow.path.split('?')[0], n: rescuedRow.attempts }) : t('ov.hero.rescuedSubPlain');
      case 'working':
        return t('ov.hero.workingSub');
      case 'sleeping':
        return t('ov.hero.sleepingSub', { mins: quietMins });
      default:
        return t('ov.hero.idleSub', { n: running.length });
    }
  });

  /** 最近救回的几条：最近 50 条里的，新的在前 */
  const recentRescues = $derived(store.recent.filter(rescued).slice(0, 4));

  function openLog(r: Row) {
    ui.selectedRef = r.ref;
    ui.go('logs');
  }

  function fail(raw: unknown) {
    overlays.toast(faultText(toFailure(raw)), 'coral');
  }

  /** 空状态里的「启动第一个实例」：用默认配置 */
  let startingFirst = $state(false);
  async function startFirst() {
    if (startingFirst) return;
    startingFirst = true;
    try {
      const choices = await store.listConfigs();
      const choice = choices.find((c) => c.isDefault) ?? choices[0];
      if (!choice) {
        overlays.toast(t('start.none'), 'coral');
        return;
      }
      await store.start(choice.target);
      overlays.toast(t('toast.started', { name: choice.label }));
    } catch (raw) {
      fail(raw);
    } finally {
      startingFirst = false;
    }
  }

  /** 实例不在了、但留着「应当在运行」的记录：照记录重新启动 */
  async function restore(c: Card) {
    if (!c.expectedRestore) return;
    try {
      await store.restore(c.port, c.expectedRestore);
      overlays.toast(t('toast.started', { name: nameOf(c) }));
    } catch (raw) {
      fail(raw);
    }
  }

  /** 放弃恢复：删掉那条记录，卡片随之消失。配置文件不动 */
  function forget(c: Card) {
    const token = c.expectedRestore;
    if (!token) return;
    overlays.ask({
      title: t('card.forgetTitle', { name: nameOf(c) }),
      body: t('card.forgetBody'),
      okLabel: t('card.forget'),
      danger: false,
      onOk: () => void store.forgetRestore(c.port, token).catch(fail),
    });
  }

  async function enableWatchdog() {
    try {
      await store.setWatchdog(true);
    } catch (raw) {
      fail(raw);
    }
  }

  function restartLine(r: Watchdog['restarts'][number]): string {
    const vars = { port: r.port, ago: ago(store.now - r.at) };
    if (!r.ok) return t('wd.restartFailed', vars);
    if (r.kind === 'hang') return t('wd.respawned', vars);
    return t(r.kind === 'crash' ? 'wd.restartedCrash' : 'wd.restarted', vars);
  }
</script>

{#if !store.connected}
  <!-- 第一次读完之前不下结论：「没有实例」要等真的读到了才能讲 -->
  <section class="empty connecting">
    <p>{store.readFault ? faultText(store.readFault) : t('ov.connecting')}</p>
  </section>
{:else if store.cards.length === 0}
  <!-- 扩展挂在总览顶上的内容，没有实例时也照样显示：否则扩展的作者看不出它为什么没出来；要不要在空状态下显示什么，由扩展自己按实例列表决定 -->
  <ExtensionMounts point="overview.top" />
  <!-- 空状态：没有任何实例。吉祥物在睡觉，给出唯一的下一步 -->
  <section class="empty">
    {#if mascots.current}<MascotView mood="sleeping" size="hero" px={150} />{/if}
    <h1>{t('empty.instances.title')}</h1>
    <p>{t('empty.instances.body')}</p>
    <button class="btn primary big" onclick={startFirst} disabled={startingFirst}>
      <Icon name="play" size={16} />{t('empty.instances.cta')}
    </button>
  </section>
{:else}
  <ExtensionMounts point="overview.top" />
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
        <span class="stat"><Icon name="hourglass" size={15} /><span class="lbl">{t('ov.inflight')}</span><b class="num">{store.inflight}</b></span>
        <span class="stat"><Icon name="list" size={15} /><span class="lbl">{t('ov.req1h')}</span><b class="num">{req1h === null ? '—' : fmtNum(req1h)}</b></span>
        <span class="stat"><Icon name="umbrella" size={15} /><span class="lbl">{t('ov.retry1h')}</span><b class="num">{retry1h === null ? '—' : fmtNum(retry1h)}</b></span>
      </div>
    </div>
    <div class="rescued">
      <span class="r-label">{t('ov.rescued')}</span>
      <span class="r-count"><FlipNumber value={rescuedTotal} /></span>
      <span class="r-unit">{t('ov.rescuedUnit')}</span>
      <span class="r-today chip mint"><Icon name="sparkles" size={13} />{t('ov.rescuedSince')}</span>
    </div>
  </section>

  {#if store.readFault}
    <div class="banner coral read-fault"><Icon name="alert" size={16} /><p>{faultText(store.readFault)}</p></div>
  {/if}

  <div class="section-head">
    <h2 class="section-title">{t('ov.instances')}<span class="count">{store.cards.length}</span></h2>
    <StartMenu />
  </div>

  <div class="cards">
    {#each store.cards as inst (inst.port)}
      {@const status = statusOf(inst)}
      {@const retrying = inst.activity?.retrying ?? 0}
      {@const spark = hourBuckets(inst.activity?.buckets)}
      {@const req = lastHour(inst.activity?.buckets, 'requests')}
      {@const retries = lastHour(inst.activity?.buckets, 'retries')}
      {@const lastError = inst.activity?.lastError ?? null}
      <article class="icard st-{status}" class:retrying={retrying > 0}>
        <button class="hit" onclick={() => ui.go('instance', inst.port)} aria-label="{inst.port} {aliasOf(inst) ?? ''}"></button>
        <header>
          <span class="port">{inst.port}</span>
          {#if aliasOf(inst)}<span class="alias" title={inst.metadata?.aliases.join(', ')}>{aliasOf(inst)}</span>{/if}
        </header>
        <div class="badges">
          {#if status === 'restore'}
            <span class="chip butter"><Icon name="clock" size={13} stroke={2.4} />{t('status.restore')}</span>
          {:else if status === 'unresponsive'}
            <span class="chip solid coral"><Icon name="alert" size={13} stroke={2.4} />{t('status.unresponsive')}</span>
          {:else if status === 'starting' || status === 'restarting' || status === 'stopping'}
            <span class="chip butter"><span class="spin"></span>{t(`status.${status}`)}</span>
          {:else if status === 'down' || status === 'incompatible' || status === 'unknown'}
            <span class="chip"><span class="dot gray"></span>{t(`status.${status}`)}</span>
          {:else if retrying > 0}
            <span class="chip solid sky"><Icon name="umbrella" size={13} stroke={2.4} />{t('card.retryingN', { n: retrying })}</span>
          {:else}
            <span class="chip mint"><span class="dot mint" class:breathe={status === 'active'}></span>{t(`status.${status}`)}</span>
          {/if}
          {#if inst.metadata}
            {#if forwardOnly(inst)}
              <span class="hazard"><span class="tape"></span>{t('mode.forwardOnly')}<span class="sep">/</span>{t('mode.noRetry')}</span>
            {:else}
              <span class="chip"><Icon name="upstream" size={12} stroke={2.4} /><span class="host">{inst.metadata.baseUrl.replace(/^https?:\/\//, '')}</span></span>
            {/if}
          {/if}
        </div>

        {#if status === 'restore'}
          <p class="stopped-hint">{t(inst.restoreReason === 'config-missing' ? 'card.restoreMissing' : 'card.restoreHint')}</p>
          {#if inst.expectedRestore}
            <div class="stopped-act">
              <button class="btn sm mint" onclick={() => restore(inst)}><Icon name="play" size={13} />{t('card.restore')}</button>
              <button class="btn sm ghost" onclick={() => forget(inst)}>{t('card.forget')}</button>
            </div>
          {/if}
        {:else if !inst.activity}
          <p class="stopped-hint">{t(controllable(inst) ? 'card.noActivity' : 'card.noRun')}</p>
        {:else}
          {#if spark}
            <Sparkline req={spark.req} retry={spark.retry} label={t('card.sparkLabel', { m: Math.max(1, Math.round(inst.activity.buckets.widthSecs / 60)) })} />
          {/if}
          <dl class="metrics">
            <div><dt><i class="k req"></i>{t('card.requests')}</dt><dd class="num">{req === null ? '—' : fmtNum(req)}</dd></div>
            <div><dt><i class="k retry"></i>{t('card.retries')}</dt><dd class="num">{forwardOnly(inst) || retries === null ? '—' : fmtNum(retries)}</dd></div>
            <div><dt>{t('card.inflight')}</dt><dd class="num">{inst.activity.inFlight}</dd></div>
          </dl>
          <footer>
            {#if status === 'unresponsive'}
              <span class="err"><Icon name="plug" size={13} /><span class="ell">{t('ov.hero.worried', { port: inst.port })}</span></span>
            {:else if inst.historyUnavailable === 'source-redaction-missing'}
              <span class="err" title={t('card.oldVersionHint')}><Icon name="restart" size={13} /><span class="ell">{t('card.oldVersion')}</span></span>
            {:else if lastError && store.now - lastError.at < 3_600_000}
              <span class="err" title={t('card.lastError')}>
                <Icon name="cloudRain" size={13} /><span class="ell">{codeText(lastError.code, lastError.status)}</span><span class="when">{ago(store.now - lastError.at)}</span>
              </span>
            {:else}
              <span class="ok"><Icon name="check" size={13} stroke={2.4} />{t('card.noErrors')}</span>
            {/if}
            {#if inst.startedAt !== null}
              <span class="up">
                {status === 'idle' && inst.activity.requestsTotal > 0
                  ? t('card.idleFor', { d: uptime(store.now - inst.activity.lastRequestAt) })
                  : t('card.uptime', { d: uptime(store.now - inst.startedAt) })}
              </span>
            {/if}
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
        {#if !store.watchdogOn}
          <p class="wd-main">{t('wd.disabled')}</p>
          <button class="btn sm mint" onclick={enableWatchdog}>{t('wd.enable')}</button>
        {:else if store.watchdog === null}
          <p class="wd-main">{t('wd.notRunning')}</p>
        {:else}
          <p class="wd-main">{running.length ? t('wd.watching', { n: running.length }) : t('wd.nothing')}</p>
          <p class="wd-meta">
            {t(store.watchdog.state === 'stale' ? 'wd.stale' : 'wd.lastScan', { ago: ago(store.now - store.watchdog.lastScanAt) })}
          </p>
          {#each store.watchdog.restarts as r (`${r.port}/${r.at}`)}
            <p class="wd-event"><Icon name="restart" size={13} />{restartLine(r)}</p>
          {/each}
        {/if}
      </div>
    </section>

    <section class="panel rescues">
      <div class="rs-head">
        <h3>{t('ov.recentRescues')}</h3>
        <button class="btn ghost sm" onclick={() => ui.go('logs')}>{t('ov.viewAll')}<Icon name="chevronRight" size={14} /></button>
      </div>
      <ul>
        {#each recentRescues as r (refKey(r.ref))}
          <li>
            <button onclick={() => openLog(r)}>
              <ResultChip row={r} />
              <span class="rs-path mono">{r.path.split('?')[0]}</span>
              <span class="rs-model">{r.model ?? ''}</span>
              <span class="rs-line">{t('ov.rescueLine', { n: r.attempts })}</span>
              <span class="rs-when">{ago(store.now - (r.endedAt ?? r.startedAt))}</span>
            </button>
          </li>
        {:else}
          <li class="rs-none">{t('ov.noRescues')}</li>
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
  .empty.connecting {
    padding-top: 120px;
  }
  /* 重读失败：上一次读到的内容还在下面显示着，这里说明它可能不是最新的 */
  .read-fault {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: -10px 0 20px;
    padding: 10px 14px;
    border-radius: var(--r-md);
    background: var(--coral-soft);
    color: var(--coral-deep);
    font-size: var(--fs-sm);
  }
  .read-fault p {
    margin: 0;
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
  .stopped-act {
    position: relative;
    margin-top: auto;
    display: flex;
    gap: 8px;
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
  .rs-none {
    padding: 10px 4px;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .rs-line {
    color: var(--sky-deep);
    font-weight: 550;
    white-space: nowrap;
  }
</style>

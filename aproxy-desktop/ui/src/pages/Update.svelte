<!--
  更新：当前版本（含变体：带界面 / 无界面）、可装的最新版本，以及安装进度的几个阶段：
  下载并校验 → 通知实例 → 替换程序 → 逐个重启 → 确认 → 完成。

  检查更新要联网，所以只在打开这一页、换渠道、或者用户点「再检查一次」时才查。
  安装由 Rust 交给安装程序去做，进度经 update-progress 事件整份推过来（store.job）：离开本页再回来照样看得到。
  安装时带回去的是检查时拿到的句柄（check），不是版本号：检查之后可装的版本变了，Rust 会拒绝，界面再查一次。
-->
<script lang="ts">
  import { t, ago, bytes } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import { pop } from '../lib/motion';
  import type { InstallPhase, UpdateInfo } from '../lib/backend/contract';
  import { toFailure, type Failure } from '../lib/backend/link';
  import { faultText } from '../lib/faults';
  import { aliasOf } from '../lib/view';
  import Icon, { type IconName } from '../components/Icon.svelte';
  import MascotView from '../components/MascotView.svelte';
  import { mascots } from '../lib/mascot/registry.svelte';
  import JellyBar from '../components/JellyBar.svelte';
  import Segmented from '../components/Segmented.svelte';

  /** 界面上的阶段：安装程序的阶段更细，几个合成一个给人看 */
  type Stage = 'download' | 'notify' | 'swap' | 'restart' | 'verify' | 'done';
  const stages: { id: Stage; icon: IconName }[] = [
    { id: 'download', icon: 'download' },
    { id: 'notify', icon: 'info' },
    { id: 'swap', icon: 'repeat' },
    { id: 'restart', icon: 'restart' },
    { id: 'verify', icon: 'shield' },
    { id: 'done', icon: 'check' },
  ];
  const STAGE_OF: Record<InstallPhase, Stage | null> = {
    marking: 'download',
    downloading: 'download',
    downloaded: 'download',
    broadcasting: 'notify',
    acked: 'notify',
    swapping: 'swap',
    swapped: 'swap',
    restarting: 'restart',
    verifying: 'verify',
    cleaning: 'verify',
    done: 'done',
    failed: null,
    aborted: null,
    unknown: null,
  };

  let channel = $state<'auto' | 'pre'>('auto');
  let info = $state.raw<UpdateInfo | null>(null);
  let checking = $state(false);
  let checkFault = $state.raw<Failure | null>(null);

  async function check(ch: 'auto' | 'pre') {
    checking = true;
    try {
      const i = await store.checkUpdate(ch);
      // 换了渠道之后才回来的旧结果不要
      if (ch === channel) {
        info = i;
        checkFault = null;
      }
    } catch (raw) {
      if (ch === channel) checkFault = toFailure(raw);
    } finally {
      checking = false;
    }
  }

  // 换渠道就重新检查一次；还没回来时保留上一次的结果，不闪空白
  $effect(() => {
    void check(channel);
  });

  // 安装进度在 store 里：离开本页再回来，正在进行的安装照样看得到
  const job = $derived(store.job);
  const running = $derived(!!job && job.end === null && job.fault === null);
  const stage = $derived<Stage | null>(job?.end ? 'done' : job?.phase ? STAGE_OF[job.phase] : running ? 'download' : null);
  const stageIdx = $derived(stage ? stages.findIndex((s) => s.id === stage) : -1);
  const targets = $derived(job?.targets ?? []);
  const restarted = $derived(job?.instances.filter((i) => i.outcome === 'restarted').length ?? 0);
  const target = $derived(job?.end?.version ?? job?.requestedVersion ?? info?.latest ?? '');

  /** 当前阶段走了多少：下载按字节（总量未知时不显示比例），逐个重启按实例；其余阶段没有可量的进度 */
  const stageProgress = $derived.by(() => {
    if (!job) return 0;
    if (stage === 'download') return job.download?.total ? job.download.bytes / job.download.total : job.phase === 'downloaded' ? 1 : 0.5;
    if (stage === 'restart') return targets.length ? job.instances.length / targets.length : 1;
    return 1;
  });

  async function install() {
    if (!info?.check) return;
    try {
      const end = await store.installUpdate(info.check);
      if (end.outcome === 'up-to-date') overlays.toast(t('up.upToDate'));
    } catch (raw) {
      const f = toFailure(raw);
      // 检查之后可装的版本变了：重新查一次，让用户看到新的再决定
      if (f.code === 'desktop_changed') void check(channel);
      overlays.toast(faultText(f), 'coral');
    }
    // 装完再问一次：当前版本这时已经是新版本了
    void check(channel);
  }

  function detail(s: Stage): string {
    if (!job) return '';
    switch (s) {
      case 'download':
        return job.download ? (job.download.total ? t('up.d.download', { a: bytes(job.download.bytes), b: bytes(job.download.total) }) : bytes(job.download.bytes)) : '';
      case 'notify':
        return t('up.d.notify', { n: targets.length });
      case 'swap':
        return t('up.d.swap');
      case 'restart':
        return t('up.d.restart', { done: restarted, total: targets.length });
      case 'verify':
        return t('up.d.verify');
      default:
        return '';
    }
  }

  function portState(port: number): 'done' | 'failed' | 'active' | 'waiting' {
    const r = job?.instances.find((i) => i.port === port);
    if (r) return r.outcome === 'restarted' ? 'done' : 'failed';
    if (stage !== 'restart') return 'waiting';
    // 还没有结果的里面，排在最前的那个正在交接
    const pending = targets.filter((p) => !job?.instances.some((i) => i.port === p));
    return pending[0] === port ? 'active' : 'waiting';
  }

  const released = $derived(info?.publishedAt ? new Intl.DateTimeFormat(ui.lang, { month: 'long', day: 'numeric' }).format(new Date(info.publishedAt)) : '');
</script>

<header class="head">
  <h1>{t('nav.update')}</h1>
  <Segmented
    options={[
      { value: 'auto', label: t('up.channel.auto') },
      { value: 'pre', label: t('up.channel.pre') },
    ]}
    bind:value={channel}
    label={t('up.channel')}
    size="sm"
  />
</header>

{#if checkFault && !info}
  <div class="banner coral"><Icon name="alert" size={16} /><p>{faultText(checkFault)}</p><button class="btn sm" onclick={() => check(channel)}>{t('up.check')}</button></div>
{:else if !info}
  <p class="muted">{t('up.checking')}</p>
{/if}

{#if info}
  <section class="versions">
    <div class="ver current">
      <span class="v-label">{t('up.current')}</span>
      <span class="v-num">{info.current}</span>
      <span class="chip lilac"><Icon name="monitor" size={12} stroke={2.4} />{t(`up.variant.${info.variant}`)}</span>
    </div>
    {#if info.updateAvailable && info.latest}
      <div class="arrow" aria-hidden="true"><Icon name="arrowRight" size={22} stroke={2.4} /></div>
      <div class="ver latest" class:done={stage === 'done'}>
        <span class="v-label">{t(info.channel === 'pre' ? 'up.latestPre' : 'up.latest')}</span>
        <span class="v-num">{info.latest}</span>
        {#if released}<span class="chip butter"><Icon name="sparkles" size={12} stroke={2.4} />{t('up.released', { date: released })}</span>{/if}
      </div>
    {:else}
      <div class="ver uptodate">
        <span class="chip mint"><Icon name="check" size={12} stroke={2.6} />{t('up.upToDate')}</span>
      </div>
    {/if}
    <div class="cta">
      {#if info.updateAvailable && info.check && stage !== 'done'}
        <button class="btn primary big" disabled={running} onclick={install}>
          <Icon name="download" size={16} />{running ? t('up.installing', { v: target }) : t('up.install', { v: info.latest ?? '' })}
        </button>
      {:else}
        <button class="btn" disabled={checking || running} onclick={() => check(channel)}><Icon name="restart" size={15} />{t('up.again')}</button>
      {/if}
      <span class="checked">{checkFault ? faultText(checkFault) : t('up.checked', { ago: ago(store.now - info.checkedAt) })}</span>
    </div>
  </section>
{/if}

{#if job && (stage || job.fault)}
  <section class="progress" in:pop={{ y: 14 }} aria-label={t('up.progress')}>
    <ol class="stepper">
      {#each stages as s, i (s.id)}
        <li class:done={i < stageIdx || stage === 'done'} class:active={i === stageIdx && stage !== 'done'}>
          <span class="bubble">
            {#if i < stageIdx || stage === 'done'}
              <Icon name="check" size={16} stroke={3} />
            {:else}
              <Icon name={s.icon} size={16} stroke={2.2} />
            {/if}
          </span>
          <span class="s-label">{t(`up.stage.${s.id}`)}</span>
          {#if i < stages.length - 1}
            <span class="rail"><span class="rail-fill" style="width: {i < stageIdx ? 100 : i === stageIdx ? stageProgress * 100 : 0}%"></span></span>
          {/if}
        </li>
      {/each}
    </ol>

    {#if job.fault}
      <div class="done-box failed" in:pop={{ y: 10, scale: 0.9 }}>
        {#if mascots.current}<MascotView mood="worried" size="hero" px={96} />{/if}
        <div>
          <h2>{t('up.failedTitle', { v: job.requestedVersion })}</h2>
          <p>{faultText(job.fault)}</p>
        </div>
      </div>
    {:else if stage === 'done'}
      <div class="done-box" in:pop={{ y: 10, scale: 0.9 }}>
        <!-- 装完了：借用「救回」的庆祝（跳一下、撒星星），只播一次 -->
        {#if mascots.current}<MascotView mood="rescued" size="hero" px={96} />{/if}
        <div>
          <h2>{job.end?.outcome === 'up-to-date' ? t('up.upToDate') : t('up.doneTitle', { v: target })}</h2>
          <p>{t('up.doneBody')}</p>
        </div>
      </div>
    {:else if stage}
      <div class="stage-box">
        <div class="stage-line">
          {#if mascots.current}<MascotView mood="working" size="small" px={40} />{/if}
          <strong>{t(`up.stage.${stage}`)}</strong>
          <span class="muted num">{detail(stage)}</span>
        </div>
        <JellyBar value={stageProgress} tone={stage === 'restart' ? 'mint' : 'lilac'} label={t(`up.stage.${stage}`)} />
      </div>
    {/if}

    {#if targets.length && (stage === 'restart' || stage === 'notify' || stage === 'verify' || job.fault)}
      <ul class="targets">
        {#each targets as port (port)}
          {@const state = portState(port)}
          {@const card = store.card(port)}
          <li class="st-{state}">
            <span class="t-icon">
              {#if state === 'done'}<Icon name="check" size={13} stroke={3} />{:else if state === 'failed'}<Icon name="x" size={13} stroke={3} />{:else if state === 'active'}<span class="spin"></span>{:else}<Icon name="clock" size={13} stroke={2.4} />{/if}
            </span>
            <span class="mono">{port}</span>
            <span class="muted">{card ? (aliasOf(card) ?? '') : ''}</span>
            <span class="t-state">
              {state === 'done' ? `v${target}` : state === 'failed' ? t('up.restartFailed') : state === 'active' ? t('up.restartingOne') : t('up.waiting')}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}

<aside class="restart-note">
  <Icon name="info" size={16} />
  <p>{t('up.restartNote')}</p>
</aside>

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 18px;
  }
  h1 {
    margin: 0;
    font-size: var(--fs-2xl);
    font-weight: 620;
  }
  .versions {
    display: grid;
    grid-template-columns: auto auto auto 1fr;
    align-items: center;
    gap: 18px;
    padding: 20px 24px;
    border-radius: var(--r-xl);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    box-shadow: var(--shadow-2);
  }
  .ver {
    display: grid;
    gap: 4px;
    justify-items: start;
  }
  .v-label {
    color: var(--text-3);
    font-size: var(--fs-sm);
    font-weight: 550;
  }
  .v-num {
    font-size: 40px;
    font-weight: 640;
    line-height: 1.05;
    font-variant-numeric: tabular-nums;
  }
  .latest .v-num {
    color: var(--lilac-deep);
  }
  .latest.done .v-num {
    color: var(--mint-deep);
  }
  .arrow {
    color: var(--text-3);
  }
  .cta {
    display: grid;
    justify-items: end;
    gap: 8px;
  }
  .btn.big {
    --btn-h: 42px;
    padding: 0 20px;
    font-size: var(--fs-lg);
  }
  .checked {
    color: var(--text-3);
    font-size: var(--fs-sm);
  }

  .progress {
    margin-top: 22px;
    padding: 22px 24px 20px;
    border-radius: var(--r-xl);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    box-shadow: var(--shadow-2);
  }
  .stepper {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    margin: 0 0 20px;
    padding: 0;
    list-style: none;
  }
  .stepper li {
    position: relative;
    display: grid;
    justify-items: center;
    gap: 7px;
  }
  .bubble {
    position: relative;
    z-index: 1;
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    border: var(--line-w) solid var(--line-soft);
    background: var(--card-2-bg);
    color: var(--text-3);
    transition:
      background-color var(--dur-mid) var(--ease-soft),
      border-color var(--dur-mid) var(--ease-soft),
      transform var(--dur-slow) var(--ease-spring);
  }
  /* 进行中与已完成的步骤是实心色块，不再描边；进行中那一步再加一圈同色柔光，表示「正在这里」 */
  .active .bubble {
    border-color: transparent;
    background: var(--lilac);
    color: var(--on-fill);
    box-shadow:
      0 2px 4px color-mix(in oklab, var(--lilac) 45%, transparent),
      0 6px 16px -4px color-mix(in oklab, var(--lilac) 95%, transparent);
    transform: scale(1.12);
    animation: bob 1.1s ease-in-out infinite;
  }
  .done .bubble {
    border-color: transparent;
    background: var(--mint);
    color: var(--on-fill);
  }
  @keyframes bob {
    0%, 100% { transform: scale(1.12) translateY(0); }
    50% { transform: scale(1.12) translateY(-3px); }
  }
  .s-label {
    color: var(--text-3);
    font-size: var(--fs-sm);
    font-weight: 550;
  }
  .active .s-label,
  .done .s-label {
    color: var(--text);
  }
  /* 阶段之间的轨道：当前阶段推进时按比例填满 */
  .rail {
    position: absolute;
    top: 18px;
    left: calc(50% + 24px);
    right: calc(-50% + 24px);
    height: 5px;
    border-radius: var(--r-pill);
    background: var(--card-2-bg);
    overflow: hidden;
  }
  .rail-fill {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--mint-deep);
    transition: width 120ms linear;
  }
  .stage-box {
    display: grid;
    gap: 12px;
    padding: 16px 18px;
    border-radius: var(--r-lg);
    background: var(--card-2-bg);
  }
  .stage-line {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .stage-line .muted {
    margin-left: auto;
  }
  .muted {
    color: var(--text-3);
  }
  .targets {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  .targets li {
    display: grid;
    grid-template-columns: 26px 60px 1fr auto;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 12px 0 6px;
    border-radius: var(--r-pill);
    background: var(--card-bg);
    border: var(--line-w-thin) solid var(--line-soft);
    font-size: var(--fs-sm);
  }
  .t-icon {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: var(--card-2-bg);
    color: var(--text-3);
  }
  .st-done .t-icon {
    background: var(--mint);
    color: var(--on-fill);
  }
  /* 正在处理的目标行：淡丁香底 + 贴身阴影把它从列表里托起来 */
  .targets li.st-active {
    border-color: transparent;
    background: var(--lilac-soft);
    box-shadow: var(--shadow-1);
  }
  .st-active .t-icon {
    background: var(--lilac);
    color: var(--on-fill);
  }
  .t-state {
    color: var(--text-3);
    font-weight: 550;
  }
  .st-done .t-state {
    color: var(--mint-deep);
  }
  .st-active .t-state {
    color: var(--lilac-deep);
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
  .done-box {
    display: flex;
    align-items: center;
    gap: 18px;
    padding: 8px 6px 0;
  }
  .done-box h2 {
    margin: 0 0 4px;
    font-size: var(--fs-xl);
    font-weight: 620;
  }
  .done-box p {
    margin: 0;
    color: var(--text-2);
  }

  /* 实例列表放在阶段框外面：失败与完成之后也要能看到每个实例的结果 */
  .progress > .targets {
    margin-top: 14px;
  }
  /* 没有可装的版本时，这一格占掉箭头与最新版本的两格，按钮仍在最右 */
  .uptodate {
    grid-column: span 2;
    align-self: center;
  }
  .done-box.failed h2 {
    color: var(--coral-deep);
  }
  .targets li.st-failed {
    color: var(--coral-deep);
  }
  .banner {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 16px;
    padding: 10px 14px;
    border-radius: var(--r-md);
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  .banner p {
    flex: 1;
    margin: 0;
  }
  .restart-note {
    display: flex;
    margin-top: 22px;
    max-width: 640px;
    gap: 10px;
    padding: 14px 16px;
    border-radius: var(--r-md);
    background: var(--sky-soft);
    color: var(--sky-deep);
  }
  .restart-note p {
    margin: 0;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .restart-note :global(svg) {
    flex: none;
    margin-top: 2px;
  }
</style>

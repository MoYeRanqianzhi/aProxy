<!--
  更新：当前版本（含变体：带界面 / 无界面）、最新版本、更新说明，以及安装进度的六个阶段：
  下载 → 校验 → 通知实例 → 替换 → 逐个重启 → 完成。
  版本信息与安装都经 Backend：检查更新是一次请求，安装进度由后端一拍拍推过来（update-progress 事件）。
  原型的 mock 后端会把整个过程演示一遍（假的计时），用来审核阶段动画。
-->
<script lang="ts">
  import { t, ago } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { pop } from '../lib/motion';
  import type { Instance, UpdateInfo, UpdateStage } from '../lib/types';
  import Icon, { type IconName } from '../components/Icon.svelte';
  import MascotView from '../components/MascotView.svelte';
  import { mascots } from '../lib/mascot/registry.svelte';
  import JellyBar from '../components/JellyBar.svelte';
  import Segmented from '../components/Segmented.svelte';

  const stages: { id: UpdateStage; icon: IconName }[] = [
    { id: 'download', icon: 'download' },
    { id: 'verify', icon: 'shield' },
    { id: 'notify', icon: 'info' },
    { id: 'swap', icon: 'repeat' },
    { id: 'restart', icon: 'restart' },
    { id: 'done', icon: 'check' },
  ];

  let channel = $state<UpdateInfo['channel']>('stable');
  let info = $state<UpdateInfo | null>(null);

  // 换通道就重新检查一次；还没回来时保留上一次的结果，不闪空白
  $effect(() => {
    const ch = channel;
    let alive = true;
    store.checkUpdate(ch).then((i) => {
      if (alive) info = i;
    });
    return () => {
      alive = false;
    };
  });

  // 安装进度在 store 里：离开本页再回来，正在进行的安装照样看得到
  const progress = $derived(store.updateProgress);
  const stage = $derived(progress?.stage ?? null);
  const running = $derived(!!progress && progress.stage !== 'done');
  const stageIdx = $derived(stage ? stages.findIndex((s) => s.id === stage) : -1);
  const stageProgress = $derived(progress?.progress ?? 0);
  const targets = $derived(
    (progress?.targets ?? []).map((port) => store.instance(port) ?? ({ port, alias: null } as Pick<Instance, 'port' | 'alias'>)),
  );
  const restarted = $derived(progress?.restarted.length ?? 0);
  const target = $derived(progress?.version ?? info?.latest ?? '');

  async function install() {
    if (!info) return;
    await store.installUpdate(info.latest);
    // 装完再问一次：当前版本这时已经是新版本了
    info = await store.checkUpdate(channel);
  }

  function detail(s: UpdateStage): string {
    switch (s) {
      case 'download':
        return t('up.d.download', { a: `${(progress?.downloadedMb ?? 0).toFixed(1)} MB`, b: `${progress?.totalMb ?? 0} MB` });
      case 'verify':
        return t('up.d.verify');
      case 'notify':
        return t('up.d.notify', { n: targets.length });
      case 'swap':
        return t('up.d.swap');
      case 'restart':
        return t('up.d.restart', { done: restarted, total: targets.length });
      default:
        return '';
    }
  }

  const released = $derived(
    info ? new Intl.DateTimeFormat(ui.lang, { month: 'long', day: 'numeric' }).format(new Date(info.publishedAt)) : '',
  );
  const NOTE_ICON: Record<UpdateInfo['notes'][number]['kind'], IconName> = { feature: 'sparkles', fix: 'check' };
</script>

<header class="head">
  <h1>{t('nav.update')}</h1>
  <Segmented
    options={[
      { value: 'stable', label: t('up.channel.stable') },
      { value: 'pre', label: t('up.channel.pre') },
    ]}
    bind:value={channel}
    label={t('up.channel')}
    size="sm"
  />
</header>

{#if info}
  <section class="versions">
    <div class="ver current">
      <span class="v-label">{t('up.current')}</span>
      <span class="v-num">{info.current}</span>
      <span class="chip lilac"><Icon name="monitor" size={12} stroke={2.4} />{t(`up.variant.${info.variant}`)}</span>
    </div>
    <div class="arrow" aria-hidden="true"><Icon name="arrowRight" size={22} stroke={2.4} /></div>
    <div class="ver latest" class:done={stage === 'done'}>
      <span class="v-label">{t('up.latest')}</span>
      <span class="v-num">{info.latest}</span>
      <span class="chip butter"><Icon name="sparkles" size={12} stroke={2.4} />{t('up.released', { date: released })}</span>
    </div>
    <div class="cta">
      {#if stage === 'done'}
        <button class="btn" onclick={install}><Icon name="restart" size={15} />{t('up.again')}</button>
      {:else}
        <button class="btn primary big" disabled={running} onclick={install}>
          <Icon name="download" size={16} />{running ? t('up.installing', { v: target }) : t('up.install', { v: info.latest })}
        </button>
      {/if}
      <span class="checked">{t('up.checked', { ago: ago(store.now - info.checkedAt) })}</span>
    </div>
  </section>
{/if}

{#if stage}
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

    {#if stage === 'done'}
      <div class="done-box" in:pop={{ y: 10, scale: 0.9 }}>
        <!-- 装完了：借用「救回」的庆祝（跳一下、撒星星），只播一次 -->
        {#if mascots.current}<MascotView mood="rescued" size="hero" px={96} />{/if}
        <div>
          <h2>{t('up.doneTitle', { v: target })}</h2>
          <p>{t('up.doneBody')}</p>
        </div>
      </div>
    {:else}
      <div class="stage-box">
        <div class="stage-line">
          {#if mascots.current}<MascotView mood="working" size="small" px={40} />{/if}
          <strong>{t(`up.stage.${stage}`)}</strong>
          <span class="muted num">{detail(stage)}</span>
        </div>
        <JellyBar value={stageProgress} tone={stage === 'restart' ? 'mint' : 'lilac'} label={t(`up.stage.${stage}`)} />
        {#if stage === 'restart' || stage === 'notify'}
          <ul class="targets">
            {#each targets as inst, i (inst.port)}
              {@const state = stage === 'notify' ? 'waiting' : i < restarted ? 'done' : i === restarted ? 'active' : 'waiting'}
              <li class="st-{state}">
                <span class="t-icon">
                  {#if state === 'done'}<Icon name="check" size={13} stroke={3} />{:else if state === 'active'}<span class="spin"></span>{:else}<Icon name="clock" size={13} stroke={2.4} />{/if}
                </span>
                <span class="mono">{inst.port}</span>
                <span class="muted">{inst.alias ?? ''}</span>
                <span class="t-state">
                  {state === 'done' ? `v${target}` : state === 'active' ? t('up.restartingOne') : t('up.waiting')}
                </span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  </section>
{/if}

{#if info}
  <div class="lower">
    <section class="panel notes">
      <h2 class="section-title">{t('up.notes')}<span class="count">{info.latest}</span></h2>
      <ul>
        {#each info.notes as note, i (i)}
          <li><Icon name={NOTE_ICON[note.kind]} size={15} />{note.text[ui.lang] ?? note.text.en}</li>
        {/each}
      </ul>
    </section>
    <aside class="restart-note">
      <Icon name="info" size={16} />
      <p>{t('up.restartNote')}</p>
    </aside>
  </div>
{/if}

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

  .lower {
    display: grid;
    grid-template-columns: 1.4fr 1fr;
    gap: 16px;
    margin-top: 22px;
    align-items: start;
  }
  .notes {
    padding: 16px 18px 12px;
  }
  .notes ul {
    list-style: none;
    margin: 10px 0 0;
    padding: 0;
  }
  .notes li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 0;
    border-top: var(--line-w-thin) dashed var(--line-soft);
  }
  .notes li :global(svg) {
    color: var(--lilac-deep);
    flex: none;
  }
  .restart-note {
    display: flex;
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

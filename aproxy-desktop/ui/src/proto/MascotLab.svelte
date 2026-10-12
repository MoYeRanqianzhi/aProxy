<!--
  【仅原型】吉祥物预览页（#/mascots）：所有吉祥物 × 所有状态的总表；选中一个后看全部状态 × 三种尺寸、
  动画分解、软件图标候选与托盘状态版。
  不在导航里，从审核工具条的「跳转」或地址栏进入。

  和界面其它地方一样，只通过注册表与 MascotView 使用吉祥物；图标部分直接调用定义的 icon()，
  拼上底板后转成图片——scripts/icons.mjs 生成软件图标文件用的是同一个拼法（lib/mascot/app-icon.ts），
  所以这里看到的就是实际像素。
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { ui, type Palette } from '../lib/state.svelte';
  import { t, type MessageKey } from '../lib/i18n';
  import { mascots, mascotName } from '../lib/mascot/registry.svelte';
  import { mascotPalette } from '../lib/mascot/palette.svelte';
  import { MASCOT_MOODS, type MascotDefinition, type MascotMood } from '../lib/mascot/types';
  // 软件图标的拼法与 scripts/icons.mjs 生成图标文件时是同一个函数，这里看到的就是图标文件的样子
  import { appIcon, PLATES } from '../lib/mascot/app-icon';
  import MascotView from '../components/MascotView.svelte';
  import Segmented from '../components/Segmented.svelte';

  let pick = $state(mascots.current?.id ?? 'tumbler');
  const entry = $derived(mascots.get(pick));
  // 只有内置吉祥物的定义在界面这一侧；自定义的代码只在沙箱里跑，图标合成那几节就没法替它做
  const def = $derived(entry?.def);

  // 模拟「又重试了一次」与「又救回了一个」：定时让重试次数加一、让庆祝重播，审核时不用一直点
  let retries = $state(3);
  let replay = $state(0);
  onMount(() => {
    const id = setInterval(() => {
      retries = retries >= 12 ? 1 : retries + 1;
      replay += 1;
    }, 3400);
    return () => clearInterval(id);
  });

  const theme = $derived(ui.variant);
  const motion = $derived<'full' | 'reduced'>(ui.reducedMotion ? 'reduced' : 'full');

  // 每种状态的分帧时刻（毫秒），挑的是动作最有辨识度的那几下
  const FRAMES: { mood: MascotMood; at: number[] }[] = [
    { mood: 'retrying', at: [0, 90, 200, 330, 480, 680, 950, 1500] },
    { mood: 'rescued', at: [0, 180, 400, 620, 860, 1000, 1250, 1650] },
    { mood: 'working', at: [0, 120, 330, 500, 700, 840, 1000, 1150] },
    { mood: 'sleeping', at: [0, 500, 1000, 1500, 2000, 2500, 3000, 3500] },
  ];

  /* ---------- 图标 ---------- */
  const APP_SIZES = [16, 32, 48, 256];
  const TRAY_SIZES = [16, 24, 32];
  // 仿 Windows 11 任务栏的两种底色：托盘图标的状态点外圈要和它一致
  const TASKBAR = { light: '#f3f3f3', dark: '#202020' } as const;
  const BARS = ['light', 'dark'] as const;

  function trayIcon(d: MascotDefinition, mood: MascotMood, px: number, bar: 'light' | 'dark'): string {
    return d.icon({ px, mood, theme: bar, palette: { ...mascotPalette.current, surface: TASKBAR[bar] } });
  }

  const dataUrl = (svg: string) => 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg);

  /** 16px 逐像素放大：先在 16×16 画布上真实栅格化，再用最近邻放大 8 倍并画上像素格 */
  function zoom(canvas: HTMLCanvasElement, svg: string) {
    const img = new Image();
    img.onload = () => {
      const small = document.createElement('canvas');
      small.width = small.height = 16;
      small.getContext('2d')!.drawImage(img, 0, 0, 16, 16);
      const ctx = canvas.getContext('2d')!;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.imageSmoothingEnabled = false;
      ctx.drawImage(small, 0, 0, canvas.width, canvas.height);
      ctx.strokeStyle = 'rgba(127,127,127,.22)';
      ctx.lineWidth = 1;
      const step = canvas.width / 16;
      for (let i = 1; i < 16; i++) {
        ctx.beginPath();
        ctx.moveTo(i * step + 0.5, 0);
        ctx.lineTo(i * step + 0.5, canvas.height);
        ctx.moveTo(0, i * step + 0.5);
        ctx.lineTo(canvas.width, i * step + 0.5);
        ctx.stroke();
      }
    };
    img.src = dataUrl(svg);
  }

  /** Svelte action：svg 参数变化（换形象、换主题）时重画 */
  function pixels(canvas: HTMLCanvasElement, svg: string) {
    zoom(canvas, svg);
    return { update: (next: string) => zoom(canvas, next) };
  }

  const moodKey = (m: MascotMood) => `lab.mood.${m}` as MessageKey;
  const mascotOptions = $derived(mascots.entries.map((e) => ({ value: e.id, label: mascotName(e) })));
</script>

<header class="head">
  <h1>{t('lab.title')}</h1>
  <p>{t('lab.lead')}</p>
  <div class="controls">
    <label><span>{t('lab.mascot')}</span><Segmented size="sm" options={mascotOptions} bind:value={pick} label={t('lab.mascot')} /></label>
    <label>
      <span>{t('lab.theme')}</span>
      <Segmented
        size="sm"
        options={[
          { value: 'light', label: t('set.light') },
          { value: 'dark', label: t('set.dark') },
        ]}
        value={theme}
        onchange={(v) => (ui.modePref = v)}
        label={t('lab.theme')}
      />
    </label>
    <label>
      <span>{t('lab.palette')}</span>
      <Segmented
        size="sm"
        options={[
          { value: 'soft' as Palette, label: t('proto.palette.soft') },
          { value: 'standard' as Palette, label: t('proto.palette.standard') },
          { value: 'vivid' as Palette, label: t('proto.palette.vivid') },
        ]}
        bind:value={ui.palette}
        label={t('lab.palette')}
      />
    </label>
    <label>
      <span>{t('lab.motion')}</span>
      <Segmented
        size="sm"
        options={[
          { value: 'full', label: t('lab.motion.full') },
          { value: 'reduced', label: t('lab.motion.reduced') },
        ]}
        value={motion}
        onchange={(v) => (ui.reducedMotionOverride = v === 'reduced')}
        label={t('lab.motion')}
      />
    </label>
    <button class="btn sm" onclick={() => (retries += 1)}>{t('lab.bump')}</button>
    <button class="btn sm" onclick={() => (replay += 1)}>{t('lab.replay')}</button>
  </div>
</header>

<section class="panel block">
  <h2 class="section-title">{t('lab.matrix')}</h2>
  <p class="desc">{t('lab.matrix.desc')}</p>
  <div class="matrix">
    <span></span>
    {#each MASCOT_MOODS as mood (mood)}
      <span class="col-h">{t(moodKey(mood))}</span>
    {/each}
    {#each mascots.entries as e (e.id)}
      <div class="m-name">
        <strong>{mascotName(e)}</strong>
        {#if e.source === 'custom'}<span class="chip pink tiny">{t('set.mascot.custom')}</span>{/if}
      </div>
      {#each MASCOT_MOODS as mood (mood)}
        <div class="m-cell">
          <MascotView id={e.id} {mood} {retries} size="small" px={64} replay={mood === 'rescued' ? replay : 0} />
        </div>
      {/each}
    {/each}
  </div>
</section>

{#if entry}
  <section class="panel block">
    <h2 class="section-title">{t('lab.states')}</h2>
    <div class="grid">
      <span></span>
      <span class="col-h">{t('lab.size.hero')} · 128</span>
      <span class="col-h">{t('lab.size.small')} · 48</span>
      <span class="col-h">{t('lab.size.icon')} · 32 / 22</span>
      {#each MASCOT_MOODS as mood (mood)}
        <div class="mood-cell">
          <strong>{t(moodKey(mood))}</strong>
          <span>{t(`${moodKey(mood)}.desc` as MessageKey)}</span>
        </div>
        <div class="stage hero-stage">
          <MascotView id={entry.id} {mood} {retries} size="hero" px={128} replay={mood === 'rescued' ? replay : 0} />
        </div>
        <div class="stage">
          <MascotView id={entry.id} {mood} {retries} size="small" px={48} replay={mood === 'rescued' ? replay : 0} />
        </div>
        <div class="stage icons">
          <MascotView id={entry.id} {mood} size="icon" px={32} />
          <MascotView id={entry.id} {mood} size="icon" px={22} />
        </div>
      {/each}
    </div>
  </section>

  <section class="panel block">
    <h2 class="section-title">{t('lab.film')}</h2>
    <p class="desc">{t('lab.film.desc')}</p>
    {#each FRAMES as f (f.mood)}
      <div class="film">
        <span class="film-label">{t(moodKey(f.mood))}</span>
        {#each f.at as ms (ms)}
          <div class="frame">
            <MascotView id={entry.id} mood={f.mood} retries={5} size="hero" px={88} freezeAt={ms} />
            <span class="ms">{ms} ms</span>
          </div>
        {/each}
      </div>
    {/each}
  </section>

{/if}

{#if def}
  <section class="panel block">
    <h2 class="section-title">{t('lab.appIcon')}</h2>
    <p class="desc">{t('lab.appIcon.desc')}</p>
    {#each PLATES as plate (plate)}
      <div class="icon-row">
        <span class="row-label">{t(`lab.plate.${plate}` as MessageKey)}</span>
        {#each BARS as bar (bar)}
          <div class="taskbar {bar}" title={t(`lab.taskbar.${bar}` as MessageKey)}>
            {#each APP_SIZES as px (px)}
              <figure>
                <img src={dataUrl(appIcon(def, plate, px, mascotPalette.current))} width={px} height={px} alt="" />
                <figcaption>{px}</figcaption>
              </figure>
            {/each}
          </div>
        {/each}
        <figure class="zoom">
          <canvas width="128" height="128" use:pixels={appIcon(def, plate, 16, mascotPalette.current)}></canvas>
          <figcaption>{t('lab.zoom')}</figcaption>
        </figure>
      </div>
    {/each}
  </section>

  <section class="panel block">
    <h2 class="section-title">{t('lab.tray')}</h2>
    <p class="desc">{t('lab.tray.desc')}</p>
    {#each BARS as bar (bar)}
      <div class="tray-row">
        <span class="row-label">{t(`lab.taskbar.${bar}` as MessageKey)}</span>
        <div class="taskbar {bar} wide">
          {#each MASCOT_MOODS as mood (mood)}
            <figure class="tray">
              <div class="tray-icons">
                {#each TRAY_SIZES as px (px)}
                  <img src={dataUrl(trayIcon(def, mood, px, bar))} width={px} height={px} alt="" />
                {/each}
              </div>
              <figcaption>{t(moodKey(mood))}</figcaption>
            </figure>
          {/each}
        </div>
      </div>
    {/each}
    <div class="tray-zoom">
      {#each MASCOT_MOODS as mood (mood)}
        <figure class="zoom">
          <canvas width="96" height="96" use:pixels={trayIcon(def, mood, 16, 'dark')}></canvas>
          <figcaption>{t(moodKey(mood))}</figcaption>
        </figure>
      {/each}
    </div>
  </section>
{/if}

<style>
  .head h1 {
    margin: 0 0 6px;
    font-size: var(--fs-2xl);
    font-weight: 620;
  }
  .head p {
    margin: 0 0 16px;
    max-width: 70ch;
    color: var(--text-2);
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px 18px;
    margin-bottom: 20px;
  }
  .controls label {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .block {
    padding: 16px 20px 18px;
    margin-bottom: 18px;
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-1);
  }
  .desc {
    margin: 4px 0 12px;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }

  /* 总表：第一列是名字，后面每列一个状态 */
  .matrix {
    display: grid;
    grid-template-columns: minmax(120px, 160px) repeat(6, 1fr);
    align-items: center;
    row-gap: 6px;
    margin-top: 10px;
  }
  .m-name {
    display: grid;
    justify-items: start;
    gap: 4px;
    padding: 8px 0;
  }
  .m-cell {
    display: grid;
    place-items: center;
    padding: 6px 0;
    border-top: var(--line-w-thin) dashed var(--line-soft);
  }
  .matrix .chip.tiny {
    height: 19px;
    padding: 0 7px;
    font-size: var(--fs-xs);
  }

  .grid {
    display: grid;
    grid-template-columns: minmax(200px, 1fr) 170px 110px 110px;
    align-items: center;
    margin-top: 10px;
  }
  .col-h {
    padding: 0 0 8px;
    color: var(--text-3);
    font-size: var(--fs-xs);
    text-align: center;
  }
  .mood-cell,
  .stage {
    min-height: 150px;
    border-top: var(--line-w-thin) solid var(--line-soft);
  }
  .mood-cell {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 4px;
    padding-right: 18px;
  }
  .mood-cell span {
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .stage {
    display: grid;
    place-items: center;
  }
  .icons {
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 14px;
  }

  .film {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 0;
    border-top: var(--line-w-thin) solid var(--line-soft);
  }
  .film-label {
    width: 72px;
    flex: none;
    font-weight: 600;
    font-size: var(--fs-sm);
  }
  .frame {
    display: grid;
    justify-items: center;
    gap: 2px;
  }
  .ms {
    color: var(--text-3);
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
  }

  .icon-row,
  .tray-row {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 10px 0;
    border-top: var(--line-w-thin) solid var(--line-soft);
  }
  .row-label {
    width: 96px;
    flex: none;
    font-weight: 600;
    font-size: var(--fs-sm);
  }
  /* 任务栏条：刻意用固定的系统色，而不是应用令牌——它模拟的是应用之外的环境 */
  .taskbar {
    display: flex;
    align-items: flex-end;
    gap: 16px;
    padding: 12px 16px 8px;
    border-radius: var(--r-md);
  }
  .taskbar.light {
    background: #f3f3f3;
    color: #5b5b5b;
  }
  .taskbar.dark {
    background: #202020;
    color: #b8b8b8;
  }
  .taskbar.wide {
    flex: 1;
    justify-content: space-around;
  }
  figure {
    margin: 0;
    display: grid;
    justify-items: center;
    gap: 4px;
  }
  figcaption {
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
  }
  /* 256px 的那张缩到 96 显示，否则一行放不下；它的细节在别处的大图里看 */
  .taskbar img[width='256'] {
    width: 96px;
    height: 96px;
  }
  img,
  canvas {
    display: block;
  }
  .zoom canvas {
    border-radius: var(--r-xs);
    background: repeating-conic-gradient(#e7e7ee 0 25%, #ffffff 0 50%) 0 0 / 16px 16px;
    image-rendering: pixelated;
  }
  .zoom figcaption {
    color: var(--text-3);
  }
  .tray-icons {
    display: flex;
    align-items: flex-end;
    gap: 10px;
  }
  .tray-zoom {
    display: flex;
    flex-wrap: wrap;
    gap: 14px;
    padding-top: 12px;
    border-top: var(--line-w-thin) solid var(--line-soft);
  }
  .tray-zoom canvas {
    background: #202020;
  }
</style>

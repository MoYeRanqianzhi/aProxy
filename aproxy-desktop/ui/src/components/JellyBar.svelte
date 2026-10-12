<!--
  果冻进度条：填充里有缓缓移动的斜纹；走满的那一刻整条「啵」地压扁再弹回。
  用在重试倒计时与安装进度上。value 为 0–1。
-->
<script lang="ts">
  let {
    value,
    tone = 'sky',
    label,
    striped = true,
  }: { value: number; tone?: 'sky' | 'mint' | 'lilac' | 'butter'; label: string; striped?: boolean } = $props();

  const pct = $derived(Math.max(0, Math.min(1, value)) * 100);
</script>

<div
  class="jelly {tone}"
  class:full={pct >= 100}
  role="progressbar"
  aria-label={label}
  aria-valuemin={0}
  aria-valuemax={100}
  aria-valuenow={Math.round(pct)}
>
  <div class="fill" class:striped style="width: {pct}%"></div>
</div>

<style>
  .jelly {
    --fill: var(--sky);
    position: relative;
    height: 14px;
    border-radius: var(--r-pill);
    border: var(--line-w) solid var(--line);
    background: var(--card-2-bg);
    /* 轻微内凹：让空槽读作「凹下去的轨道」，填充的果冻才像是被灌进去的 */
    box-shadow: inset 0 1px 2px oklch(0.3 0.05 290 / 0.1);
    overflow: hidden;
    transform-origin: left center;
  }
  .jelly.mint {
    --fill: var(--mint);
  }
  .jelly.lilac {
    --fill: var(--lilac);
  }
  .jelly.butter {
    --fill: var(--butter);
  }
  .fill {
    height: 100%;
    min-width: 10px;
    border-radius: var(--r-pill);
    background-color: var(--fill);
    /* 果冻前沿一道同色深边：灌到哪儿一眼可见，深浅主题与各配色都跟着填充色变 */
    box-shadow: inset -2px 0 0 color-mix(in oklab, var(--fill) 55%, var(--text));
    transition: width 260ms linear;
  }
  .fill.striped {
    background-image: repeating-linear-gradient(
      -45deg,
      transparent 0 6px,
      color-mix(in oklab, #fff 38%, transparent) 6px 11px
    );
    background-size: 31px 100%;
    animation: stripes 1.2s linear infinite;
  }
  .full {
    animation: jelly 620ms var(--ease-spring);
  }
  @keyframes stripes {
    to { background-position: 31px 0; }
  }
  @keyframes jelly {
    0% { transform: scale(1, 1); }
    25% { transform: scale(1.02, 0.7); }
    50% { transform: scale(0.99, 1.18); }
    75% { transform: scale(1.005, 0.94); }
    100% { transform: scale(1, 1); }
  }
</style>

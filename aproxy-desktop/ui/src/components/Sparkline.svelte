<!--
  迷你走势图：近一小时每 2 分钟一桶。请求是平滑面积线，重试是贴底的小柱（天蓝 = 重试的语义色）。
  两者同一个单位（次数）、同一根纵轴；不画坐标轴，悬停时显示该桶的数值。
  图下方卡片里的「请求 N / 重试 N」带同色小标记，充当图例，所以身份不只靠颜色。
-->
<script lang="ts">
  import { t } from '../lib/i18n';

  let {
    req,
    retry,
    height = 46,
    label,
  }: { req: number[]; retry: number[]; height?: number; label: string } = $props();

  const W = 240;
  const PAD_TOP = 5;

  let hover = $state<number | null>(null);

  const max = $derived(Math.max(4, ...req));
  const step = $derived(W / (req.length - 1));
  const y = (v: number) => height - 2 - (v / max) * (height - PAD_TOP - 2);

  /** Catmull-Rom 转三次贝塞尔：曲线经过每个点，又不会出现尖角 */
  const linePath = $derived.by(() => {
    const pts = req.map((v, i) => [i * step, y(v)] as const);
    let d = `M${pts[0][0]},${pts[0][1]}`;
    for (let i = 0; i < pts.length - 1; i++) {
      const p0 = pts[i - 1] ?? pts[i];
      const p1 = pts[i];
      const p2 = pts[i + 1];
      const p3 = pts[i + 2] ?? p2;
      const c1x = p1[0] + (p2[0] - p0[0]) / 6;
      const c1y = Math.min(height - 2, p1[1] + (p2[1] - p0[1]) / 6);
      const c2x = p2[0] - (p3[0] - p1[0]) / 6;
      const c2y = Math.min(height - 2, p2[1] - (p3[1] - p1[1]) / 6);
      d += ` C${c1x},${c1y} ${c2x},${c2y} ${p2[0]},${p2[1]}`;
    }
    return d;
  });
  const areaPath = $derived(`${linePath} L${W},${height} L0,${height} Z`);

  const barW = $derived(Math.max(2, step * 0.6));

  function onMove(ev: PointerEvent) {
    const rect = (ev.currentTarget as SVGElement).getBoundingClientRect();
    const x = ((ev.clientX - rect.left) / rect.width) * W;
    hover = Math.max(0, Math.min(req.length - 1, Math.round(x / step)));
  }

  function bucketLabel(i: number) {
    const minsAgo = (req.length - 1 - i) * 2;
    return minsAgo === 0 ? t('time.justNow') : t('time.minAgo', { n: minsAgo });
  }
</script>

<div class="spark" style="--h: {height}px">
  <svg
    viewBox="0 0 {W} {height}"
    preserveAspectRatio="none"
    role="img"
    aria-label={label}
    onpointermove={onMove}
    onpointerleave={() => (hover = null)}
  >
    <path class="area" d={areaPath} />
    {#each retry as v, i (i)}
      {#if v > 0}
        {@const h = Math.max(2.5, (v / max) * (height - PAD_TOP - 2))}
        <rect class="bar" x={i * step - barW / 2} y={height - h} width={barW} height={h + 2} rx="2" />
      {/if}
    {/each}
    <path class="line" d={linePath} />
    {#if hover !== null}
      <line class="guide" x1={hover * step} x2={hover * step} y1="0" y2={height} />
    {/if}
  </svg>
  {#if hover !== null}
    <!-- 圆点用 HTML 画：SVG 用了 preserveAspectRatio="none"，圆会被拉成椭圆 -->
    <span class="knob" style="left: {(hover / (req.length - 1)) * 100}%; top: {y(req[hover])}px"></span>
    <div class="tip" style="left: {(hover / (req.length - 1)) * 100}%">
      <span class="when">{bucketLabel(hover)}</span>
      <span><i class="k req"></i>{req[hover]}</span>
      <span><i class="k retry"></i>{retry[hover]}</span>
    </div>
  {/if}
</div>

<style>
  .spark {
    position: relative;
    height: var(--h);
  }
  svg {
    display: block;
    width: 100%;
    height: 100%;
    overflow: visible;
    cursor: crosshair;
  }
  .area {
    fill: var(--lilac-soft);
  }
  .line {
    fill: none;
    stroke: var(--chart-req);
    stroke-width: 2px;
    vector-effect: non-scaling-stroke;
    stroke-linejoin: round;
  }
  .bar {
    fill: var(--chart-retry);
    stroke: var(--card-bg);
    stroke-width: 1px;
    vector-effect: non-scaling-stroke;
  }
  .guide {
    stroke: var(--text-3);
    stroke-width: 1px;
    stroke-dasharray: 2 3;
    vector-effect: non-scaling-stroke;
  }
  .knob {
    position: absolute;
    width: 9px;
    height: 9px;
    margin: -4.5px 0 0 -4.5px;
    border-radius: 50%;
    background: var(--card);
    border: 2px solid var(--chart-req);
    pointer-events: none;
  }
  .tip {
    position: absolute;
    bottom: calc(100% + 8px);
    transform: translateX(-50%);
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 4px 9px;
    border-radius: var(--r-pill);
    border: var(--line-w-thin) solid var(--line);
    background: var(--overlay-bg);
    color: var(--text);
    font-size: var(--fs-xs);
    font-weight: 500;
    white-space: nowrap;
    box-shadow: var(--shadow-2);
    pointer-events: none;
    z-index: 2;
  }
  .when {
    color: var(--text-2);
  }
  .k {
    display: inline-block;
    width: 8px;
    height: 8px;
    margin-right: 4px;
    border-radius: 3px;
    vertical-align: 0;
  }
  .k.req {
    background: var(--chart-req);
  }
  .k.retry {
    background: var(--chart-retry);
  }
</style>

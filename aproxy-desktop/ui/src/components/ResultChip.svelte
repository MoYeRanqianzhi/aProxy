<!--
  一条请求的「结果」胶囊，日志行、总览、详情里共用：
    一次成功   200（薄荷）
    救回       529 › 200 与重试次数（天蓝：下过雨，伞撑住了）
    重试中     伞 + 第 N 次（天蓝实心，次数翻牌）
    接收中     三个跳动的小点（丁香）
    失败       状态码或原因（珊瑚）
  高度固定 24px，不会撑高日志行。
-->
<script lang="ts">
  import type { LogEntry } from '../lib/types';
  import { t } from '../lib/i18n';
  import Icon from './Icon.svelte';
  import FlipNumber from './FlipNumber.svelte';

  let { entry }: { entry: LogEntry } = $props();

  const first = $derived(entry.attempts[0]);
  const last = $derived(entry.attempts[entry.attempts.length - 1]);
  const retries = $derived(Math.max(0, entry.attempts.length - 1));
  const firstCode = $derived(first?.status ?? 'ERR');
</script>

{#if entry.outcome === 'ok'}
  <span class="res ok"><span class="code">{last?.status ?? 200}</span></span>
{:else if entry.outcome === 'rescued'}
  <span class="res rescued" title={t('logs.retriesTitle', { n: retries })}>
    <span class="code from">{firstCode}</span>
    <Icon name="chevronRight" size={11} stroke={3} />
    <span class="code">{last?.status}</span>
    <span class="times">{t('logs.retries', { n: retries })}</span>
  </span>
{:else if entry.outcome === 'retrying'}
  <span class="res retrying" title={t('logs.retriesTitle', { n: entry.attempts.length })}>
    <span class="umb"><Icon name="umbrella" size={13} stroke={2.4} /></span>
    {t('logs.status.retrying')}
    <span class="times">#<FlipNumber value={entry.attempts.length + 1} tiles={false} /></span>
  </span>
{:else if entry.outcome === 'streaming'}
  <span class="res streaming">
    <span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>{t('logs.status.streaming')}
  </span>
{:else}
  <span class="res failed" title={last?.reason ? t(`reason.${last.reason}`) : ''}>
    <Icon name="x" size={11} stroke={3} />
    {last?.status ?? t('logs.status.failed')}
  </span>
{/if}

<style>
  .res {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 24px;
    padding: 0 9px;
    border-radius: var(--r-pill);
    font-size: var(--fs-sm);
    font-weight: 600;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
    line-height: 1;
  }
  .ok {
    background: var(--mint-soft);
    color: var(--mint-deep);
  }
  .rescued {
    background: var(--sky-soft);
    color: var(--sky-deep);
    padding-right: 3px;
  }
  .from {
    opacity: 0.75;
    text-decoration: line-through;
    text-decoration-thickness: 1.5px;
  }
  .times {
    display: inline-flex;
    align-items: center;
    height: 18px;
    margin-left: 3px;
    padding: 0 6px;
    border-radius: var(--r-pill);
    background: var(--sky);
    color: var(--on-fill);
    font-size: 11px;
  }
  /* 重试中是整张表里最该被看见的状态：饱和实心色块 + 同色相柔光，不靠描边 */
  .retrying {
    background: var(--sky);
    color: var(--on-fill);
    box-shadow: 0 2px 8px -2px color-mix(in oklab, var(--sky) 90%, transparent);
    padding-right: 3px;
  }
  .retrying .times {
    background: var(--card);
    color: var(--text);
  }
  .umb {
    display: inline-flex;
    animation: sway 1.6s ease-in-out infinite;
    transform-origin: 50% 90%;
  }
  @keyframes sway {
    0%, 100% { transform: rotate(-10deg); }
    50% { transform: rotate(10deg); }
  }
  .streaming {
    background: var(--lilac-soft);
    color: var(--lilac-deep);
  }
  .dots {
    display: inline-flex;
    gap: 2.5px;
    margin-right: 3px;
  }
  .dots i {
    width: 4.5px;
    height: 4.5px;
    border-radius: 50%;
    background: currentColor;
    animation: dot 1s ease-in-out infinite;
  }
  .dots i:nth-child(2) {
    animation-delay: 0.15s;
  }
  .dots i:nth-child(3) {
    animation-delay: 0.3s;
  }
  @keyframes dot {
    0%, 60%, 100% { transform: translateY(0); opacity: 0.5; }
    30% { transform: translateY(-3px); opacity: 1; }
  }
  .failed {
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
</style>

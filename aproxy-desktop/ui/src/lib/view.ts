/*
 * 契约数据在界面上怎么概括：一张卡片显示成什么状态、什么颜色的点，一条请求算不算「救回」、耗时多久。
 *
 * 契约（backend/contract.ts）只说事实，这些概括是界面自己的说法，放在这里让每一页说法一致。
 * 全是纯函数，不读 store。
 */
import type { Buckets, Card, Row } from './backend/contract';

/**
 * 卡片显示的状态。restore：实例不在了，但留着「应当在运行」的记录，等用户恢复或放弃；
 * down：进程不在了；incompatible、unknown：说不清它死没死（版本不认识、信息读不全），不能强制停止。
 */
export type Status =
  | 'active'
  | 'idle'
  | 'starting'
  | 'restarting'
  | 'stopping'
  | 'unresponsive'
  | 'restore'
  | 'down'
  | 'incompatible'
  | 'unknown';

export function statusOf(c: Card): Status {
  if (c.restorePending) return 'restore';
  if (c.liveness !== 'live') return c.liveness;
  switch (c.state) {
    case 'starting':
      return 'starting';
    // 换版本的交接做好了准备：马上就要由新版本接手
    case 'swap-prepared':
      return 'restarting';
    case 'stopping':
      return 'stopping';
  }
  return (c.activity?.inFlight ?? 0) > 0 ? 'active' : 'idle';
}

/** 状态点的颜色类；breathe 是缓慢的呼吸动画，表示「正在干活」 */
export function dotOf(c: Card): string {
  const s = statusOf(c);
  switch (s) {
    case 'unresponsive':
      return 'coral';
    case 'starting':
    case 'restarting':
    case 'stopping':
    case 'restore':
      return 'butter';
    case 'down':
    case 'incompatible':
    case 'unknown':
      return 'gray';
  }
  if ((c.activity?.retrying ?? 0) > 0) return 'sky breathe';
  return s === 'active' ? 'mint breathe' : 'mint';
}

/** 卡片上显示的名字：第一个别名。一个配置可以有好几个别名，其余的放在提示里 */
export function aliasOf(c: Card): string | null {
  return c.metadata?.aliases[0] ?? null;
}

/** 仅转发：请求原样交给上游，失败了也不重试 */
export function forwardOnly(c: Card): boolean {
  return c.metadata?.delivery === 'forward';
}

/** 这次运行能不能做针对运行的操作（停止、重启、看详情）：要有可信的运行记录，并且进程答复了 */
export function controllable(c: Card): boolean {
  return c.run !== null && (c.liveness === 'live' || c.liveness === 'unresponsive');
}

/** 「救回」：成功了，而且不是第一次就成功的 */
export function rescued(r: Row): boolean {
  return r.outcome === 'ok' && r.attempts > 1;
}

/** 正在重试：还没结束，而且已经不止一次尝试了 */
export function retrying(r: Row): boolean {
  return r.outcome === 'in-flight' && (r.attempts > 1 || r.phase === 'backoff');
}

/** 请求的耗时：结束了按结束时刻，没结束的按界面时钟 */
export function durationOf(r: Row, now: number): number {
  return Math.max(0, (r.endedAt ?? now) - r.startedAt);
}

/**
 * 近一小时的计数：最后一小时那几个桶的和。实例不提供分桶（数组为空）时为 null，界面显示「—」，
 * 不拿别的数去凑。桶不满一小时（实例刚启动）就是已有的那些桶的和
 */
export function lastHour(b: Buckets | undefined, key: 'requests' | 'retries' | 'failures'): number | null {
  if (!b || b.widthSecs <= 0 || b[key].length === 0) return null;
  const n = Math.max(1, Math.round(3600 / b.widthSecs));
  return b[key].slice(-n).reduce((s, v) => s + v, 0);
}

/** 最后一小时的那几个桶，给迷你走势图；没有分桶时为 null */
export function hourBuckets(b: Buckets | undefined): { req: number[]; retry: number[] } | null {
  if (!b || b.widthSecs <= 0 || b.requests.length < 2) return null;
  const n = Math.max(2, Math.round(3600 / b.widthSecs));
  return { req: b.requests.slice(-n), retry: b.retries.slice(-n) };
}

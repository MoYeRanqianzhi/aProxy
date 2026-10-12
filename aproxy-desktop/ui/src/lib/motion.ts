/*
 * 动效：带轻微回弹的弹簧过渡，以及「减少动效」时的退路。
 *
 * 所有 Svelte 过渡都从这里取，它们在创建时读 ui.reducedMotion：
 * 打开减少动效后只剩短促的淡入淡出，没有位移与缩放。
 * 位移与缩放的幅度再乘上主题的动效强度（--motion），时长不变：强度管「动多远」，不管「动多快」。
 */
import type { TransitionConfig } from 'svelte/transition';
import { ui } from './state.svelte';

/** 欠阻尼弹簧：首个峰值约 1.06（轻微回弹），t=1 时精确落在 1 */
export function springOut(t: number): number {
  return t >= 1 ? 1 : 1 - Math.cos(t * Math.PI * 2.5) * Math.exp(-7 * t);
}

/** 快出慢收，用在淡出与颜色 */
export function softOut(t: number): number {
  return 1 - Math.pow(1 - t, 4);
}

/**
 * 动效强度。从 <html> 上的 --motion 读，而不是从主题对象读：变量是唯一的来源，
 * 主题或扩展只要改了这个变量，CSS 动效和这里的过渡就一起变。只在过渡创建时读一次，开销可以忽略。
 */
function strength(): number {
  const v = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--motion'));
  return Number.isFinite(v) ? v : 1;
}

function reduced(): TransitionConfig {
  return { duration: 140, easing: softOut, css: (t) => `opacity:${t}` };
}

/** 弹出：从略小、略低的位置弹到原位。卡片、弹窗、菜单用 */
export function pop(
  _node: Element,
  { delay = 0, y = 10, scale = 0.96, duration = 520 }: { delay?: number; y?: number; scale?: number; duration?: number } = {},
): TransitionConfig {
  if (ui.reducedMotion) return reduced();
  const k = strength();
  const dy = y * k;
  const from = 1 - (1 - scale) * k;
  return {
    delay,
    duration,
    css: (t) => {
      const s = springOut(t);
      const o = Math.min(1, t * 3);
      return `opacity:${o};transform:translateY(${(1 - s) * dy}px) scale(${from + (1 - from) * s})`;
    },
  };
}

/** 抽屉：从右侧滑入，落位时轻轻回弹 */
export function drawer(_node: Element, { x = 56, duration = 560 }: { x?: number; duration?: number } = {}): TransitionConfig {
  if (ui.reducedMotion) return reduced();
  const dx = x * strength();
  return {
    duration,
    css: (t) => {
      const s = springOut(t);
      return `opacity:${Math.min(1, t * 2.5)};transform:translateX(${(1 - s) * dx}px)`;
    },
  };
}

/** 页面切换：淡入并从下方 8px 升起 */
export function pageIn(_node: Element): TransitionConfig {
  if (ui.reducedMotion) return reduced();
  const dy = 8 * strength();
  return {
    duration: 420,
    css: (t) => {
      const s = springOut(t);
      return `opacity:${softOut(t)};transform:translateY(${(1 - s) * dy}px)`;
    },
  };
}

/**
 * 新日志条目滑入（只做位移与透明度，不改行高，给虚拟滚动留路）。
 * enabled 为 false 时不播放：切换筛选让旧条目重新出现时，不该整屏一起滑动。
 */
export function slideIn(_node: Element, { enabled = true }: { enabled?: boolean } = {}): TransitionConfig {
  if (!enabled) return { duration: 0 };
  if (ui.reducedMotion) return reduced();
  const dy = 14 * strength();
  return {
    duration: 460,
    css: (t) => {
      const s = springOut(t);
      return `opacity:${softOut(t)};transform:translateY(${(1 - s) * dy}px)`;
    },
  };
}

export function fadeOut(_node: Element, { duration = 160 }: { duration?: number } = {}): TransitionConfig {
  return { duration: ui.reducedMotion ? 100 : duration, easing: softOut, css: (t) => `opacity:${t}` };
}

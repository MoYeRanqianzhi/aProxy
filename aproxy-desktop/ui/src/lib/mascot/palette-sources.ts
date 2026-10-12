/*
 * 吉祥物调色板的每个角色取自界面的哪个令牌。这张表属于界面而不属于吉祥物契约：界面改版换变量名时只改这里，
 * 吉祥物不受影响。界面里由 palette.svelte.ts 让浏览器把它们算成颜色；scripts/icons.mjs 生成软件图标时在
 * Node 里按同一张表、用 app.css 的浅色令牌自己算（所以这里只能有类型导入）。
 *
 * 浅/深两档用 color-mix 从主色推出来，而不是直接用 --lilac-soft / --lilac-deep：
 * 那两个令牌在深色主题里是反过来的（soft 变暗、deep 变亮，因为它们是「文字配底色」的一对），
 * 而吉祥物的肚皮必须永远比身体浅、描边必须永远比身体深。
 */
import type { MascotPalette } from './types';

export const PALETTE_SOURCES: Record<keyof MascotPalette, string> = {
  primary: 'var(--lilac)',
  primarySoft: 'color-mix(in oklab, var(--lilac) 42%, white)',
  primaryDeep: 'color-mix(in oklab, var(--lilac) 48%, var(--on-fill))',
  ink: 'var(--on-fill)',
  blush: 'var(--pink)',
  cream: 'var(--mascot-body)',
  surface: 'var(--card)',
  ok: 'var(--mint)',
  retry: 'var(--sky)',
  error: 'var(--coral)',
  idle: 'var(--text-3)',
  star: 'var(--butter)',
  shadow: 'var(--on-fill)',
};

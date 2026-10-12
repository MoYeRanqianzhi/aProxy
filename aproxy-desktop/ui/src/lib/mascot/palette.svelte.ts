/*
 * 宿主这一侧：把界面的主题令牌解析成吉祥物契约里的 MascotPalette。
 *
 * 吉祥物不读界面的 CSS 变量（见 types.ts 的说明），所以由这里在主题、配色强度、自定义主题变化后
 * 统一解析一次：用一个隐藏的探针元素让浏览器算出每个令牌的最终颜色（var()、color-mix() 都在这一步展开），
 * 再画到 1×1 的画布上读回 sRGB 像素，得到 #rrggbb。
 * 统一成十六进制是为了让吉祥物输出的 SVG 在任何地方都能用：存成图标文件、交给不认识 oklch() 的图标工具链也不出错。
 */
import { PALETTE_SOURCES } from './palette-sources';
import type { MascotPalette } from './types';

/** 首次解析之前的占位（浅色主题、标准配色的近似值），只在挂载前的一瞬间用得到 */
const FALLBACK: MascotPalette = {
  primary: '#c4a6f5',
  primarySoft: '#e4d7fb',
  primaryDeep: '#715a9c',
  ink: '#2e2550',
  blush: '#f5b5d0',
  cream: '#fffdf8',
  surface: '#ffffff',
  ok: '#7ee0b4',
  retry: '#8fd0f5',
  error: '#f59a8a',
  idle: '#6f6890',
  star: '#f7dc6f',
  shadow: '#2e2550',
};

class MascotPaletteState {
  current = $state<MascotPalette>(FALLBACK);

  private probe: HTMLElement | null = null;
  private ctx: CanvasRenderingContext2D | null = null;

  /** 主题相关的 <html> 属性或自定义主题变量改完之后调用；读计算样式会强制一次同步的样式计算，所以拿到的是新值 */
  refresh() {
    if (!this.probe) {
      this.probe = document.createElement('i');
      // transition:none 不能省：减少动效时全局规则给所有元素加了颜色过渡，探针要是也跟着过渡，
      // 改完颜色立刻读到的是过渡的起点（上一个令牌的颜色），整套调色板会错位一格
      this.probe.style.cssText = 'position:absolute;width:0;height:0;overflow:hidden;visibility:hidden;pointer-events:none;transition:none!important';
      this.probe.setAttribute('aria-hidden', 'true');
      document.body.append(this.probe);
      const canvas = document.createElement('canvas');
      canvas.width = canvas.height = 1;
      this.ctx = canvas.getContext('2d', { willReadFrequently: true });
    }
    const next = {} as MascotPalette;
    for (const key of Object.keys(PALETTE_SOURCES) as (keyof MascotPalette)[]) {
      this.probe.style.color = PALETTE_SOURCES[key];
      next[key] = this.toHex(getComputedStyle(this.probe).color) ?? FALLBACK[key];
    }
    // 颜色没变就不换对象，免得所有吉祥物白白重新挂载、动画从头播
    if ((Object.keys(next) as (keyof MascotPalette)[]).some((k) => next[k] !== this.current[k])) this.current = next;
  }

  private toHex(color: string): string | null {
    const ctx = this.ctx;
    if (!ctx) return null;
    ctx.clearRect(0, 0, 1, 1);
    ctx.fillStyle = '#000';
    ctx.fillStyle = color;
    ctx.fillRect(0, 0, 1, 1);
    const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
    return '#' + [r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('');
  }
}

export const mascotPalette = new MascotPaletteState();

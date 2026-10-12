/*
 * 软件图标：吉祥物的静态图标拼上一种「底」，成为一张独立的 SVG。
 *
 * 吉祥物预览页（proto/MascotLab）用它展示候选；scripts/icons.mjs 用同一个函数生成 aproxy-desktop/icons/
 * 里的 icon.ico 与 icon.png——预览页里看到的就是图标文件的样子。换形象或换底之后重跑那个脚本即可。
 * 脚本在 Node 里直接运行这个文件，所以这里只做字符串拼接、不碰 DOM，也只能有类型导入。
 */
import type { MascotDefinition, MascotPalette } from './types';

export type Plate = 'squircle' | 'circle' | 'sticker';
export const PLATES: readonly Plate[] = ['squircle', 'circle', 'sticker'];

/**
 * 把吉祥物返回的独立 <svg> 改成可以嵌进另一张 SVG 的子元素：换掉根元素上的位置与大小。
 * 契约规定 icon() 返回带 xmlns 的完整 <svg>，根元素的开始标签就在最前面。
 */
function nest(svg: string, x: number, y: number, size: number): string {
  return svg.replace(/^<svg\b[^>]*>/, (open) =>
    open
      .replace(/\s(?:x|y|width|height)="[^"]*"/g, '')
      .replace(/^<svg\b/, `<svg x="${x}" y="${y}" width="${size}" height="${size}"`),
  );
}

/**
 * 底 + 吉祥物拼成一张软件图标。底画在 100×100 的画布上；吉祥物按它最终会占多少像素去挑细节档位，
 * 所以 16px 的方底图标里，吉祥物用的是 16 格那一档。
 * 方底用奶黄（与丁香色的身体互为补色，在两种任务栏上都跳得出来）；圆底用浅丁香加深色细圈；
 * 无底贴纸用一圈白色「裁切边」，在深色任务栏上也有轮廓。
 */
export function appIcon(d: MascotDefinition, plate: Plate, px: number, pal: MascotPalette): string {
  // 16px 时底会吃掉太多像素，吉祥物放大一些，让它压到底的边缘也比缩成一团强
  const inset = plate === 'sticker' ? 7 : px <= 16 ? 8 : 14;
  const inner = 100 - inset * 2;
  // 三种底（奶黄、浅丁香、白色裁切边）都是浅色，所以按浅底取图标细节的颜色，与界面主题无关
  const glyph = nest(d.icon({ px: Math.round((px * inner) / 100), mood: null, theme: 'light', palette: pal }), inset, inset, inner);
  let body = '';
  if (plate === 'squircle') {
    body = `<rect x="3" y="3" width="94" height="94" rx="25" fill="${pal.star}"/>${glyph}`;
  } else if (plate === 'circle') {
    const ring = Math.max(1.2, 100 / px);
    body = `<circle cx="50" cy="50" r="${48 - ring / 2}" fill="${pal.primarySoft}" stroke="${pal.primaryDeep}" stroke-width="${ring}"/>${glyph}`;
  } else {
    // 裁切边：把吉祥物的不透明区域向外膨胀，填白，垫在它下面。宽度按像素算：16px 时 1px，大图 4px
    const cutPx = px <= 16 ? 1 : px <= 48 ? 1.5 : 4;
    const r = (cutPx * 100) / px;
    body =
      `<defs><filter id="cut" x="-20%" y="-20%" width="140%" height="140%">` +
      `<feMorphology in="SourceAlpha" operator="dilate" radius="${r}" result="grow"/>` +
      `<feFlood flood-color="#ffffff"/><feComposite in2="grow" operator="in" result="edge"/>` +
      `<feMerge><feMergeNode in="edge"/><feMergeNode in="SourceGraphic"/></feMerge></filter></defs>` +
      `<g filter="url(#cut)">${glyph}</g>`;
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="${px}" height="${px}">${body}</svg>`;
}

/*
 * 生成桌面界面的软件图标，写进 aproxy-desktop/icons/：
 *   icon.ico  Windows（16/24/32/48/64/256）：tauri-build 把它编进 exe 的资源，窗口与任务栏的图标取自那里；
 *   icon.png  128×128，Linux 与 macOS 的窗口图标（generate_context! 把它解成 RGBA 嵌进二进制，所以不取更大）。
 *
 * 图标就是吉祥物预览页里的「软件图标」：吉祥物的 icon({ px, mood: null }) 拼上一种底（src/lib/mascot/app-icon.ts），
 * 颜色取浅色主题、标准配色强度下的令牌（src/app.css 与 src/lib/mascot/palette-sources.ts）。换形象、换底、改配色
 * 之后重跑即可，生成的文件随仓库提交：
 *   npm run icons                                        默认就是定下的软件图标：不倒翁 + 无底贴纸
 *   npm run icons -- --mascot courier --plate squircle   形象：tumbler | courier | orb；底：squircle | circle | sticker
 *
 * 每个尺寸单独渲染（吉祥物按像素尺寸挑细节档位，不是把大图缩小）。栅格化用 resvg：贴纸的白色裁切边是 SVG 滤镜。
 * ICO 里 256 存 PNG、更小的存 32 位 BMP，与常见的图标工具一致，任何读 ICO 的程序都认得。
 * 颜色在这里按 CSS Color 4 的公式自己算（oklch → oklab → sRGB），超出 sRGB 的分量直接截断。浏览器里的调色板
 * （palette.svelte.ts）由浏览器做色域映射，所以超出 sRGB 的颜色可能差一个色阶（2026-10-12 对过：13 个角色里只有
 * 奶黄 star 差一，#fddc5b 对 #fddd5b）。
 *
 * Node 24 直接运行，被导入的 .ts 文件由 Node 原生剥掉类型标注，所以它们只能有类型导入。
 */
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { Resvg } from '@resvg/resvg-js';
import { appIcon, PLATES } from '../src/lib/mascot/app-icon.ts';
import { PALETTE_SOURCES } from '../src/lib/mascot/palette-sources.ts';
import { courier } from '../src/lib/mascot/courier.ts';
import { orb } from '../src/lib/mascot/orb.ts';
import { tumbler } from '../src/lib/mascot/tumbler.ts';

const MASCOTS = { tumbler, courier, orb };
const ICO_SIZES = [16, 24, 32, 48, 64, 256];
const PNG_SIZE = 128;
const OUT_DIR = new URL('../../icons/', import.meta.url);

const { values: args } = parseArgs({
  options: {
    mascot: { type: 'string', default: 'tumbler' },
    plate: { type: 'string', default: 'sticker' },
  },
});
const mascot = MASCOTS[args.mascot];
if (!mascot) fail(`没有这个吉祥物：${args.mascot}（可选：${Object.keys(MASCOTS).join(' | ')}）`);
if (!PLATES.includes(args.plate)) fail(`没有这种底：${args.plate}（可选：${PLATES.join(' | ')}）`);

const palette = resolvePalette(readFileSync(new URL('../src/app.css', import.meta.url), 'utf8'));
const render = (px) => new Resvg(appIcon(mascot, args.plate, px, palette)).render();

mkdirSync(OUT_DIR, { recursive: true });
writeFileSync(new URL('icon.ico', OUT_DIR), ico(ICO_SIZES.map((px) => ({ px, image: render(px) }))));
writeFileSync(new URL('icon.png', OUT_DIR), render(PNG_SIZE).asPng());
console.log(`icons: ${args.mascot} + ${args.plate} → aproxy-desktop/icons/icon.ico, icon.png`);

function fail(message) {
  console.error(message);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// 颜色：从 app.css 取浅色主题的令牌，按 PALETTE_SOURCES 算出吉祥物调色板（#rrggbb）
// ---------------------------------------------------------------------------

/** 浅色主题、标准配色强度下的调色板。只认 app.css 里实际出现的写法，碰到别的就报错，不猜 */
function resolvePalette(css) {
  const block = /:root,\s*:root\[data-variant='light'\]\s*\{([^}]*)\}/.exec(css);
  const vivid = /:root\[data-palette='standard'\]\s*\{\s*--vivid:\s*([\d.]+);/.exec(css);
  if (!block || !vivid) fail('src/app.css 里找不到浅色主题的令牌块或标准配色强度，图标脚本要跟着它改');
  const tokens = Object.fromEntries([...block[1].matchAll(/--([\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
  const color = (expr) => evaluate(expr, tokens, Number(vivid[1]));
  return Object.fromEntries(Object.entries(PALETTE_SOURCES).map(([role, expr]) => [role, toHex(color(expr))]));
}

/** 一个颜色表达式 → OKLab（{ L, a, b }）。认 var()、oklch()、color-mix(in oklab, …) 与 white */
function evaluate(expr, tokens, vivid) {
  const s = expr.trim();
  let m;
  if (s === 'white') return { L: 1, a: 0, b: 0 };
  if ((m = /^var\(--([\w-]+)\)$/.exec(s))) {
    if (!(m[1] in tokens)) fail(`app.css 的浅色主题里没有令牌 --${m[1]}`);
    return evaluate(tokens[m[1]], tokens, vivid);
  }
  if ((m = /^oklch\((.*)\)$/.exec(s))) {
    const parts = topLevel(m[1], ' ').filter((p) => p !== '');
    if (parts.length !== 3) fail(`图标脚本只认不带透明度的 oklch(L C H)：${s}`);
    const [L, C, H] = parts.map((p) => component(p, vivid));
    const h = (H * Math.PI) / 180;
    return { L, a: C * Math.cos(h), b: C * Math.sin(h) };
  }
  if ((m = /^color-mix\(\s*in oklab\s*,(.*)\)$/.exec(s))) {
    const sides = topLevel(m[1], ',').map((side) => {
      const pm = /^(.*?)\s+([\d.]+)%$/.exec(side.trim());
      return pm ? { color: evaluate(pm[1], tokens, vivid), p: Number(pm[2]) / 100 } : { color: evaluate(side, tokens, vivid), p: null };
    });
    if (sides.length !== 2) fail(`color-mix 要正好两种颜色：${s}`);
    const [x, y] = sides;
    const px = x.p ?? (y.p === null ? 0.5 : 1 - y.p);
    const py = y.p ?? 1 - px;
    const total = px + py;
    return { L: (x.color.L * px + y.color.L * py) / total, a: (x.color.a * px + y.color.a * py) / total, b: (x.color.b * px + y.color.b * py) / total };
  }
  fail(`图标脚本不认识这个颜色写法：${s}`);
}

/** oklch() 的一个分量：数字、百分数，或者配色强度的缩放 calc(N * var(--vivid)) */
function component(part, vivid) {
  let m;
  if ((m = /^calc\(\s*([\d.]+)\s*\*\s*var\(--vivid\)\s*\)$/.exec(part))) return Number(m[1]) * vivid;
  if ((m = /^([\d.]+)%$/.exec(part))) return Number(m[1]) / 100;
  if (/^[\d.]+$/.test(part)) return Number(part);
  fail(`oklch() 里不认识的分量：${part}`);
}

/** 按顶层的分隔符切开（括号里的不切） */
function topLevel(s, sep) {
  const out = [''];
  let depth = 0;
  for (const ch of s) {
    if (ch === '(') depth++;
    if (ch === ')') depth--;
    if (ch === sep && depth === 0) out.push('');
    else out[out.length - 1] += ch;
  }
  return out.map((p) => p.trim());
}

/** OKLab → sRGB（Björn Ottosson 的矩阵，CSS Color 4 用的同一组）→ #rrggbb，超出 [0, 1] 的分量截断 */
function toHex({ L, a, b }) {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const linear = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  return (
    '#' +
    linear
      .map((c) => {
        const v = c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055;
        return Math.round(Math.min(1, Math.max(0, v)) * 255)
          .toString(16)
          .padStart(2, '0');
      })
      .join('')
  );
}

// ---------------------------------------------------------------------------
// ICO：ICONDIR + 每个尺寸一条 ICONDIRENTRY + 图像数据（256 为 PNG，其余为 32 位 BMP）
// ---------------------------------------------------------------------------

function ico(entries) {
  const images = entries.map(({ px, image }) => (px >= 256 ? image.asPng() : bmp(px, image.pixels)));
  const header = Buffer.alloc(6 + 16 * entries.length);
  header.writeUInt16LE(0, 0); // 保留
  header.writeUInt16LE(1, 2); // 1 = 图标
  header.writeUInt16LE(entries.length, 4);
  let offset = header.length;
  entries.forEach(({ px }, i) => {
    const at = 6 + 16 * i;
    header.writeUInt8(px >= 256 ? 0 : px, at); // 宽，0 表示 256
    header.writeUInt8(px >= 256 ? 0 : px, at + 1); // 高
    header.writeUInt8(0, at + 2); // 调色板颜色数（真彩色为 0）
    header.writeUInt8(0, at + 3); // 保留
    header.writeUInt16LE(1, at + 4); // 位面数
    header.writeUInt16LE(32, at + 6); // 每像素位数
    header.writeUInt32LE(images[i].length, at + 8);
    header.writeUInt32LE(offset, at + 12);
    offset += images[i].length;
  });
  return Buffer.concat([header, ...images]);
}

/**
 * ICO 里的 BMP：BITMAPINFOHEADER（高度写成两倍，含后面的 AND 掩码）+ 自下而上的 BGRA 像素 + 1 位 AND 掩码。
 * resvg 给的像素是预乘了透明度的 RGBA，BMP 要的是没有预乘的，先还原。AND 掩码给只认掩码的老程序用：全透明处置 1。
 */
function bmp(px, premultiplied) {
  const maskStride = Math.ceil(px / 32) * 4;
  const info = Buffer.alloc(40);
  info.writeUInt32LE(40, 0); // biSize
  info.writeInt32LE(px, 4); // biWidth
  info.writeInt32LE(px * 2, 8); // biHeight：颜色 + 掩码
  info.writeUInt16LE(1, 12); // biPlanes
  info.writeUInt16LE(32, 14); // biBitCount
  info.writeUInt32LE(0, 16); // biCompression = BI_RGB
  info.writeUInt32LE(px * px * 4 + maskStride * px, 20); // biSizeImage
  const pixels = Buffer.alloc(px * px * 4);
  const mask = Buffer.alloc(maskStride * px);
  for (let y = 0; y < px; y++) {
    const row = px - 1 - y; // 自下而上
    for (let x = 0; x < px; x++) {
      const src = (y * px + x) * 4;
      const alpha = premultiplied[src + 3];
      const straight = (c) => (alpha === 0 ? 0 : Math.min(255, Math.round((c * 255) / alpha)));
      const dst = (row * px + x) * 4;
      pixels[dst] = straight(premultiplied[src + 2]);
      pixels[dst + 1] = straight(premultiplied[src + 1]);
      pixels[dst + 2] = straight(premultiplied[src]);
      pixels[dst + 3] = alpha;
      if (alpha === 0) mask[row * maskStride + (x >> 3)] |= 0x80 >> (x & 7);
    }
  }
  return Buffer.concat([info, pixels, mask]);
}

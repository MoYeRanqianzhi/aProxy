/*
 * 备选吉祥物：几何小球。
 *
 * 和不倒翁刻意走相反的路子：没有嘴、没有腮红，一个正圆加两枚胶囊眼；品牌记号 ↻ 不长在头上，
 * 而是一道绕着小球的环——转起来就是「正在重试」的那个转圈。表情全靠眼睛的几何形状换：
 *   空闲 胶囊眼、慢眨     忙碌 ∩ 笑眼、轻弹     重试 斜切的眼、环飞快地转
 *   救回 星星眼、环亮成庆祝色     出错 小圆点眼、环断开变红     睡觉 一字眼、环褪色停住
 * 它存在的意义之一是证明契约够用：同一份 MascotState，形象、部件、动画都可以完全不同。
 */
import type { MascotDefinition, MascotIconOptions, MascotMood, MascotPalette, MascotState } from './types';

// 环：圆心与小球相同，半径 46，从左下（7 点半）顺时针绕过顶端 270° 到右下（4 点半），末端一枚箭头。
// 缺口留在正下方，小球站着的地方，环不会穿过地面
const RING = 'M27.47 104.53A46 46 0 1 1 92.53 104.53';
const RING_TIP = 'M89.35 107.7L95.9 106.8L90.3 101.1Z';

function eyes(mood: MascotMood, p: MascotPalette): string {
  const ink = p.ink;
  switch (mood) {
    case 'working':
      return `<path d="M44 73Q49 64 54 73M66 73Q71 64 76 73" fill="none" stroke="${ink}" stroke-width="4" stroke-linecap="round"/>`;
    case 'retrying':
      // 胶囊眼各向内斜切一刀：用力的样子
      return (
        `<rect x="46" y="62" width="6.5" height="12" rx="3.2" fill="${ink}" transform="rotate(14 49 68)"/>` +
        `<rect x="67.5" y="62" width="6.5" height="12" rx="3.2" fill="${ink}" transform="rotate(-14 71 68)"/>`
      );
    case 'rescued': {
      const star = (cx: number) =>
        `<path d="M${cx} 59C${cx + 1.2} 66 ${cx + 2} 66.8 ${cx + 8} 68C${cx + 2} 69.2 ${cx + 1.2} 70 ${cx} 77C${cx - 1.2} 70 ${cx - 2} 69.2 ${cx - 8} 68C${cx - 2} 66.8 ${cx - 1.2} 66 ${cx} 59Z" fill="${ink}"/>`;
      return star(49) + star(71);
    }
    case 'worried':
      return `<circle cx="50" cy="72" r="3.6" fill="${ink}"/><circle cx="70" cy="72" r="3.6" fill="${ink}"/>`;
    case 'sleeping':
      return `<path d="M43 70H55M65 70H77" stroke="${ink}" stroke-width="4" stroke-linecap="round"/>`;
    default:
      return (
        `<g class="eyes"><rect x="45.8" y="61" width="6.5" height="14" rx="3.25" fill="${ink}"/>` +
        `<rect x="67.7" y="61" width="6.5" height="14" rx="3.25" fill="${ink}"/></g>`
      );
  }
}

function ringColor(p: MascotPalette, mood: MascotMood, dark: boolean): string {
  if (mood === 'rescued') return p.star;
  if (mood === 'worried') return p.error;
  if (mood === 'retrying') return p.retry;
  return dark ? p.primarySoft : p.primaryDeep;
}

function ball(p: MascotPalette, mood: MascotMood, nonScaling: boolean): string {
  const ns = nonScaling ? ' vector-effect="non-scaling-stroke"' : '';
  return (
    `<circle cx="60" cy="72" r="34" fill="${p.primary}" stroke="${p.primaryDeep}" stroke-width="1.5"${ns}/>` +
    // 左上角一道月牙高光：几何球体的标志
    `<path d="M38 60A26 26 0 0 1 56 44A30 30 0 0 0 42 63Z" fill="${p.primarySoft}" opacity=".9"/>` +
    `<g class="face">${eyes(mood, p)}</g>`
  );
}

function render(s: MascotState): string {
  if (s.size === 'icon') return icon({ px: 32, mood: s.mood, theme: s.theme, palette: s.palette });
  const p = s.palette;
  const dark = s.theme === 'dark';
  const ring = ringColor(p, s.mood, dark);
  // 出错时环断成几截（虚线），睡觉时褪成半透明：环的状态就是连接的状态
  const dash = s.mood === 'worried' ? ' stroke-dasharray="14 9"' : '';
  const fade = s.mood === 'sleeping' ? ' opacity=".35"' : '';

  let fx = '';
  if (s.mood === 'retrying' && s.size === 'hero' && s.retries > 0) {
    const label = `×${s.retries}`;
    const w = 15 + label.length * 7.4;
    fx +=
      `<g class="badge"><rect x="${112 - w}" y="4" width="${w}" height="20" rx="10" fill="${p.retry}" stroke="${p.surface}" stroke-width="2.5"/>` +
      `<text x="${112 - w / 2}" y="18.4" text-anchor="middle" font-size="13" font-weight="700" fill="${p.ink}">${label}</text></g>`;
  }
  if (s.mood === 'rescued') {
    for (const [x, y, r, d] of [
      [14, 34, 6, 0],
      [104, 30, 7.5, 0.12],
      [108, 92, 5, 0.24],
    ]) {
      const k = r * 0.22;
      fx += `<g transform="translate(${x} ${y})"><path class="spark d-${Math.round((d as number) * 100)}" d="M0 ${-r}C${k} ${-k} ${k} ${-k} ${r} 0C${k} ${k} ${k} ${k} 0 ${r}C${-k} ${k} ${-k} ${k} ${-r} 0C${-k} ${-k} ${-k} ${-k} 0 ${-r}Z" fill="${p.star}" stroke="${p.primaryDeep}" stroke-width=".8"/></g>`;
    }
  }
  if (s.mood === 'sleeping') {
    const ink = dark ? p.primarySoft : p.primaryDeep;
    for (const [x, y, k, d] of [
      [92, 30, 7, 0],
      [102, 14, 9, 1.4],
    ]) {
      fx += `<g transform="translate(${x} ${y})"><path class="z d-${Math.round((d as number) * 100)}" d="M0 0H${k}L0 ${k}H${k}" fill="none" stroke="${ink}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/></g>`;
    }
  }

  return (
    `<svg class="orb mood-${s.mood}${s.reducedMotion ? ' still' : ''}" viewBox="0 0 120 120" aria-hidden="true">` +
    `<g class="ground"><ellipse cx="60" cy="114" rx="26" ry="3.6" fill="${p.shadow}" opacity=".08"/><ellipse cx="60" cy="114" rx="17" ry="2.4" fill="${p.shadow}" opacity=".1"/></g>` +
    `<g class="spin"><path d="${RING}" fill="none" stroke="${ring}" stroke-width="3.2" stroke-linecap="round"${dash}${fade}/>` +
    `<path d="${RING_TIP}" fill="${ring}" stroke="${ring}" stroke-width="2" stroke-linejoin="round"${fade}/></g>` +
    `<g class="hop">${ball(p, s.mood, true)}</g>` +
    `<g class="fx">${fx}</g>` +
    `</svg>`
  );
}

function icon(o: MascotIconOptions): string {
  const p = o.palette;
  const mood = o.mood ?? 'idle';
  const dark = o.theme === 'dark';
  const open = (vb: string) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${vb}" width="${o.px}" height="${o.px}">`;
  const dot = (cx: number, r: number, ring: number) => {
    const c = o.mood ? statusColor(p, o.mood) : null;
    return c ? `<circle cx="${cx}" cy="${cx}" r="${r + ring}" fill="${p.surface}"/><circle cx="${cx}" cy="${cx}" r="${r}" fill="${c}"/>` : '';
  };
  const ring = o.mood ? ringColor(p, mood, dark) : dark ? p.primarySoft : p.primaryDeep;

  if (o.px >= 64) {
    return (
      open('6 13 108 108') +
      `<path d="${RING}" fill="none" stroke="${ring}" stroke-width="3.2" stroke-linecap="round"/><path d="${RING_TIP}" fill="${ring}" stroke="${ring}" stroke-width="2" stroke-linejoin="round"/>` +
      ball(p, mood, false) +
      dot(98, 11, 4) +
      `</svg>`
    );
  }
  // 小尺寸：环加粗、球放大，眼睛只留两条竖线
  const eyes =
    mood === 'sleeping'
      ? `<path d="M5.4 8.6H7.2M8.8 8.6H10.6" stroke="${p.ink}" stroke-width="1.1" stroke-linecap="round"/>`
      : `<rect x="5.6" y="7.3" width="1.3" height="2.6" rx=".65" fill="${p.ink}"/><rect x="9.1" y="7.3" width="1.3" height="2.6" rx=".65" fill="${p.ink}"/>`;
  return (
    open('0 0 16 16') +
    `<path d="M3.33 13.27A6.6 6.6 0 1 1 12.67 13.27" fill="none" stroke="${ring}" stroke-width="1.4" stroke-linecap="round"/>` +
    `<circle cx="8" cy="8.6" r="4.6" fill="${p.primary}" stroke="${p.primaryDeep}" stroke-width=".9"/>` +
    eyes +
    dot(12.6, 2.6, 1.2) +
    `</svg>`
  );
}

function statusColor(p: MascotPalette, mood: MascotMood): string | null {
  switch (mood) {
    case 'working':
    case 'rescued':
      return p.ok;
    case 'retrying':
      return p.retry;
    case 'worried':
      return p.error;
    case 'sleeping':
      return p.idle;
    default:
      return null;
  }
}

const css = /* css */ `
  svg { display: block; width: 100%; height: 100%; overflow: visible; }
  .spin { transform-box: view-box; transform-origin: 60px 72px; }
  .hop { transform-box: view-box; transform-origin: 60px 106px; }
  .ground { transform-box: view-box; transform-origin: 60px 114px; }
  .eyes, .spark, .z, .badge { transform-box: fill-box; transform-origin: center; }
  .still * { animation: none !important; }

  .mood-idle .spin { animation: orb-spin 14s linear infinite; }
  .mood-idle .eyes { animation: orb-blink 5s ease-in-out infinite; }
  .mood-working .spin { animation: orb-spin 2.6s linear infinite; }
  .mood-working .hop { animation: orb-bounce 0.9s ease-in-out infinite; }
  /* 重试：环像加载圈一样飞快地转，小球每次重新挂载（又重试了一次）被压扁一下再弹回 */
  .mood-retrying .spin { animation: orb-spin 0.9s linear infinite; }
  .mood-retrying .hop { animation: orb-squash 0.9s cubic-bezier(0.3, 1.5, 0.5, 1) both; }
  .mood-retrying .badge { animation: orb-pop 0.45s cubic-bezier(0.3, 1.6, 0.5, 1) both; }
  .mood-rescued .hop { animation: orb-leap 1.2s both; }
  .mood-rescued .spin { animation: orb-spin 1.2s cubic-bezier(0.2, 0.8, 0.3, 1) both; }
  .mood-rescued .spark { animation: orb-twinkle 1.5s ease-out both; }
  .mood-worried .hop { animation: orb-shiver 3s ease-in-out infinite; }
  .mood-sleeping .hop { animation: orb-breathe 4.2s ease-in-out infinite; }
  .mood-sleeping .z { animation: orb-z 3.4s ease-out infinite both; }

  @keyframes orb-spin { to { transform: rotate(360deg); } }
  @keyframes orb-blink { 0%, 93%, 100% { transform: scaleY(1); } 96% { transform: scaleY(0.12); } }
  @keyframes orb-bounce { 0%, 100% { transform: translateY(0) scale(1, 1); } 15% { transform: scale(1.05, 0.95); } 50% { transform: translateY(-6px) scale(0.98, 1.02); } 85% { transform: scale(1.04, 0.96); } }
  @keyframes orb-squash { 0% { transform: scale(1.18, 0.8); } 100% { transform: scale(1, 1); } }
  @keyframes orb-pop { 0% { transform: scale(0.3); } 100% { transform: scale(1); } }
  @keyframes orb-leap {
    0% { transform: translateY(0) scale(1, 1); } 15% { transform: scale(1.1, 0.88); }
    45% { transform: translateY(-14px) scale(0.95, 1.06); } 72% { transform: translateY(0) scale(1.08, 0.92); } 100% { transform: translateY(0) scale(1, 1); }
  }
  @keyframes orb-twinkle { 0% { opacity: 0; transform: scale(0); } 35% { opacity: 1; transform: scale(1.2); } 100% { opacity: 0; transform: scale(0.5) rotate(40deg); } }
  @keyframes orb-shiver { 0%, 100% { transform: translateX(0); } 20% { transform: translateX(-1.5px); } 40% { transform: translateX(1.5px); } 60% { transform: translateX(-1px); } 80% { transform: translateX(0.5px); } }
  @keyframes orb-breathe { 0%, 100% { transform: scale(1, 1); } 50% { transform: scale(1.03, 0.97); } }
  @keyframes orb-z { 0% { opacity: 0; transform: translate(-3px, 5px) scale(0.6); } 30%, 75% { opacity: 1; } 100% { opacity: 0; transform: translate(3px, -7px) scale(1); } }

  /* 错开的出场时刻写成类而不是内联 style：界面的 CSP 不允许内联样式属性 */
  .fx .d-12 { animation-delay: 0.12s; }
  .fx .d-24 { animation-delay: 0.24s; }
  .fx .d-140 { animation-delay: 1.4s; }
`;

export const orb: MascotDefinition = {
  id: 'orb',
  name: { en: 'Orb', 'zh-CN': '几何小球' },
  description: {
    en: 'A minimal ball inside its own retry ring.',
    'zh-CN': '极简的小球，绕着一道会转的重试环。',
  },
  css,
  render,
  icon,
};

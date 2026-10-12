/*
 * 备选吉祥物：信使团子。
 *
 * 一颗奶白色的扁团子，戴一顶主色的邮差帽（帽子正面印着 ↻），两只小手捧着一封信——
 * 它的工作就是把请求送到，送不到就再送一次。和不倒翁不同，它的状态主要写在信上：
 *   空闲 抱着信眨眼     忙碌 抱着信小步颠     重试 把信重新往前递一下，火漆变成 ↻
 *   救回 信封打开、冒出一颗心     出错 火漆变成「!」、歪着担心     睡觉 闭眼、帽子歪了
 */
import type { MascotDefinition, MascotIconOptions, MascotMood, MascotPalette, MascotState } from './types';

const BODY = 'M60 40C86 40 101 57 101 77C101 97 83 108 60 108C37 108 19 97 19 77C19 57 34 40 60 40Z';
const CAP = 'M35 51C35 32 85 32 85 51Q60 45.5 35 51Z';
const BRIM = 'M78 49.5Q92 46.5 97 52.5Q88 55.5 78 53Z';

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

function face(mood: MascotMood, p: MascotPalette): string {
  const ink = p.ink;
  const line = (d: string, w = 2.3) =>
    `<path d="${d}" fill="none" stroke="${ink}" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round"/>`;
  const beans = (ry = 4.3) =>
    `<g class="eyes"><ellipse cx="49" cy="63" rx="3.4" ry="${ry}" fill="${ink}"/><ellipse cx="71" cy="63" rx="3.4" ry="${ry}" fill="${ink}"/>` +
    `<circle cx="50.3" cy="61.4" r="1.2" fill="#fff"/><circle cx="72.3" cy="61.4" r="1.2" fill="#fff"/></g>`;
  switch (mood) {
    case 'working':
    case 'rescued':
      return line('M45 64.5Q49 58.5 53 64.5', 2.6) + line('M67 64.5Q71 58.5 75 64.5', 2.6) + line('M56.5 69Q60 72.4 63.5 69', 2.2);
    case 'retrying':
      return beans(3.8) + line('M44 56L53 58.2', 2.4) + line('M76 56L67 58.2', 2.4) + line('M56.5 70.4H63.5', 2.2);
    case 'worried':
      return beans(3.8) + line('M44.5 57.6L52.5 55', 2.3) + line('M75.5 57.6L67.5 55', 2.3) + line('M55.5 71Q57.75 69 60 71Q62.25 73 64.5 71', 2);
    case 'sleeping':
      return line('M45 63Q49 66.5 53 63', 2.3) + line('M67 63Q71 66.5 75 63', 2.3);
    default:
      return beans() + line('M56.5 69.4Q60 72.4 63.5 69.4', 2.2);
  }
}

/** 信：信封、封口、火漆。火漆的图案就是状态 */
function letter(mood: MascotMood, p: MascotPalette): string {
  const deep = p.primaryDeep;
  const open = mood === 'rescued';
  const flap = open
    ? `<path d="M38.6 75L60 60L81.4 75Z" fill="${p.primarySoft}" stroke="${deep}" stroke-width="1.4" stroke-linejoin="round"/>`
    : `<path d="M38.6 75L60 89.5L81.4 75" fill="none" stroke="${deep}" stroke-width="1.4" stroke-linejoin="round"/>`;
  let seal = '';
  if (mood === 'retrying') {
    seal =
      `<g class="seal"><circle cx="60" cy="89.5" r="6" fill="${p.retry}" stroke="${deep}" stroke-width="1.2"/>` +
      `<path d="M57 91.5A3.4 3.4 0 1 1 63.2 90.4" fill="none" stroke="${p.ink}" stroke-width="1.5" stroke-linecap="round"/>` +
      `<path d="M61.6 92.3L64.6 91.6L62.6 88.9Z" fill="${p.ink}"/></g>`;
  } else if (mood === 'worried') {
    seal =
      `<circle cx="60" cy="89.5" r="6" fill="${p.error}" stroke="${deep}" stroke-width="1.2"/>` +
      `<rect x="59.1" y="85.6" width="1.8" height="4.6" rx=".9" fill="${p.ink}"/><circle cx="60" cy="92.6" r="1.1" fill="${p.ink}"/>`;
  } else if (!open) {
    seal = `<path d="M60 93.6C57 91.2 55.4 89.8 55.4 88.2C55.4 86.8 56.5 85.8 57.8 85.8C58.8 85.8 59.5 86.4 60 87.2C60.5 86.4 61.2 85.8 62.2 85.8C63.5 85.8 64.6 86.8 64.6 88.2C64.6 89.8 63 91.2 60 93.6Z" fill="${p.blush}" stroke="${deep}" stroke-width="1"/>`;
  }
  // 救回：信封打开，一颗心从里面冒出来
  const heart = open
    ? `<g class="heart"><path d="M60 64C54 59.2 50.8 56.4 50.8 53.2C50.8 50.4 53 48.4 55.6 48.4C57.6 48.4 59 49.6 60 51.2C61 49.6 62.4 48.4 64.4 48.4C67 48.4 69.2 50.4 69.2 53.2C69.2 56.4 66 59.2 60 64Z" fill="${p.blush}" stroke="${deep}" stroke-width="1.3"/></g>`
    : '';
  return (
    `<g class="letter">${heart}` +
    `<rect x="38" y="74" width="44" height="29" rx="4" fill="${p.primarySoft}" stroke="${deep}" stroke-width="1.4"/>` +
    flap +
    seal +
    // 两只小手扒着信封两边
    `<ellipse cx="37.5" cy="87" rx="5" ry="6" fill="${p.cream}" stroke="${deep}" stroke-width="1.3"/>` +
    `<ellipse cx="82.5" cy="87" rx="5" ry="6" fill="${p.cream}" stroke="${deep}" stroke-width="1.3"/>` +
    `</g>`
  );
}

function figure(p: MascotPalette, mood: MascotMood, nonScaling: boolean): string {
  const ns = nonScaling ? ' vector-effect="non-scaling-stroke"' : '';
  const capTilt = mood === 'sleeping' ? -12 : 0;
  return (
    `<path d="${BODY}" fill="${p.cream}" stroke="${p.primaryDeep}" stroke-width="1.5"${ns}/>` +
    `<g class="cap" transform="rotate(${capTilt} 60 46)">` +
    `<path d="${BRIM}" fill="${p.primaryDeep}"/>` +
    `<path d="${CAP}" fill="${p.primary}" stroke="${p.primaryDeep}" stroke-width="1.5"${ns} stroke-linejoin="round"/>` +
    // 帽子正面的 ↻：品牌记号
    `<path d="M57.03 44.47A4.2 4.2 0 1 1 63.64 43.6" fill="none" stroke="${p.cream}" stroke-width="1.8" stroke-linecap="round"/>` +
    `<path d="M62.34 45.85L65.75 44.35L61.93 42.15Z" fill="${p.cream}" stroke="${p.cream}" stroke-width=".6" stroke-linejoin="round"/>` +
    `</g>` +
    `<ellipse cx="38.5" cy="70" rx="5.2" ry="3.2" fill="${p.blush}" opacity=".85"/>` +
    `<ellipse cx="81.5" cy="70" rx="5.2" ry="3.2" fill="${p.blush}" opacity=".85"/>` +
    `<g class="face">${face(mood, p)}</g>` +
    letter(mood, p)
  );
}

function render(s: MascotState): string {
  if (s.size === 'icon') return icon({ px: 32, mood: s.mood, theme: s.theme, palette: s.palette });
  const p = s.palette;
  const hero = s.size === 'hero';
  const dark = s.theme === 'dark';
  const lean = s.mood === 'worried' ? -7 : 0;
  let fx = '';
  if (s.mood === 'retrying') {
    fx += `<g class="sweat"><path d="M92 46C89.5 50 88.1 52.2 88.1 54.2A3.9 3.9 0 0 0 95.9 54.2C95.9 52.2 94.5 50 92 46Z" fill="${p.retry}" stroke="${p.primaryDeep}" stroke-width="1"/></g>`;
    if (hero && s.retries > 0) {
      const label = `×${s.retries}`;
      const w = 15 + label.length * 7.4;
      fx +=
        `<g class="badge"><rect x="${112 - w}" y="12" width="${w}" height="20" rx="10" fill="${p.retry}" stroke="${p.surface}" stroke-width="2.5"/>` +
        `<text x="${112 - w / 2}" y="26.4" text-anchor="middle" font-size="13" font-weight="700" fill="${p.ink}">${label}</text></g>`;
    }
  }
  if (s.mood === 'worried') {
    fx += `<g class="sweat"><path d="M27 46C24.5 50 23.1 52.2 23.1 54.2A3.9 3.9 0 0 0 30.9 54.2C30.9 52.2 29.5 50 27 46Z" fill="${p.retry}" stroke="${p.primaryDeep}" stroke-width="1"/></g>`;
  }
  if (s.mood === 'rescued') {
    for (const [x, y, r, d] of [
      [16, 40, 7, 0],
      [104, 30, 8, 0.12],
      [12, 92, 5, 0.24],
      [108, 88, 6, 0.18],
    ]) {
      const k = r * 0.22;
      fx += `<g transform="translate(${x} ${y})"><path class="spark d-${Math.round((d as number) * 100)}" d="M0 ${-r}C${k} ${-k} ${k} ${-k} ${r} 0C${k} ${k} ${k} ${k} 0 ${r}C${-k} ${k} ${-k} ${k} ${-r} 0C${-k} ${-k} ${-k} ${-k} 0 ${-r}Z" fill="${p.star}" stroke="${p.primaryDeep}" stroke-width=".8"/></g>`;
    }
  }
  if (s.mood === 'sleeping') {
    const ink = dark ? p.primarySoft : p.primaryDeep;
    for (const [x, y, k, d] of [
      [90, 34, 7, 0],
      [100, 18, 9, 1.3],
    ]) {
      fx += `<g transform="translate(${x} ${y})"><path class="z d-${Math.round((d as number) * 100)}" d="M0 0H${k}L0 ${k}H${k}" fill="none" stroke="${ink}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/></g>`;
    }
  }
  return (
    `<svg class="courier mood-${s.mood}${s.reducedMotion ? ' still' : ''}" viewBox="0 0 120 120" aria-hidden="true">` +
    `<g class="ground"><ellipse cx="60" cy="112" rx="36" ry="4" fill="${p.shadow}" opacity=".07"/><ellipse cx="60" cy="112" rx="26" ry="2.6" fill="${p.shadow}" opacity=".1"/></g>` +
    `<g class="hop"><g transform="rotate(${lean} 60 108)">${figure(p, s.mood, true)}</g></g>` +
    `<g class="fx">${fx}</g>` +
    `</svg>`
  );
}

function icon(o: MascotIconOptions): string {
  const p = o.palette;
  const mood = o.mood ?? 'idle';
  const c = o.mood ? statusColor(p, o.mood) : null;
  const open = (vb: string) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${vb}" width="${o.px}" height="${o.px}">`;
  if (o.px >= 64) {
    const dot = c ? `<circle cx="98" cy="100" r="15" fill="${p.surface}"/><circle cx="98" cy="100" r="11" fill="${c}"/>` : '';
    return open('12 20 96 96') + figure(p, mood, false) + dot + `</svg>`;
  }
  // 小图标：扁团子 + 帽子 + 一块信封，眼睛两粒，信封在 16px 下也还认得出是个长方块
  const dot = c ? `<circle cx="12.7" cy="12.7" r="3.8" fill="${p.surface}"/><circle cx="12.7" cy="12.7" r="2.6" fill="${c}"/>` : '';
  const eyes =
    mood === 'sleeping'
      ? `<path d="M5.4 8.2H6.9M9.1 8.2H10.6" stroke="${p.ink}" stroke-width="1" stroke-linecap="round"/>`
      : `<rect x="5.6" y="7.2" width="1.2" height="1.8" rx=".6" fill="${p.ink}"/><rect x="9.2" y="7.2" width="1.2" height="1.8" rx=".6" fill="${p.ink}"/>`;
  return (
    open('0 0 16 16') +
    `<ellipse cx="8" cy="9.6" rx="6.6" ry="5.6" fill="${p.cream}" stroke="${p.primaryDeep}" stroke-width=".9"/>` +
    `<path d="M3.6 6.2C3.6 2.6 12.4 2.6 12.4 6.2Q8 5.2 3.6 6.2Z" fill="${p.primary}" stroke="${p.primaryDeep}" stroke-width=".8" stroke-linejoin="round"/>` +
    eyes +
    `<rect x="4.6" y="10" width="6.8" height="4.4" rx=".8" fill="${p.primarySoft}" stroke="${p.primaryDeep}" stroke-width=".8"/>` +
    dot +
    `</svg>`
  );
}

const css = /* css */ `
  svg { display: block; width: 100%; height: 100%; overflow: visible; }
  .hop { transform-box: view-box; transform-origin: 60px 108px; }
  .ground { transform-box: view-box; transform-origin: 60px 112px; }
  .letter { transform-box: view-box; transform-origin: 60px 88px; }
  .seal, .heart, .eyes, .spark, .z, .badge { transform-box: fill-box; transform-origin: center; }
  .still * { animation: none !important; }

  .mood-idle .eyes { animation: cr-blink 5.4s ease-in-out infinite; }
  .mood-idle .hop { animation: cr-float 4.6s ease-in-out infinite; }
  .mood-working .hop { animation: cr-trot 0.8s ease-in-out infinite; }
  .mood-working .letter { animation: cr-bob 0.8s ease-in-out 0.12s infinite; }
  /* 重试：每次重新挂载（又重试了一次）把信往前递一下再收回，火漆上的 ↻ 转一圈 */
  .mood-retrying .letter { animation: cr-resend 1.1s cubic-bezier(0.3, 1.4, 0.5, 1) both; }
  .mood-retrying .seal { animation: cr-turn 0.9s ease-in-out 0.15s both; }
  .mood-retrying .badge { animation: cr-pop 0.45s cubic-bezier(0.3, 1.6, 0.5, 1) both; }
  .mood-retrying .sweat, .mood-worried .sweat { animation: cr-sweat 2.4s ease-in infinite both; }
  .mood-rescued .hop { animation: cr-leap 1.3s both; }
  .mood-rescued .heart { animation: cr-heart 1.3s cubic-bezier(0.3, 1.5, 0.5, 1) both; }
  .mood-rescued .spark { animation: cr-twinkle 1.6s ease-out both; }
  .mood-worried .hop { animation: cr-wobble 2.8s ease-in-out infinite; }
  .mood-sleeping .hop { animation: cr-breathe 4s ease-in-out infinite; }
  .mood-sleeping .z { animation: cr-z 3.4s ease-out infinite both; }

  @keyframes cr-blink { 0%, 92%, 100% { transform: scaleY(1); } 95% { transform: scaleY(0.12); } }
  @keyframes cr-float { 0%, 100% { transform: translateY(0); } 50% { transform: translateY(-2.5px); } }
  @keyframes cr-trot { 0%, 100% { transform: translateY(0) scale(1, 1); } 25% { transform: translateY(-4px) scale(0.99, 1.01); } 50% { transform: translateY(0) scale(1.03, 0.97); } 75% { transform: translateY(-4px); } }
  @keyframes cr-bob { 0%, 100% { transform: translateY(0); } 50% { transform: translateY(1.5px); } }
  @keyframes cr-resend { 0% { transform: translateX(0) rotate(0); } 30% { transform: translateX(7px) rotate(4deg); } 100% { transform: translateX(0) rotate(0); } }
  @keyframes cr-turn { to { transform: rotate(360deg); } }
  @keyframes cr-pop { 0% { transform: scale(0.3); } 100% { transform: scale(1); } }
  @keyframes cr-sweat { 0% { opacity: 0; transform: translateY(-3px); } 20% { opacity: 1; } 75% { opacity: 1; transform: translateY(4px); } 100% { opacity: 0; transform: translateY(7px); } }
  @keyframes cr-leap { 0% { transform: translateY(0) scale(1, 1); } 15% { transform: scale(1.08, 0.9); } 45% { transform: translateY(-12px) scale(0.96, 1.05); } 72% { transform: translateY(0) scale(1.06, 0.94); } 100% { transform: translateY(0) scale(1, 1); } }
  @keyframes cr-heart { 0% { opacity: 0; transform: translateY(14px) scale(0.2); } 45% { opacity: 1; transform: translateY(-4px) scale(1.15); } 100% { opacity: 1; transform: translateY(0) scale(1); } }
  @keyframes cr-twinkle { 0% { opacity: 0; transform: scale(0); } 35% { opacity: 1; transform: scale(1.2); } 100% { opacity: 0; transform: scale(0.5) rotate(40deg); } }
  @keyframes cr-wobble { 0%, 100% { transform: rotate(0); } 50% { transform: rotate(2.4deg); } }
  @keyframes cr-breathe { 0%, 100% { transform: scale(1, 1); } 50% { transform: scale(1.015, 0.975); } }
  @keyframes cr-z { 0% { opacity: 0; transform: translate(-3px, 5px) scale(0.6); } 30%, 75% { opacity: 1; } 100% { opacity: 0; transform: translate(3px, -7px) scale(1); } }

  /* 错开的出场时刻写成类而不是内联 style：界面的 CSP 不允许内联样式属性 */
  .fx .d-12 { animation-delay: 0.12s; }
  .fx .d-18 { animation-delay: 0.18s; }
  .fx .d-24 { animation-delay: 0.24s; }
  .fx .d-130 { animation-delay: 1.3s; }
`;

export const courier: MascotDefinition = {
  id: 'courier',
  name: { en: 'Courier', 'zh-CN': '信使团子' },
  description: {
    en: 'Carries your request; if it bounces, it delivers it again.',
    'zh-CN': '捧着你的请求去送，送不到就再送一次。',
  },
  css,
  render,
  icon,
};

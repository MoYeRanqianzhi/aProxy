/*
 * 一个「用户自己装的」自定义吉祥物示例：小云。原型把它放在 public/mascots/nimbus/ 冒充扩展文件夹
 * ~/.aproxy/mascots/nimbus/ 里的 mascot.js。
 *
 * 它就是第三方作者会交出来的东西：一个纯 JavaScript 的 ES 模块，默认导出一个吉祥物定义
 * （契约见 src/lib/mascot/types.ts：拿 MascotState、返回 SVG 字符串、样式全在 css 里）。
 * 界面从不执行它：宿主经 Backend.readMascot 取到源码，原样交给沙箱 iframe（public/mascot-sandbox.js），
 * 在那里 import、渲染。所以它不能也不需要引用界面的任何东西。
 *
 * 天气就是它的表情：
 *   空闲 慢慢飘、偶尔眨眼     忙碌 弯弯笑眼、飘得快些     重试 下雨（上游在下雨，它在扛）
 *   救回 太阳从身后探出来     出错 底下打了一道闪电     睡觉 闭眼、Zzz
 */

// 云朵 = 一条底边胶囊 + 三团圆。描边的办法：先把这几块全部加粗描边画一遍，再在上面不描边地填一遍，
// 内部重叠处的描边被盖掉，只剩外轮廓露在外面的那一半（3px 的一半，正好 1.5px）
const PUFFS = [
  '<rect x="22" y="70" width="76" height="28" rx="14"/>',
  '<circle cx="42" cy="72" r="17"/>',
  '<circle cx="63" cy="60" r="24"/>',
  '<circle cx="84" cy="74" r="14"/>',
].join('');

function face(mood, p) {
  const ink = p.ink;
  const line = (d, w = 3.2) => `<path d="${d}" fill="none" stroke="${ink}" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round"/>`;
  const blush = `<ellipse cx="46" cy="78" rx="4.6" ry="3" fill="${p.blush}" opacity=".75"/><ellipse cx="80" cy="78" rx="4.6" ry="3" fill="${p.blush}" opacity=".75"/>`;
  switch (mood) {
    case 'working':
      return blush + line('M50 71Q54 65.5 58 71M68 71Q72 65.5 76 71') + `<path d="M58 77Q63 84 68 77Z" fill="${ink}"/>`;
    case 'retrying':
      // 眉毛压低、嘴抿成一条线：在使劲
      return (
        blush +
        line('M48.5 63.5L57 65.5M77.5 63.5L69 65.5', 2.6) +
        `<ellipse cx="54" cy="70.5" rx="2.6" ry="3.2" fill="${ink}"/><ellipse cx="72" cy="70.5" rx="2.6" ry="3.2" fill="${ink}"/>` +
        line('M59 79.5H67', 2.8)
      );
    case 'rescued':
      return blush + line('M50 72Q54 66 58 72M68 72Q72 66 76 72', 3.4) + `<path d="M57 77Q63 86 69 77Z" fill="${ink}"/>`;
    case 'worried':
      return (
        line('M48 66.5L56 64M78 66.5L70 64', 2.6) +
        `<circle cx="54" cy="71" r="2.6" fill="${ink}"/><circle cx="72" cy="71" r="2.6" fill="${ink}"/>` +
        line('M57.5 80.5Q60.25 78 63 80.5T68.5 80.5', 2.6)
      );
    case 'sleeping':
      return blush + line('M50 70Q54 73.5 58 70M68 70Q72 73.5 76 70', 2.8) + `<ellipse cx="63" cy="79" rx="2.2" ry="1.8" fill="${ink}"/>`;
    default:
      return (
        blush +
        `<g class="eyes"><ellipse cx="54" cy="70" rx="2.6" ry="3.6" fill="${ink}"/><ellipse cx="72" cy="70" rx="2.6" ry="3.6" fill="${ink}"/></g>` +
        line('M59 77Q63 80.5 67 77', 2.8)
      );
  }
}

function body(p, mood) {
  return (
    `<g fill="${p.primaryDeep}" stroke="${p.primaryDeep}" stroke-width="3" stroke-linejoin="round">${PUFFS}</g>` +
    `<g fill="${p.cream}">${PUFFS}</g>` +
    // 肚子底下一道浅浅的主色：云的阴面，也让它在白卡片上不至于只剩一圈线
    `<rect x="32" y="86" width="56" height="8" rx="4" fill="${p.primarySoft}" opacity=".7"/>` +
    `<g class="face">${face(mood, p)}</g>`
  );
}

function render(s) {
  if (s.size === 'icon') return icon({ px: 32, mood: s.mood, theme: s.theme, palette: s.palette });
  const p = s.palette;
  const dark = s.theme === 'dark';
  let back = '';
  let fx = '';

  if (s.mood === 'rescued') {
    // 太阳画在云的后面：从云的右上角探出来
    let rays = '';
    for (let i = 0; i < 8; i++) {
      const a = (i * Math.PI) / 4;
      const x1 = (Math.cos(a) * 17).toFixed(2);
      const y1 = (Math.sin(a) * 17).toFixed(2);
      const x2 = (Math.cos(a) * 23).toFixed(2);
      const y2 = (Math.sin(a) * 23).toFixed(2);
      rays += `M${x1} ${y1}L${x2} ${y2}`;
    }
    back =
      `<g transform="translate(91 38)"><g class="sun">` +
      `<path d="${rays}" stroke="${p.star}" stroke-width="3.2" stroke-linecap="round"/>` +
      `<circle r="13" fill="${p.star}" stroke="${p.primaryDeep}" stroke-width="1.5"/></g></g>`;
    for (const [x, y, r, d] of [
      [14, 40, 6, 12],
      [104, 82, 5, 24],
    ]) {
      const k = r * 0.22;
      fx += `<g transform="translate(${x} ${y})"><path class="spark d-${d}" d="M0 ${-r}C${k} ${-k} ${k} ${-k} ${r} 0C${k} ${k} ${k} ${k} 0 ${r}C${-k} ${k} ${-k} ${k} ${-r} 0C${-k} ${-k} ${-k} ${-k} 0 ${-r}Z" fill="${p.star}" stroke="${p.primaryDeep}" stroke-width=".8"/></g>`;
    }
  }
  if (s.mood === 'retrying') {
    // 四滴雨轮流落下：上游在下雨
    for (const [x, d] of [
      [38, 0],
      [55, 45],
      [71, 15],
      [86, 60],
    ]) {
      fx += `<g transform="translate(${x} 101)"><path class="drop d-${d}" d="M0 0C2.2 3.2 3.4 5.2 3.4 6.8A3.4 3.4 0 0 1 -3.4 6.8C-3.4 5.2 -2.2 3.2 0 0Z" fill="${p.retry}"/></g>`;
    }
    if (s.size === 'hero' && s.retries > 0) {
      const label = `×${s.retries}`;
      const w = 15 + label.length * 7.4;
      fx +=
        `<g class="badge"><rect x="${112 - w}" y="14" width="${w}" height="20" rx="10" fill="${p.retry}" stroke="${p.surface}" stroke-width="2.5"/>` +
        `<text x="${112 - w / 2}" y="28.4" text-anchor="middle" font-size="13" font-weight="700" fill="${p.ink}">${label}</text></g>`;
    }
  }
  if (s.mood === 'worried') {
    fx += `<path class="bolt" d="M64 100L56 111H63L59 120L72 106H64.5L69 100Z" fill="${p.error}" stroke="${p.primaryDeep}" stroke-width="1.2" stroke-linejoin="round"/>`;
  }
  if (s.mood === 'sleeping') {
    const ink = dark ? p.primarySoft : p.primaryDeep;
    for (const [x, y, k, d] of [
      [96, 34, 7, 0],
      [106, 18, 9, 140],
    ]) {
      fx += `<g transform="translate(${x} ${y})"><path class="z d-${d}" d="M0 0H${k}L0 ${k}H${k}" fill="none" stroke="${ink}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/></g>`;
    }
  }

  // 云是飘着的：落地影子画得小而淡，和身体之间留出空隙
  return (
    `<svg class="nimbus mood-${s.mood}${s.reducedMotion ? ' still' : ''}" viewBox="0 0 120 120" aria-hidden="true">` +
    `<ellipse class="ground" cx="60" cy="116" rx="22" ry="2.6" fill="${p.shadow}" opacity=".08"/>` +
    back +
    `<g class="float">${body(p, s.mood)}</g>` +
    `<g class="fx">${fx}</g>` +
    `</svg>`
  );
}

function statusColor(p, mood) {
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

function icon(o) {
  const p = o.palette;
  const mood = o.mood ?? 'idle';
  const open = (vb) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${vb}" width="${o.px}" height="${o.px}">`;
  const c = o.mood ? statusColor(p, o.mood) : null;

  if (o.px >= 64) {
    const dot = c ? `<circle cx="98" cy="98" r="15" fill="${p.surface}"/><circle cx="98" cy="98" r="11" fill="${c}"/>` : '';
    return open('14 26 92 92') + body(p, mood) + dot + `</svg>`;
  }
  // 小尺寸按 16 格设计：三团圆简化成两团，只留轮廓、两只眼和状态点
  const puffs = '<rect x="1.5" y="7.5" width="13" height="6" rx="3"/><circle cx="5.6" cy="8" r="3.3"/><circle cx="9.6" cy="6.8" r="4.2"/>';
  const eyes =
    mood === 'sleeping'
      ? `<path d="M6 9.6H7.6M10.4 9.6H12" stroke="${p.ink}" stroke-width="1" stroke-linecap="round"/>`
      : `<circle cx="6.9" cy="9.5" r=".85" fill="${p.ink}"/><circle cx="11.1" cy="9.5" r=".85" fill="${p.ink}"/>`;
  const dot = c ? `<circle cx="13" cy="13" r="3.6" fill="${p.surface}"/><circle cx="13" cy="13" r="2.5" fill="${c}"/>` : '';
  return (
    open('0 0 16 16') +
    `<g fill="${p.primaryDeep}" stroke="${p.primaryDeep}" stroke-width="1.4" stroke-linejoin="round">${puffs}</g>` +
    `<g fill="${p.cream}">${puffs}</g>` +
    eyes +
    dot +
    `</svg>`
  );
}

const css = /* css */ `
  svg { display: block; width: 100%; height: 100%; overflow: visible; }
  .float, .ground { transform-box: view-box; transform-origin: 60px 90px; }
  .eyes, .drop, .spark, .z, .badge, .bolt, .sun { transform-box: fill-box; transform-origin: center; }
  .still * { animation: none !important; }

  .mood-idle .float { animation: nim-float 4.4s ease-in-out infinite; }
  .mood-idle .ground { animation: nim-shadow 4.4s ease-in-out infinite; }
  .mood-idle .eyes { animation: nim-blink 5.2s ease-in-out infinite; }
  .mood-working .float { animation: nim-float 1.8s ease-in-out infinite; }
  .mood-working .ground { animation: nim-shadow 1.8s ease-in-out infinite; }
  /* 重试：每次重新挂载（又失败了一次）身子一颤，雨一直下 */
  .mood-retrying .float { animation: nim-jolt 0.7s cubic-bezier(0.3, 1.6, 0.5, 1) both; }
  .mood-retrying .drop { animation: nim-rain 1.1s ease-in infinite both; }
  .mood-retrying .badge { animation: nim-pop 0.45s cubic-bezier(0.3, 1.6, 0.5, 1) both; }
  .mood-rescued .sun { animation: nim-sunrise 1.4s cubic-bezier(0.3, 1.4, 0.5, 1) both; }
  .mood-rescued .float { animation: nim-hop 1.2s both; }
  .mood-rescued .spark { animation: nim-twinkle 1.5s ease-out both; }
  .mood-worried .float { animation: nim-shiver 3s ease-in-out infinite; }
  .mood-worried .bolt { animation: nim-flash 2.4s ease-in-out infinite; }
  .mood-sleeping .float { animation: nim-breathe 4.6s ease-in-out infinite; }
  .mood-sleeping .z { animation: nim-z 3.4s ease-out infinite both; }

  @keyframes nim-float { 0%, 100% { transform: translateY(0); } 50% { transform: translateY(-3px); } }
  @keyframes nim-shadow { 0%, 100% { transform: scaleX(1); } 50% { transform: scaleX(0.88); } }
  @keyframes nim-blink { 0%, 93%, 100% { transform: scaleY(1); } 96% { transform: scaleY(0.12); } }
  @keyframes nim-jolt { 0% { transform: translateX(-4px) rotate(-3deg); } 40% { transform: translateX(3px) rotate(2deg); } 100% { transform: none; } }
  @keyframes nim-rain { 0% { opacity: 0; transform: translateY(-4px); } 25% { opacity: 1; } 100% { opacity: 0; transform: translateY(12px); } }
  @keyframes nim-pop { 0% { transform: scale(0.3); } 100% { transform: scale(1); } }
  @keyframes nim-sunrise { 0% { opacity: 0; transform: translate(-10px, 14px) scale(0.5) rotate(-40deg); } 100% { opacity: 1; transform: none; } }
  @keyframes nim-hop { 0%, 100% { transform: translateY(0); } 40% { transform: translateY(-9px); } 70% { transform: translateY(1px); } }
  @keyframes nim-twinkle { 0% { opacity: 0; transform: scale(0); } 35% { opacity: 1; transform: scale(1.2); } 100% { opacity: 0; transform: scale(0.5) rotate(40deg); } }
  @keyframes nim-shiver { 0%, 100% { transform: translateX(0); } 20% { transform: translateX(-1.5px); } 40% { transform: translateX(1.5px); } 60% { transform: translateX(-1px); } }
  @keyframes nim-flash { 0%, 70%, 100% { opacity: 1; } 78% { opacity: 0.25; } 86% { opacity: 1; } }
  @keyframes nim-breathe { 0%, 100% { transform: scale(1, 1); } 50% { transform: scale(1.025, 0.975); } }
  @keyframes nim-z { 0% { opacity: 0; transform: translate(-3px, 5px) scale(0.6); } 30%, 75% { opacity: 1; } 100% { opacity: 0; transform: translate(3px, -7px) scale(1); } }

  /* 错开的出场时刻写成类而不是内联 style：界面的 CSP 不允许内联样式属性 */
  .fx .d-12 { animation-delay: 0.12s; }
  .fx .d-15 { animation-delay: 0.15s; }
  .fx .d-24 { animation-delay: 0.24s; }
  .fx .d-45 { animation-delay: 0.45s; }
  .fx .d-60 { animation-delay: 0.6s; }
  .fx .d-140 { animation-delay: 1.4s; }
`;

export default {
  id: 'yun.nimbus',
  name: { en: 'Nimbus', 'zh-CN': '小云' },
  description: {
    en: 'A little cloud that rains while requests retry.',
    'zh-CN': '一朵小云：请求重试时下雨，救回时出太阳。',
  },
  css,
  render,
  icon,
};

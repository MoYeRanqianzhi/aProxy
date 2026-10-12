/*
 * 默认吉祥物：不倒翁团子。
 *
 * 圆底、上小下大——怎么推都会弹回直立，正好是 aProxy 的本职：上游出错时无限重试，客户端那边不中断。
 * 头顶一撮卷成 ↻ 的呆毛是品牌记号；身体用主色，肚皮浅一档，1.5px 主色深色调描边，脚下一团柔和的落地阴影。
 *
 * 结构：颜色全部写成属性（同一套画法既能活在界面里，也能直接导出成图标文件），
 * 动画只写在 CSS 里、只改 transform 与 opacity。部件分组，从外到内：
 *   ground  落地阴影（不随身体摆动，跳起时缩小）
 *   hop     上下跳与挤压，原点在着地点
 *   lean    静态姿势（担心时歪着、睡觉时斜靠），写在 transform 属性里，减少动效时也保留
 *   brace   持续的小幅摇摆
 *   rock    一次性的「被撞歪 → 弹簧回正」，转轴是底部圆弧的圆心 (60,77)：绕它转时底边始终贴着地面滚，
 *           这就是不倒翁的物理，所以无论歪多少都不会穿到地面以下，也永远不会躺下
 *   curl / body / belly / cheeks / face / sweat  身体上的部件
 *   fx      星星、Zzz、撞击线、角标：不跟着身体转
 *
 * 一次性的动画（被撞、撒星星）依赖宿主「状态变化才重新挂载」的约定：重试次数每加一，标记里的数字变了，
 * 宿主替换 DOM，撞击动画就从头播一次——所以它被撞的节奏就是真实的重试节奏。
 */
import type { MascotDefinition, MascotIconOptions, MascotMood, MascotPalette, MascotState } from './types';

// 身体：下面是圆心 (60,77)、半径 37 的大半个圆——不倒翁的圆底；上面两段三次贝塞尔收成一个圆润的尖顶，
// 在圆的 35° 处相切接上，所以轮廓上没有折角。宽 74、高 78，矮胖、上小下大
const BODY = 'M60 36C81 36 82.55 44.74 90.3 55.8A37 37 0 1 1 29.7 55.8C37.45 44.74 39 36 60 36Z';
// 呆毛：以发根为原点的局部坐标。先往上长，再顺时针绕 200° 卷过顶端，末端的箭头朝下——就是 ↻。
// 只绕 200° 是刻意的：绕满一圈会在发根处合拢，看起来像挂件的吊环；下半圈敞开才读得出「箭头」。
// 发根放在头顶偏左，整撮毛才居中；居正中的话像一根果柄
const CURL = 'M0 0Q-1.2 -6 0 -12A7 7 0 1 1 13.58 -9.61';
const CURL_TIP = 'M12.28 -6.04L16.99 -9.01L10.59 -11.33Z';
const CURL_ROOT = { x: 52, y: 41 };

/** 状态点的颜色：托盘与标题栏图标靠它表达状态。空闲不加点，安安静静 */
function moodColor(p: MascotPalette, mood: MascotMood): string | null {
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

/** 四角星：庆祝时撒出来的那种 */
function sparkle(r: number): string {
  const k = r * 0.22;
  return `M0 ${-r}C${k} ${-k} ${k} ${-k} ${r} 0C${k} ${k} ${k} ${k} 0 ${r}C${-k} ${k} ${-k} ${k} ${-r} 0C${-k} ${-k} ${-k} ${-k} 0 ${-r}Z`;
}

function face(mood: MascotMood, p: MascotPalette): string {
  const ink = p.ink;
  const line = (d: string, w = 2.4) =>
    `<path d="${d}" fill="none" stroke="${ink}" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round"/>`;
  // 豆豆眼：深色椭圆 + 一粒白高光，高光让它「看着你」
  const beans = (ry: number, cy = 68) =>
    `<g class="eyes"><ellipse cx="48" cy="${cy}" rx="3.8" ry="${ry}" fill="${ink}"/><ellipse cx="72" cy="${cy}" rx="3.8" ry="${ry}" fill="${ink}"/>` +
    `<circle cx="49.5" cy="${cy - ry * 0.4}" r="1.4" fill="#fff"/><circle cx="73.5" cy="${cy - ry * 0.4}" r="1.4" fill="#fff"/></g>`;

  switch (mood) {
    case 'working':
      // 开心眼 ^ ^ 与张嘴笑，舌头用腮红色
      return (
        line('M44 69.5Q48 63.5 52 69.5', 2.6) +
        line('M68 69.5Q72 63.5 76 69.5', 2.6) +
        `<path d="M55 74.5Q60 82.5 65 74.5Z" fill="${ink}"/>` +
        `<path d="M57.4 78.3Q60 76.7 62.6 78.3Q60 80.8 57.4 78.3Z" fill="${p.blush}"/>`
      );
    case 'retrying':
      // 坚定：眼睛略眯、眉毛压向中间、嘴抿成一条线
      return (
        beans(4.2, 69) +
        line('M42.5 61L52 63.6', 2.5) +
        line('M77.5 61L68 63.6', 2.5) +
        line('M55.5 76.6Q60 74.8 64.5 76.6', 2.3)
      );
    case 'rescued':
      // 笑到眯眼、大张嘴
      return (
        line('M43.5 69Q48 62.5 52.5 69', 2.8) +
        line('M67.5 69Q72 62.5 76.5 69', 2.8) +
        `<path d="M53.6 73.6Q60 85.5 66.4 73.6Z" fill="${ink}"/>` +
        `<path d="M56.6 79Q60 77 63.4 79Q60 82.6 56.6 79Z" fill="${p.blush}"/>`
      );
    case 'worried':
      // 担心：眉头上扬成「八」字、波浪嘴
      return (
        beans(4.1, 69) +
        line('M43 62.8L51.5 59.8', 2.4) +
        line('M77 62.8L68.5 59.8', 2.4) +
        line('M54.5 77Q57.25 74.4 60 77Q62.75 79.6 65.5 77', 2.2)
      );
    case 'sleeping':
      // 闭眼的弧线向下弯，嘴是一个小小的 o
      return (
        line('M44 68.5Q48 72.2 52 68.5', 2.4) +
        line('M68 68.5Q72 72.2 76 68.5', 2.4) +
        `<ellipse cx="60" cy="76.8" rx="2.1" ry="1.7" fill="${ink}"/>`
      );
    default:
      return beans(4.8) + line('M56 75.5Q60 79 64 75.5', 2.2);
  }
}

/** 汗滴：重试时挂在右边，担心时挂在左边 */
function sweat(p: MascotPalette, side: 1 | -1): string {
  const x = 60 + side * 26.5;
  return (
    `<g class="sweat"><path d="M${x} 46C${x - 2.5} 50 ${x - 3.9} 52.2 ${x - 3.9} 54.2A3.9 3.9 0 0 0 ${x + 3.9} 54.2C${x + 3.9} 52.2 ${x + 2.5} 50 ${x} 46Z" ` +
    `fill="${p.retry}" stroke="${p.primaryDeep}" stroke-width="1" stroke-linejoin="round"/>` +
    `<ellipse cx="${x - 1.4}" cy="53.6" rx="1" ry="1.5" fill="#fff" opacity=".8"/></g>`
  );
}

/**
 * 呆毛画成和身体一样的「主色 + 深色描边」：外面一道粗的深色描边、里面一道细的主色。
 * 单色深色的呆毛在深色背景上会消失，单色浅色的在浅色背景上会消失；双色在哪种底上都有轮廓。
 */
function curl(p: MascotPalette, mood: MascotMood): string {
  // 睡觉时呆毛耷拉到一边，担心时也有点蔫；其它时候竖着
  const droop = mood === 'sleeping' ? -55 : mood === 'worried' ? -14 : 0;
  const round = 'stroke-linecap="round" stroke-linejoin="round"';
  return (
    `<g class="curl"><g transform="translate(${CURL_ROOT.x} ${CURL_ROOT.y}) rotate(${droop})">` +
    `<path d="${CURL}" fill="none" stroke="${p.primaryDeep}" stroke-width="5.6" ${round}/>` +
    `<path d="${CURL_TIP}" fill="${p.primaryDeep}" stroke="${p.primaryDeep}" stroke-width="3.4" ${round}/>` +
    `<path d="${CURL}" fill="none" stroke="${p.primary}" stroke-width="3" ${round}/>` +
    `<path d="${CURL_TIP}" fill="${p.primary}" stroke="${p.primary}" stroke-width=".8" ${round}/>` +
    `</g></g>`
  );
}

function body(p: MascotPalette, mood: MascotMood, nonScaling: boolean): string {
  const ns = nonScaling ? ' vector-effect="non-scaling-stroke"' : '';
  return (
    curl(p, mood) +
    `<path d="${BODY}" fill="${p.primary}" stroke="${p.primaryDeep}" stroke-width="1.5"${ns} stroke-linejoin="round"/>` +
    `<ellipse cx="60" cy="97" rx="22" ry="13" fill="${p.primarySoft}"/>` +
    `<ellipse cx="37.5" cy="75.5" rx="6" ry="3.6" fill="${p.blush}" opacity=".8"/>` +
    `<ellipse cx="82.5" cy="75.5" rx="6" ry="3.6" fill="${p.blush}" opacity=".8"/>` +
    `<g class="face">${face(mood, p)}</g>`
  );
}

/** 落地阴影：三层同心椭圆叠出柔和的边缘，不用滤镜也不用渐变（不需要 id，放在哪都不会撞名） */
function ground(p: MascotPalette): string {
  return (
    `<g class="ground">` +
    `<ellipse cx="60" cy="115.4" rx="32" ry="4.2" fill="${p.shadow}" opacity=".07"/>` +
    `<ellipse cx="60" cy="115.4" rx="25" ry="3.2" fill="${p.shadow}" opacity=".08"/>` +
    `<ellipse cx="60" cy="115.4" rx="17" ry="2.2" fill="${p.shadow}" opacity=".1"/>` +
    `</g>`
  );
}

function render(s: MascotState): string {
  if (s.size === 'icon') return icon({ px: 32, mood: s.mood, theme: s.theme, palette: s.palette });
  const p = s.palette;
  const hero = s.size === 'hero';
  const lean = s.mood === 'worried' ? -9 : s.mood === 'sleeping' ? 5 : 0;
  // 飘在身体外面的线条（Zzz、撞击线）直接画在背景上，深色主题里要换成浅色才看得见
  const fxInk = s.theme === 'dark' ? p.primarySoft : p.primaryDeep;

  let onBody = '';
  if (s.mood === 'retrying') onBody += sweat(p, 1);
  if (s.mood === 'worried') onBody += sweat(p, -1);

  let fx = '';
  if (s.mood === 'retrying') {
    // 撞击线：从左边被撞了一下（只在大图里画，小图里看不清反而像噪点）
    if (hero) {
      fx +=
        `<g class="impact" stroke="${fxInk}" stroke-width="2.4" stroke-linecap="round">` +
        `<path d="M10 62L17 65.5"/><path d="M7 77H15.5"/><path d="M10 92L17 88.5"/></g>`;
    }
    if (hero && s.retries > 0) {
      const label = `×${s.retries}`;
      const w = 15 + label.length * 7.4;
      fx +=
        `<g class="badge"><rect x="${110 - w}" y="9" width="${w}" height="20" rx="10" fill="${p.retry}" stroke="${p.surface}" stroke-width="2.5"/>` +
        `<text x="${110 - w / 2}" y="23.4" text-anchor="middle" font-size="13" font-weight="700" fill="${p.ink}">${label}</text></g>`;
    }
  }
  if (s.mood === 'worried') {
    // 「!」气泡：出错是要被看见的状态，用出错色
    const r = hero ? 9 : 12;
    fx +=
      `<g class="alert"><circle cx="96" cy="28" r="${r}" fill="${p.error}" stroke="${p.surface}" stroke-width="2.5"/>` +
      `<rect x="${96 - r * 0.15}" y="${28 - r * 0.55}" width="${r * 0.3}" height="${r * 0.62}" rx="${r * 0.15}" fill="${p.ink}"/>` +
      `<circle cx="96" cy="${28 + r * 0.42}" r="${r * 0.17}" fill="${p.ink}"/></g>`;
  }
  if (s.mood === 'rescued') {
    const stars = hero
      ? [
          [18, 46, 7, p.star, 0],
          [102, 42, 8.5, p.star, 0.1],
          [11, 86, 5, p.blush, 0.22],
          [109, 84, 6, p.blush, 0.16],
          [86, 16, 5.5, p.star, 0.3],
        ]
      : [
          [16, 46, 9, p.star, 0],
          [104, 42, 10, p.star, 0.12],
          [88, 14, 8, p.blush, 0.24],
        ];
    fx += stars
      .map(
        ([x, y, r, c, d]) =>
          `<g transform="translate(${x} ${y})"><path class="star d-${Math.round((d as number) * 100)}" d="${sparkle(r as number)}" fill="${c}" stroke="${p.primaryDeep}" stroke-width=".8"/></g>`,
      )
      .join('');
  }
  if (s.mood === 'sleeping') {
    const zs = hero
      ? [
          [84, 42, 6, 0],
          [94, 29, 8, 1.2],
          [104, 14, 10, 2.4],
        ]
      : [
          [86, 34, 10, 0],
          [100, 12, 13, 1.6],
        ];
    fx += zs
      .map(
        ([x, y, k, d]) =>
          `<g transform="translate(${x} ${y})"><path class="z d-${Math.round((d as number) * 100)}" d="M0 0H${k}L0 ${k}H${k}" fill="none" stroke="${fxInk}" stroke-width="${hero ? 2.2 : 3}" stroke-linecap="round" stroke-linejoin="round"/></g>`,
      )
      .join('');
  }

  return (
    `<svg class="tb mood-${s.mood}${s.reducedMotion ? ' still' : ''}" viewBox="0 0 120 120" aria-hidden="true">` +
    ground(p) +
    `<g class="hop"><g class="lean" transform="rotate(${lean} 60 77)"><g class="brace"><g class="rock">` +
    body(p, s.mood, true) +
    onBody +
    `</g></g></g></g>` +
    `<g class="fx">${fx}</g>` +
    `</svg>`
  );
}

/*
 * 图标：分三档细节，各自按目标像素设计，而不是把大图硬缩小——
 *   < 20px（托盘 16）：16 格网格，只留轮廓、粗呆毛、两粒眼睛与状态点
 *   20–63px（标题栏 22、托盘 24/32）：32 格网格，加上肚皮、腮红、呆毛的箭头
 *   ≥ 64px（软件图标 256）：完整形象的静态版
 * 输出是带 xmlns 的独立 SVG，颜色全是属性，可以直接存成文件交给图标工具链。
 */
function icon(o: MascotIconOptions): string {
  const p = o.palette;
  const mood = o.mood ?? 'idle';
  const dotColor = o.mood ? moodColor(p, o.mood) : null;
  const open = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="VB" width="${o.px}" height="${o.px}">`;
  // 小图标里呆毛只有一两个像素粗，画不了双色，只能按身后底色的深浅二选一：深底用浅的主色，浅底用深色调
  const tuft = o.theme === 'dark' ? p.primary : p.primaryDeep;

  if (o.px >= 64) {
    const dot = dotColor
      ? `<circle cx="96" cy="100" r="15" fill="${p.surface}"/><circle cx="96" cy="100" r="11" fill="${dotColor}"/>`
      : '';
    // 大图标没有落地阴影：图标不站在地上。视窗上下各收一点，让身体更饱满地占满方框
    return open.replace('VB', '10 16 100 100') + body(p, mood, false) + dot + `</svg>`;
  }

  if (o.px >= 20) {
    const sw = Math.max(0.8, (1.15 * 32) / o.px);
    const eyes =
      mood === 'sleeping'
        ? `<path d="M11.3 18.6Q12.8 19.8 14.3 18.6M17.7 18.6Q19.2 19.8 20.7 18.6" fill="none" stroke="${p.ink}" stroke-width="1.2" stroke-linecap="round"/>`
        : `<ellipse cx="12.8" cy="18.4" rx="1.35" ry="1.7" fill="${p.ink}"/><ellipse cx="19.2" cy="18.4" rx="1.35" ry="1.7" fill="${p.ink}"/>`;
    const dot = dotColor
      ? `<circle cx="25.4" cy="25.4" r="6.6" fill="${p.surface}"/><circle cx="25.4" cy="25.4" r="4.8" fill="${dotColor}"/>`
      : '';
    return (
      open.replace('VB', '0 0 32 32') +
      `<g transform="translate(12.8 11.2) scale(.42)"><path d="${CURL}" fill="none" stroke="${tuft}" stroke-width="4.6" stroke-linecap="round" stroke-linejoin="round"/>` +
      `<path d="${CURL_TIP}" fill="${tuft}" stroke="${tuft}" stroke-width="2.4" stroke-linejoin="round"/></g>` +
      `<path d="M16 9.6C21 9.6 22.28 12 24.35 14.95A10.2 10.2 0 1 1 7.65 14.95C9.72 12 11 9.6 16 9.6Z" fill="${p.primary}" stroke="${p.primaryDeep}" stroke-width="${sw}" stroke-linejoin="round"/>` +
      `<ellipse cx="16" cy="25.8" rx="6" ry="3.6" fill="${p.primarySoft}"/>` +
      `<ellipse cx="10.2" cy="21.4" rx="1.7" ry="1" fill="${p.blush}"/><ellipse cx="21.8" cy="21.4" rx="1.7" ry="1" fill="${p.blush}"/>` +
      eyes +
      dot +
      `</svg>`
    );
  }

  // 16 格：每个部件都对齐到整像素附近，缩到 16px 时边缘不糊成一片
  const eyes16 =
    mood === 'sleeping'
      ? `<path d="M5.2 9.6H7M9 9.6H10.8" stroke="${p.ink}" stroke-width="1.1" stroke-linecap="round"/>`
      : `<rect x="5.45" y="8.3" width="1.3" height="2" rx=".6" fill="${p.ink}"/><rect x="9.25" y="8.3" width="1.3" height="2" rx=".6" fill="${p.ink}"/>`;
  const dot16 = dotColor
    ? `<circle cx="12.6" cy="12.6" r="3.9" fill="${p.surface}"/><circle cx="12.6" cy="12.6" r="2.7" fill="${dotColor}"/>`
    : '';
  return (
    open.replace('VB', '0 0 16 16') +
    `<path d="M6.9 5.6Q6.7 4.4 6.9 3.2A1.7 1.7 0 1 1 10.2 3.78" fill="none" stroke="${tuft}" stroke-width="1.3" stroke-linecap="round"/>` +
    `<path d="M8 4.6C10.6 4.6 11.33 5.74 12.42 7.3A5.4 5.4 0 1 1 3.58 7.3C4.67 5.74 5.4 4.6 8 4.6Z" fill="${p.primary}" stroke="${p.primaryDeep}" stroke-width="1" stroke-linejoin="round"/>` +
    eyes16 +
    dot16 +
    `</svg>`
  );
}

const css = /* css */ `
  svg { display: block; width: 100%; height: 100%; overflow: visible; }
  .hop { transform-box: view-box; transform-origin: 60px 114px; }
  .brace, .rock { transform-box: view-box; transform-origin: 60px 77px; }
  .ground { transform-box: view-box; transform-origin: 60px 115.4px; }
  .curl { transform-box: view-box; transform-origin: 52px 41px; }
  .eyes, .star, .z, .badge, .alert { transform-box: fill-box; transform-origin: center; }
  /* 减少动效：只停掉动画，静态姿势（歪着、斜靠）和表情照常 */
  .still * { animation: none !important; }

  /* 空闲：很慢地左右摇，偶尔眨眼 */
  .mood-idle .brace { animation: tb-sway 5s ease-in-out infinite; }
  .mood-idle .eyes { animation: tb-blink 5.6s ease-in-out infinite; }
  @keyframes tb-sway { 0%, 100% { transform: rotate(-2.2deg); } 50% { transform: rotate(2.2deg); } }
  @keyframes tb-blink { 0%, 92%, 100% { transform: scaleY(1); } 95% { transform: scaleY(0.1); } }

  /* 忙碌：轻轻地蹦，落地时压扁一下；呆毛慢半拍 */
  .mood-working .hop { animation: tb-hop 1.2s infinite; }
  .mood-working .ground { animation: tb-ground 1.2s infinite; }
  .mood-working .curl { animation: tb-curl-hop 1.2s ease-in-out infinite; }
  @keyframes tb-hop {
    0% { transform: translateY(0) scale(1, 1); animation-timing-function: ease-out; }
    10% { transform: translateY(0) scale(1.05, 0.95); animation-timing-function: cubic-bezier(0.2, 0.7, 0.4, 1); }
    42% { transform: translateY(-7px) scale(0.98, 1.03); animation-timing-function: cubic-bezier(0.6, 0, 0.8, 0.4); }
    70% { transform: translateY(0) scale(1.05, 0.95); animation-timing-function: ease-out; }
    84%, 100% { transform: translateY(0) scale(1, 1); }
  }
  @keyframes tb-ground { 0%, 10%, 70%, 100% { transform: scale(1); opacity: 1; } 42% { transform: scale(0.8); opacity: 0.6; } }
  @keyframes tb-curl-hop { 0%, 100% { transform: rotate(0); } 30% { transform: rotate(-7deg); } 60% { transform: rotate(5deg); } }

  /* 重试：每次重新挂载（= 又重试了一次）被从左边撞歪，弹簧似地左右回摆几下回正；
     之后是很小的「顶住」摇摆，直到下一次重试 */
  .mood-retrying .rock { animation: tb-bump 1.8s both; }
  .mood-retrying .brace { animation: tb-brace 3s ease-in-out 1.8s infinite; }
  .mood-retrying .curl { animation: tb-curl-lag 1.8s both; }
  .mood-retrying .impact { opacity: 0; animation: tb-impact 0.55s ease-out both; }
  .mood-retrying .badge { animation: tb-pop 0.5s cubic-bezier(0.3, 1.6, 0.5, 1) 0.1s both; }
  .mood-retrying .sweat { animation: tb-sweat 2.4s ease-in 0.4s infinite both; }
  @keyframes tb-bump {
    0% { transform: translateX(0) rotate(0); animation-timing-function: cubic-bezier(0.1, 0.8, 0.3, 1); }
    8% { transform: translateX(3px) rotate(16deg); animation-timing-function: ease-in-out; }
    22% { transform: translateX(-2px) rotate(-10deg); animation-timing-function: ease-in-out; }
    36% { transform: translateX(1.2px) rotate(6deg); animation-timing-function: ease-in-out; }
    50% { transform: translateX(-0.6px) rotate(-3.4deg); animation-timing-function: ease-in-out; }
    63% { transform: translateX(0) rotate(1.8deg); animation-timing-function: ease-in-out; }
    76% { transform: rotate(-0.8deg); animation-timing-function: ease-in-out; }
    88% { transform: rotate(0.3deg); animation-timing-function: ease-in-out; }
    100% { transform: rotate(0); }
  }
  @keyframes tb-brace { 0%, 100% { transform: rotate(0); } 50% { transform: rotate(1.6deg); } }
  @keyframes tb-curl-lag {
    0% { transform: rotate(0); } 10% { transform: rotate(-18deg); } 24% { transform: rotate(12deg); }
    38% { transform: rotate(-7deg); } 52% { transform: rotate(4deg); } 66% { transform: rotate(-2deg); } 80%, 100% { transform: rotate(0); }
  }
  @keyframes tb-impact { 0% { opacity: 0; transform: translateX(5px); } 20% { opacity: 1; } 100% { opacity: 0; transform: translateX(-4px); } }
  @keyframes tb-pop { 0% { transform: scale(0.3); } 100% { transform: scale(1); } }
  @keyframes tb-sweat {
    0% { opacity: 0; transform: translateY(-3px); } 20% { opacity: 1; }
    75% { opacity: 1; transform: translateY(4px); } 100% { opacity: 0; transform: translateY(7px); }
  }

  /* 救回：蓄力、跳起、落地压扁，四周撒星星——只播一次，之后笑着站好 */
  .mood-rescued .hop { animation: tb-leap 1.4s both; }
  .mood-rescued .ground { animation: tb-ground-leap 1.4s both; }
  .mood-rescued .curl { animation: tb-curl-hop 0.7s ease-in-out 0.45s 2 both; }
  .mood-rescued .star { animation: tb-twinkle 1.7s ease-out both; }
  @keyframes tb-leap {
    0% { transform: translateY(0) scale(1, 1); animation-timing-function: ease-out; }
    14% { transform: translateY(0) scale(1.08, 0.9); animation-timing-function: cubic-bezier(0.2, 0.8, 0.4, 1); }
    44% { transform: translateY(-15px) scale(0.95, 1.06); animation-timing-function: cubic-bezier(0.6, 0, 0.9, 0.5); }
    70% { transform: translateY(0) scale(1.08, 0.92); animation-timing-function: ease-out; }
    84% { transform: translateY(0) scale(0.98, 1.02); }
    100% { transform: translateY(0) scale(1, 1); }
  }
  @keyframes tb-ground-leap { 0%, 14%, 70%, 100% { transform: scale(1); opacity: 1; } 44% { transform: scale(0.7); opacity: 0.5; } }
  @keyframes tb-twinkle {
    0% { opacity: 0; transform: scale(0) rotate(-40deg); }
    30% { opacity: 1; transform: scale(1.2) rotate(0); }
    65% { opacity: 1; transform: scale(1) rotate(10deg); }
    100% { opacity: 0; transform: scale(0.4) rotate(30deg); }
  }

  /* 担心：歪着（静态姿势在属性里），一直想自己扶正又扶不正，但绝不倒下 */
  .mood-worried .brace { animation: tb-wobble 2.8s ease-in-out infinite; }
  .mood-worried .sweat { animation: tb-sweat 2.8s ease-in infinite both; }
  .mood-worried .alert { animation: tb-alert 2.8s ease-in-out infinite; }
  @keyframes tb-wobble { 0%, 100% { transform: rotate(0); } 50% { transform: rotate(2.8deg); } }
  @keyframes tb-alert { 0%, 100% { transform: translateY(0); } 50% { transform: translateY(-2px); } }

  /* 睡觉：慢慢呼吸，Zzz 一个接一个往上飘 */
  .mood-sleeping .hop { animation: tb-breathe 4s ease-in-out infinite; }
  .mood-sleeping .z { animation: tb-z 3.6s ease-out infinite both; }
  @keyframes tb-breathe { 0%, 100% { transform: scale(1, 1); } 50% { transform: scale(1.012, 1.03); } }
  @keyframes tb-z {
    0% { opacity: 0; transform: translate(-4px, 6px) scale(0.6); } 25% { opacity: 1; }
    80% { opacity: 1; } 100% { opacity: 0; transform: translate(4px, -8px) scale(1.05); }
  }

  /* 错开的出场时刻写成类而不是内联 style：界面的 CSP 不允许内联样式属性 */
  .fx .d-10 { animation-delay: 0.1s; }
  .fx .d-12 { animation-delay: 0.12s; }
  .fx .d-16 { animation-delay: 0.16s; }
  .fx .d-22 { animation-delay: 0.22s; }
  .fx .d-24 { animation-delay: 0.24s; }
  .fx .d-30 { animation-delay: 0.3s; }
  .fx .d-120 { animation-delay: 1.2s; }
  .fx .d-160 { animation-delay: 1.6s; }
  .fx .d-240 { animation-delay: 2.4s; }
`;

export const tumbler: MascotDefinition = {
  id: 'tumbler',
  name: { en: 'Tumbler', 'zh-CN': '不倒翁' },
  description: {
    en: 'Knock it over and it rocks right back up — the default.',
    'zh-CN': '怎么推都会弹回直立。默认形象。',
  },
  css,
  render,
  icon,
};

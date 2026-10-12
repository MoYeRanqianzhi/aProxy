/*
 * 主题字符串：一段可以复制分享的文字，形如 aproxy-theme-v1:{"name":"抹茶",...}。
 *
 * 主题只改 CSS 变量，不碰组件结构：这里把主题解析、校验后生成一张只有自定义属性声明的样式表，
 * 挂在 document 上（constructable stylesheet：经 CSSOM 写入，不是内联 <style>，不受 CSP 的内联样式限制）。
 * 变量名就是 app.css 里的令牌，所以组件与将来的扩展都不必知道主题的存在。
 *
 * 颜色怎么取：作者给的颜色只决定色调（色相与鲜艳程度），深浅按界面自己的台阶取——主色块、浅底、
 * 深色字各在固定的亮度上，和内置主题一模一样。这样任何主题下「深色字配浅底」「墨紫字配彩色块」的对比度
 * 都和内置主题一样满足 WCAG AA，不会因为作者挑了一个很深的主色，就让按钮上的字看不清。
 * 这和 Material You 的动态取色是一个思路：种子色给色调，每个角色在固定的色阶上取色。
 * 计算交给浏览器的相对颜色语法 oklch(from <种子> L C H)，所以这里不需要任何色彩空间换算。
 *
 * 字符串是外部输入（从别处粘贴来的），所以严格校验：颜色只认 #rgb、#rrggbb 与 oklch(L C H)，数字有范围，
 * 字体只能从界面自带的几种里选。校验通过的值才会拼进样式表，不存在注入。不认识的字段直接忽略，给以后的版本留余地。
 */
import type { ThemeColors, ThemeSpec } from './types';

export const THEME_PREFIX = 'aproxy-theme-v1:';

type ColorRole = keyof ThemeColors;
type Variant = 'light' | 'dark';

const ROLES: readonly ColorRole[] = ['primary', 'text', 'surface', 'ok', 'retry', 'error', 'warn'];
const FONTS: readonly NonNullable<ThemeSpec['font']>[] = ['rounded', 'system', 'mono'];

/**
 * 每个颜色角色派生出的令牌，以及它们在内置主题里的亮度 l 与色度 c（照抄 app.css）。
 * seed 是这个角色的种子在内置主题里的色度：作者种子的色度按 c/seed 的比例缩放每一级的色度，
 * 所以灰一点的种子得到整体灰一点的一族颜色；比例封顶 1.6 倍，免得超出屏幕色域。
 * dh 是个别台阶相对种子的色相偏移（黄压深时要偏向琥珀才不发绿，天蓝压深时略偏蓝），也照抄内置主题。
 * vivid：彩色族跟随「配色强度」缩放色度；文字与表面不跟。
 */
interface Step {
  token: string;
  l: number;
  c: number;
  dh?: number;
}
interface Family {
  seed: number;
  vivid: boolean;
  steps: Step[];
}

const LADDER: Record<Variant, Record<ColorRole, Family>> = {
  light: {
    primary: {
      seed: 0.13,
      vivid: true,
      steps: [
        { token: '--lilac', l: 0.78, c: 0.13 },
        { token: '--lilac-soft', l: 0.95, c: 0.04 },
        { token: '--lilac-deep', l: 0.47, c: 0.14 },
        { token: '--focus', l: 0.55, c: 0.17 },
      ],
    },
    text: {
      seed: 0.07,
      vivid: false,
      steps: [
        { token: '--text', l: 0.27, c: 0.07 },
        { token: '--text-2', l: 0.44, c: 0.05 },
        { token: '--text-3', l: 0.53, c: 0.045 },
        { token: '--on-fill', l: 0.25, c: 0.07 },
      ],
    },
    surface: {
      seed: 0.016,
      vivid: false,
      // 浅色的卡片（--card）是纯白，不随表面色调变：白卡片浮在带色调的纸面上，层次才清楚
      steps: [
        { token: '--paper', l: 0.965, c: 0.016 },
        { token: '--sheet', l: 0.988, c: 0.007 },
        { token: '--card-2', l: 0.975, c: 0.01 },
        { token: '--hover', l: 0.93, c: 0.02 },
        { token: '--line', l: 0.885, c: 0.02 },
        { token: '--line-soft', l: 0.91, c: 0.016 },
        { token: '--line-strong', l: 0.66, c: 0.035 },
        { token: '--line-dash', l: 0.78, c: 0.03 },
      ],
    },
    ok: {
      seed: 0.14,
      vivid: true,
      steps: [
        { token: '--mint', l: 0.85, c: 0.14 },
        { token: '--mint-soft', l: 0.95, c: 0.055 },
        { token: '--mint-deep', l: 0.47, c: 0.1 },
      ],
    },
    retry: {
      seed: 0.12,
      vivid: true,
      steps: [
        { token: '--sky', l: 0.83, c: 0.12 },
        { token: '--sky-soft', l: 0.95, c: 0.04 },
        { token: '--sky-deep', l: 0.47, c: 0.11, dh: 8 },
      ],
    },
    error: {
      seed: 0.15,
      vivid: true,
      steps: [
        { token: '--coral', l: 0.76, c: 0.15 },
        { token: '--coral-soft', l: 0.95, c: 0.04 },
        { token: '--coral-deep', l: 0.52, c: 0.15 },
      ],
    },
    warn: {
      seed: 0.15,
      vivid: true,
      steps: [
        { token: '--butter', l: 0.9, c: 0.15 },
        { token: '--butter-soft', l: 0.97, c: 0.06 },
        { token: '--butter-deep', l: 0.5, c: 0.09, dh: -20 },
      ],
    },
  },
  dark: {
    primary: {
      seed: 0.12,
      vivid: true,
      steps: [
        { token: '--lilac', l: 0.75, c: 0.12 },
        { token: '--lilac-soft', l: 0.32, c: 0.06 },
        { token: '--lilac-deep', l: 0.84, c: 0.09 },
        { token: '--focus', l: 0.8, c: 0.12 },
      ],
    },
    text: {
      seed: 0.012,
      vivid: false,
      steps: [
        { token: '--text', l: 0.96, c: 0.012 },
        { token: '--text-2', l: 0.83, c: 0.03 },
        { token: '--text-3', l: 0.72, c: 0.035 },
      ],
    },
    surface: {
      seed: 0.032,
      vivid: false,
      steps: [
        { token: '--paper', l: 0.19, c: 0.032 },
        { token: '--sheet', l: 0.225, c: 0.036 },
        { token: '--card', l: 0.265, c: 0.04 },
        { token: '--card-2', l: 0.24, c: 0.038 },
        { token: '--hover', l: 0.31, c: 0.045 },
        { token: '--line', l: 0.36, c: 0.04 },
        { token: '--line-soft', l: 0.31, c: 0.035 },
        { token: '--line-strong', l: 0.56, c: 0.04 },
        { token: '--line-dash', l: 0.46, c: 0.04 },
        // 彩色块上的字在深色主题里也是深色：它属于表面这一族（深底的颜色），而不是浅色的正文
        { token: '--on-fill', l: 0.22, c: 0.06 },
      ],
    },
    ok: {
      seed: 0.12,
      vivid: true,
      steps: [
        { token: '--mint', l: 0.79, c: 0.12 },
        { token: '--mint-soft', l: 0.31, c: 0.05 },
        { token: '--mint-deep', l: 0.86, c: 0.1 },
      ],
    },
    retry: {
      seed: 0.105,
      vivid: true,
      steps: [
        { token: '--sky', l: 0.78, c: 0.105 },
        { token: '--sky-soft', l: 0.31, c: 0.05, dh: 8 },
        { token: '--sky-deep', l: 0.85, c: 0.08 },
      ],
    },
    error: {
      seed: 0.13,
      vivid: true,
      steps: [
        { token: '--coral', l: 0.73, c: 0.13 },
        { token: '--coral-soft', l: 0.31, c: 0.055 },
        { token: '--coral-deep', l: 0.83, c: 0.09 },
      ],
    },
    warn: {
      seed: 0.125,
      vivid: true,
      steps: [
        { token: '--butter', l: 0.86, c: 0.125 },
        { token: '--butter-soft', l: 0.32, c: 0.05, dh: -10 },
        { token: '--butter-deep', l: 0.88, c: 0.1, dh: -3 },
      ],
    },
  },
};

/** 更高对比度：次要文字与细边各往「远离底色」的方向挪一档，正文与彩色块不动 */
const MORE_CONTRAST: Record<Variant, Record<string, number>> = {
  light: { '--text-2': 0.38, '--text-3': 0.46, '--line': 0.82, '--line-soft': 0.87, '--line-strong': 0.56 },
  dark: { '--text-2': 0.9, '--text-3': 0.8, '--line': 0.44, '--line-soft': 0.37, '--line-strong': 0.66 },
};

/** 字体只换界面字体；等宽字体（日志、代码）始终是等宽的。rounded 就是内置字体，不输出 */
const FONT_STACKS: Record<Exclude<NonNullable<ThemeSpec['font']>, 'rounded'>, string> = {
  system: "'Segoe UI Variable Text', 'Segoe UI', 'Microsoft YaHei UI', 'PingFang SC', 'Noto Sans CJK SC', system-ui, sans-serif",
  mono: "'Maple Mono', 'Cascadia Mono', Consolas, 'Microsoft YaHei UI', 'PingFang SC', monospace",
};

/**
 * 内置主题本身也是一份主题：「复制主题字符串」在没导入过主题时导出的就是它，拿去改几个颜色就是新主题。
 * 种子写成 oklch() 而不是十六进制：内置的几个颜色略超出 sRGB，换成十六进制会被截断、色相偏几度。
 */
export const BUILTIN_THEME: ThemeSpec = {
  name: 'aProxy',
  contrast: 'standard',
  radius: 1,
  font: 'rounded',
  motion: 1,
  titlebarMin: 44,
  light: {
    primary: 'oklch(0.78 0.13 295)',
    text: 'oklch(0.27 0.07 285)',
    surface: 'oklch(0.965 0.016 295)',
    ok: 'oklch(0.85 0.14 165)',
    retry: 'oklch(0.83 0.12 232)',
    error: 'oklch(0.76 0.15 25)',
    warn: 'oklch(0.9 0.15 95)',
  },
  dark: {
    primary: 'oklch(0.75 0.12 295)',
    text: 'oklch(0.96 0.012 290)',
    surface: 'oklch(0.19 0.032 285)',
    ok: 'oklch(0.79 0.12 165)',
    retry: 'oklch(0.78 0.105 232)',
    error: 'oklch(0.73 0.13 25)',
    warn: 'oklch(0.86 0.125 95)',
  },
};

/** 设置页「填入示例」填进去的主题：和内置主题差别够大，一眼看得出主题换了什么 */
export const SAMPLE_THEME: ThemeSpec = {
  name: '抹茶 Matcha',
  author: 'aProxy',
  radius: 0.6,
  font: 'system',
  motion: 0.6,
  light: {
    primary: '#9ccc7a',
    text: '#2f3527',
    surface: '#f4f1e6',
    ok: '#4fd1b5',
    retry: '#86b8f2',
    error: '#f2908a',
    warn: '#f2cd5c',
  },
  dark: {
    primary: '#9ccc7a',
    text: '#eef1e6',
    surface: '#1b2019',
    ok: '#4fd1b5',
    retry: '#86b8f2',
    error: '#f2908a',
    warn: '#f2cd5c',
  },
};

// ---------------------------------------------------------------------------
// 解析与导出
// ---------------------------------------------------------------------------

/** 校验失败的原因，由界面翻译成一句话（i18n 键 theme.err.<key>） */
export interface ThemeError {
  key: 'empty' | 'prefix' | 'version' | 'json' | 'shape' | 'name' | 'color' | 'range' | 'enum';
  /** 出错的字段，例如 light.primary */
  field?: string;
  /** 补充：JSON 的报错、允许的取值、版本号 */
  detail?: string;
}

export type ThemeParse = { ok: true; spec: ThemeSpec } | { ok: false; error: ThemeError };

const HEX = /^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/i;
// L 可以是 0–1 的小数或百分数，C、H 是非负数，H 可以带 deg；不接受 none、calc() 与透明度
const OKLCH = /^oklch\(\s*(\d*\.?\d+)(%?)\s+(\d*\.?\d+)\s+(\d*\.?\d+)(?:deg)?\s*\)$/i;

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/** 校验并规范化一个颜色；不合格返回 null */
function color(v: unknown): string | null {
  if (typeof v !== 'string') return null;
  const s = v.trim();
  if (HEX.test(s)) return s.toLowerCase();
  const m = OKLCH.exec(s);
  if (!m) return null;
  const l = m[2] ? Number(m[1]) / 100 : Number(m[1]);
  const c = Number(m[3]);
  const h = Number(m[4]);
  if (l > 1 || c > 0.5 || h > 360) return null;
  return `oklch(${l} ${c} ${h})`;
}

export function parseTheme(text: string): ThemeParse {
  const fail = (key: ThemeError['key'], field?: string, detail?: string): ThemeParse => ({ ok: false, error: { key, field, detail } });
  const s = text.trim();
  if (!s) return fail('empty');
  const m = /^aproxy-theme-v(\d+):/.exec(s);
  if (!m) return fail('prefix');
  if (m[1] !== '1') return fail('version', undefined, `v${m[1]}`);

  let raw: unknown;
  try {
    raw = JSON.parse(s.slice(m[0].length));
  } catch (e) {
    return fail('json', undefined, (e as Error).message);
  }
  if (!isObject(raw)) return fail('shape', 'JSON');

  if (typeof raw.name !== 'string' || !raw.name.trim() || raw.name.trim().length > 40) return fail('name', 'name');
  const spec: ThemeSpec = { name: raw.name.trim() };

  if (raw.author !== undefined) {
    if (typeof raw.author !== 'string' || raw.author.trim().length > 40) return fail('shape', 'author');
    if (raw.author.trim()) spec.author = raw.author.trim();
  }
  if (raw.contrast !== undefined) {
    if (raw.contrast !== 'standard' && raw.contrast !== 'more') return fail('enum', 'contrast', 'standard | more');
    spec.contrast = raw.contrast;
  }
  for (const key of ['radius', 'motion'] as const) {
    const v = raw[key];
    if (v === undefined) continue;
    if (typeof v !== 'number' || !Number.isFinite(v) || v < 0 || v > 1.5) return fail('range', key, '0 – 1.5');
    spec[key] = Math.round(v * 100) / 100;
  }
  // 标题栏的最小高度（像素）：低于 36 搜索框就挤得没法看，高于 64 白占内容的地方
  if (raw.titlebarMin !== undefined) {
    const v = raw.titlebarMin;
    if (typeof v !== 'number' || !Number.isFinite(v) || v < 36 || v > 64) return fail('range', 'titlebarMin', '36 – 64');
    spec.titlebarMin = Math.round(v);
  }
  if (raw.font !== undefined) {
    if (!(FONTS as readonly unknown[]).includes(raw.font)) return fail('enum', 'font', FONTS.join(' | '));
    spec.font = raw.font as ThemeSpec['font'];
  }
  for (const variant of ['light', 'dark'] as const) {
    const c = raw[variant];
    if (c === undefined) continue;
    if (!isObject(c)) return fail('shape', variant);
    const out: ThemeColors = {};
    for (const role of ROLES) {
      if (c[role] === undefined) continue;
      const v = color(c[role]);
      if (!v) return fail('color', `${variant}.${role}`);
      out[role] = v;
    }
    spec[variant] = out;
  }
  return { ok: true, spec };
}

export function serializeTheme(spec: ThemeSpec): string {
  return THEME_PREFIX + JSON.stringify(spec);
}

// ---------------------------------------------------------------------------
// 生成样式表
// ---------------------------------------------------------------------------

/** 一个变体的全部颜色声明：作者没给的角色用内置主题的种子补齐，于是两种模式下的令牌总是完整的一套 */
function variantDecls(spec: ThemeSpec, variant: Variant): string[] {
  const colors = { ...BUILTIN_THEME[variant], ...spec[variant] };
  const more = spec.contrast === 'more' ? MORE_CONTRAST[variant] : {};
  const out: string[] = [];
  for (const role of ROLES) {
    const fam = LADDER[variant][role];
    const seed = colors[role]!;
    for (const st of fam.steps) {
      const l = more[st.token] ?? st.l;
      const c = `calc(${st.c} * min(c / ${fam.seed}, 1.6)${fam.vivid ? ' * var(--vivid)' : ''})`;
      const h = st.dh ? `calc(h + ${st.dh})` : 'h';
      out.push(`${st.token}: oklch(from ${seed} ${l} ${c} ${h});`);
    }
  }
  return out;
}

export function themeCss(spec: ThemeSpec): string {
  const root: string[] = [];
  if (spec.radius !== undefined) root.push(`--radius: ${spec.radius};`);
  if (spec.motion !== undefined) root.push(`--motion: ${spec.motion};`);
  if (spec.titlebarMin !== undefined) root.push(`--titlebar-min: ${spec.titlebarMin}px;`);
  if (spec.font && spec.font !== 'rounded') root.push(`--font-ui: ${FONT_STACKS[spec.font]};`);
  return [
    `:root { ${root.join(' ')} }`,
    `:root[data-variant='light'] { ${variantDecls(spec, 'light').join(' ')} }`,
    `:root[data-variant='dark'] { ${variantDecls(spec, 'dark').join(' ')} }`,
  ].join('\n');
}

let sheet: CSSStyleSheet | null = null;

/** 换上主题（null 换回内置）。同一张样式表反复改写，不会越挂越多 */
export function applyTheme(spec: ThemeSpec | null) {
  if (!sheet) {
    sheet = new CSSStyleSheet();
    // 排在文档自己的样式表之后：选择器与 app.css 相同，后出现的赢
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  }
  sheet.replaceSync(spec ? themeCss(spec) : '');
}

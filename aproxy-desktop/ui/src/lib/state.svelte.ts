/*
 * 界面状态（Svelte 5 runes）。
 *
 * 用户偏好（语言、模式、主题、Mica…）与「系统给出的事实」（系统语言、深浅色、减少动效偏好、
 * Mica 能力）分开存：偏好可以是「跟随系统」，最终生效值由 getter 推导。
 * 正式接入 Tauri 后，系统事实由 Rust 端提供（例如 Mica 是否支持由 Rust 按系统 build 判定），
 * 偏好持久化到 settings.json 的 ui 段；原型里两者都是本地假值。
 */

import type { ThemeSpec } from './types';

/** mascots 是原型专用的吉祥物预览页，不在导航里，只能从审核工具条或地址栏 #/mascots 进入 */
export type Page = 'overview' | 'instance' | 'logs' | 'config' | 'update' | 'settings' | 'mascots';
export type Lang = 'zh-CN' | 'en';
export type LangPref = 'system' | Lang;
/** 浅色/深色模式的偏好。「主题」另有所指：导入的一整套颜色、圆角、字体与动效（ThemeSpec） */
export type ModePref = 'system' | 'light' | 'dark';
export type Palette = 'soft' | 'standard' | 'vivid';
/** Mica 的系统能力：可用；系统不支持（Windows 10 或早期 Windows 11）；系统关了透明效果（Mica 会显示为纯色） */
export type MicaSupport = 'available' | 'unsupported' | 'transparency-off';
/** 原型专用的场景切换：正常数据、没有实例（空状态）、实例无响应（错误状态） */
export type Scenario = 'normal' | 'empty' | 'error';

const PAGES: Page[] = ['overview', 'instance', 'logs', 'config', 'update', 'settings', 'mascots'];

class UiState {
  page = $state<Page>('overview');
  /** 实例详情页与配置页当前看的实例 */
  instancePort = $state(12346);
  /** 日志页打开详情的那一条（key = 端口:序号）；命令面板与总览也会设置它 */
  selectedLog = $state<string | null>(null);
  /** 从别处跳到日志页时预设的实例筛选 */
  logInstanceFilter = $state<number | 'all'>('all');

  langPref = $state<LangPref>('system');
  modePref = $state<ModePref>('system');
  palette = $state<Palette>('standard');
  micaEnabled = $state(false);
  /** 选中的吉祥物 id；'none' 表示不显示（见 mascot/registry） */
  mascotId = $state('tumbler');
  /** 导入的主题，null 为内置主题。raw：整份替换，不需要深层代理 */
  themeSpec = $state.raw<ThemeSpec | null>(null);
  /** 导入前的预览：整个窗口临时换上它（可以切到别的页面看），应用或取消后清空 */
  themePreview = $state.raw<ThemeSpec | null>(null);

  // 系统事实
  systemLang = $state<Lang>('zh-CN');
  systemDark = $state(false);
  systemReducedMotion = $state(false);
  micaSupport = $state<MicaSupport>('available');

  // 原型审核工具条（scenario 只是显示用，真正的场景在 mock 后端里）
  reducedMotionOverride = $state<boolean | null>(null);
  scenario = $state<Scenario>('normal');
  toolbarOpen = $state(false);
  toolbarHidden = $state(false);

  paletteOpen = $state(false);

  get lang(): Lang {
    return this.langPref === 'system' ? this.systemLang : this.langPref;
  }

  /** 当前生效的浅色/深色变体（<html data-variant>） */
  get variant(): 'light' | 'dark' {
    if (this.modePref === 'system') return this.systemDark ? 'dark' : 'light';
    return this.modePref;
  }

  /** 窗口上正穿着的主题：预览优先；null 为内置主题 */
  get activeTheme(): ThemeSpec | null {
    return this.themePreview ?? this.themeSpec;
  }

  /** 动效强度为 0 的主题等于打开了减少动效：没有位移，只留淡入淡出 */
  get reducedMotion(): boolean {
    return this.reducedMotionOverride ?? (this.systemReducedMotion || this.activeTheme?.motion === 0);
  }

  /** <html data-mica>：只有用户开启且系统支持时才进入半透明表面 */
  get micaActive(): boolean {
    return this.micaEnabled && this.micaSupport !== 'unsupported';
  }

  go(page: Page, port?: number) {
    if (port !== undefined) this.instancePort = port;
    this.page = page;
  }
}

export const ui = new UiState();

// 启动时先按地址栏 hash 定位页面，必须早于「页面 → hash」的同步，否则会被覆盖回总览
{
  const initial = parseHash(location.hash);
  if (initial) ui.go(initial.page, initial.port);
}

/** 把 hash（#/logs、#/instance/12346）与页面互相同步，方便直接打开某页审核与截图 */
export function parseHash(hash: string): { page: Page; port?: number } | null {
  const parts = hash.replace(/^#\/?/, '').split('/').filter(Boolean);
  const page = parts[0] as Page | undefined;
  if (!page || !PAGES.includes(page)) return null;
  const port = parts[1] ? Number(parts[1]) : undefined;
  return { page, port: Number.isFinite(port) ? port : undefined };
}

export function pageHash(page: Page, port: number): string {
  return page === 'instance' || page === 'config' ? `#/${page}/${port}` : `#/${page}`;
}

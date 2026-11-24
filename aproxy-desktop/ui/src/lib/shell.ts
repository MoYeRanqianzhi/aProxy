/*
 * 窗口外壳：只有在 Tauri 窗口里才有的那一层——显示窗口、自定义标题栏的尺寸与颜色、Mica、窗口的浅色/深色。
 *
 * 组件不直接碰 Tauri。App 启动时调用 startShell；界面的外观状态（深浅色、配色、主题、Mica 开关）变了以后，
 * App 在改完 <html> 上的属性之后调用 syncWindow，由这里转成窗口 API 的调用。浏览器里（vite dev）这些都不存在：
 * inTauri 为假，两个函数什么也不做，原型件（proto/）在页面里模拟标题栏按钮与 Mica。
 *
 * 窗口能力（自定义标题栏装上没有、Mica 能不能用、系统版本）由 Rust 判定，经 desktop_info 命令取：
 * Tauri 打开 Mica 失败时不报错，页面自己也看不出系统支不支持（见 aproxy-desktop/src/lib.rs）。
 *
 * Mica 的做法照 tauri-windows-mica skill：窗口始终透明、默认不打开材质；页面在 data-mica="off" 时画不透明的底色，
 * "on" 时让标题栏与导航透出材质。打开时先换上材质、再让页面透明；关闭时反过来，先让页面不透明、等它画出来，
 * 再撤掉材质——顺序反了，中间那一下窗口是透明的，能看见后面的桌面。
 */
import { invoke, isTauri } from '@tauri-apps/api/core';
import { Effect, getCurrentWindow } from '@tauri-apps/api/window';
import { ui, type MicaSupport } from './state.svelte';
import { setTitlebarColor, watchTitlebarInsets } from './titlebar.js';

/** 页面是不是跑在 aProxy 的桌面窗口里（而不是开发时的浏览器） */
export const inTauri = isTauri();

/** desktop_info 命令的返回值，字段与 Rust 的 DesktopInfo 一致 */
interface DesktopInfo {
  /** Windows 自定义标题栏装上了：原生按钮留在右上角，页面画标题栏的其余部分 */
  customTitlebar: boolean;
  /** macOS 的 Overlay 标题栏：页面铺到窗口顶上画整条栏，交通灯由系统叠在左上角 */
  trafficLights: boolean;
  /** Linux 没有窗口管理器的装饰：页面画整条栏，最小化、最大化、关闭三个按钮也是页面画的 */
  ownControls: boolean;
  mica: MicaSupport;
  /** Windows 的系统 build；不是 Windows 为 null */
  windowsBuild: number | null;
}

/**
 * 页面一跑起来就调用：显示窗口，取窗口能力。
 * 窗口以 visible:false 创建，等页面能画了再显示，启动时就不会先闪一下白底；
 * show 放在最前面，后面任何一步出错都不该让窗口一直不出现（Rust 侧另有几秒的兜底）。
 */
export function startShell(): void {
  if (!inTauri) return;
  void getCurrentWindow().show();
  // Tauri 里 Mica 要等窗口真的换上材质才算开着（见 ui.micaActive）
  ui.micaWindow = false;
  invoke<DesktopInfo>('desktop_info').then(
    (info) => {
      ui.micaSupport = info.mica;
      ui.windowsBuild = info.windowsBuild;
      const root = document.documentElement.style;
      if (info.customTitlebar) {
        watchTitlebarInsets();
      } else if (info.trafficLights) {
        // macOS：交通灯在左上角（位置见 aproxy-desktop/src/lib.rs 的 TRAFFIC_LIGHTS：离左边 15px，
        // 三个 14px 的按钮加两道间隔约 54px 宽），左边让出 15 + 54 + 15 = 84px，右边没有按钮
        ui.trafficLights = true;
        root.setProperty('--titlebar-left', '84px');
        root.setProperty('--titlebar-right', '14px');
      } else if (info.ownControls) {
        // Linux：三个窗口按钮画在标题栏最右边（Titlebar.svelte），右内边距只留一点，按钮离窗口边 8px
        ui.ownControls = true;
        root.setProperty('--titlebar-right', '8px');
      } else {
        // 原生标题栏之下的一行工具栏：右边没有要让位的按钮，与左边留一样的边
        root.setProperty('--titlebar-right', '14px');
      }
      syncWindow();
    },
    (err) => console.warn('[shell] desktop_info failed', err),
  );
}

/** 已经交给窗口的状态，没变就不重复调用 */
let sentTheme: 'light' | 'dark' | null | undefined;
let sentCaption: string | null | undefined;
let micaTarget = false;
/**
 * 发给窗口的调用（窗口主题、Mica、标题栏按钮后面的底色）排成一队，上一个完成了才发下一个。
 * 每个调用是一次独立的 IPC 请求，同时发出的几个不保证按发出的顺序在 Rust 那边执行：
 * 启动时偏好还没读到，先按系统深浅色发一次底色，读到「深色」后马上再发一次，后发的先执行时，
 * 按钮区就停在了浅色（Windows 11 上见过）。Mica 的切换本身也要按顺序做完（先材质、后页面或者反过来）。
 */
let windowQueue: Promise<void> = Promise.resolve();

/** 排进 windowQueue；出错只记一笔，不挡后面的调用 */
function queueWindow(what: string, call: () => Promise<void>): void {
  windowQueue = windowQueue.then(call).catch((err) => console.warn(`[shell] ${what} failed`, err));
}

/** 把界面的外观同步到窗口。App 每次改完 <html> 的主题属性之后调用，这时读到的计算样式才是新的。 */
export function syncWindow(): void {
  if (!inTauri) return;
  const win = getCurrentWindow();

  // 窗口主题决定 Mica 的色调，也决定页面的 prefers-color-scheme：跟随系统时交还给系统，
  // 用户选定浅色或深色时整个窗口一起切（这正是 setTheme 的本意，不是为了给标题栏按钮上色）
  const theme = ui.modePref === 'system' ? null : ui.modePref;
  if (theme !== sentTheme) {
    sentTheme = theme;
    queueWindow('setTheme', () => win.setTheme(theme));
  }

  const wantMica = ui.micaEnabled && ui.micaSupport !== 'unsupported';
  if (wantMica !== micaTarget) {
    micaTarget = wantMica;
    queueWindow('switching Mica', async () => {
      if (wantMica) {
        await win.setEffects({ effects: [Effect.Mica] });
        ui.micaWindow = true;
      } else {
        ui.micaWindow = false;
        await nextPaint();
        await win.clearEffects();
      }
    });
  }

  // 标题栏右上角原生按钮后面的底色：Mica 开着时交给系统（材质一直铺到按钮下面），
  // 关着时涂成页面的底色，按钮区与页面标题栏连成一片。DWM 按这个颜色自动挑按钮的深浅
  const caption = ui.micaActive ? null : toHex(getComputedStyle(document.documentElement).backgroundColor);
  if (caption !== sentCaption) {
    sentCaption = caption;
    queueWindow('setTitlebarColor', () => setTitlebarColor(caption));
  }
}

/** 等页面把当前的样式真正画出来一帧 */
function nextPaint(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
}

let probe: CanvasRenderingContext2D | null = null;

/** 任意 CSS 颜色（oklch()、color-mix() 算出的值）→ #rrggbb：画到 1×1 的画布上读回 sRGB 像素。setTitlebarColor 只认这种写法 */
function toHex(color: string): string | null {
  if (!probe) {
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 1;
    probe = canvas.getContext('2d', { willReadFrequently: true });
    if (!probe) return null;
  }
  probe.clearRect(0, 0, 1, 1);
  probe.fillStyle = '#000';
  probe.fillStyle = color;
  probe.fillRect(0, 0, 1, 1);
  const [r, g, b] = probe.getImageData(0, 0, 1, 1).data;
  return '#' + [r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('');
}

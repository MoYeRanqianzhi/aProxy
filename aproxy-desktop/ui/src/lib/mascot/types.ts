/*
 * 吉祥物契约。
 *
 * 吉祥物是「按状态渲染」的纯函数，而不是写死在界面里的一张图：界面把整体状态整理成 MascotState 交给它，
 * 它返回一段 SVG 标记。这样做是为了让形象可以替换——内置几种之外，用户以后还能注入自己的吉祥物——
 * 所以契约里刻意不出现任何界面内部的东西：
 *   - 颜色由宿主解析成具体的 CSS 颜色值放进 palette，吉祥物不读界面的 CSS 变量（变量名会随界面改版变化）；
 *   - 减少动效、深浅主题也由宿主传进来，吉祥物不去查 <html> 上的属性或媒体查询；
 *   - 渲染结果放在吉祥物独占的地方：内置的放进 shadow root，用户装的自定义吉祥物连代码带画面整个跑在
 *     沙箱 iframe 里（不透明源，见 MascotFrame 与 public/mascot-sandbox.js）。吉祥物的样式影响不到界面，反之亦然。
 * 吉祥物隔离运行的理由是稳定：一个画错、抛异常或卡死的吉祥物不能拖垮界面。它拿到的只有状态数据，本来也没有可泄露的东西。
 * 自定义吉祥物以一个 ES 模块交付，默认导出下面的 MascotDefinition。
 *
 * 宿主只在状态真正变化时重新调用 render 并替换标记（标记文本不变就不动 DOM），
 * 所以「状态变化的那一刻」对吉祥物来说就是一次全新的挂载：一次性的动画（被撞歪、撒星星）放在挂载时播即可。
 */

/**
 * 整体状态，按优先级由宿主归纳：
 *   idle      有实例在跑，眼下没有请求
 *   working   有请求在途，一切正常
 *   retrying  有请求正在重试（上游出错，aProxy 在扛）
 *   rescued   刚刚有请求在长时间重试后成功——短暂的庆祝时刻，几秒后宿主会切回其它状态
 *   worried   有实例无响应或出错
 *   sleeping  没有实例在跑，或很久没有请求
 */
export type MascotMood = 'idle' | 'working' | 'retrying' | 'rescued' | 'worried' | 'sleeping';

export const MASCOT_MOODS: readonly MascotMood[] = ['idle', 'working', 'retrying', 'rescued', 'worried', 'sleeping'];

/**
 * 渲染尺寸档位，决定细节多少而不是具体像素（像素由宿主的容器决定，SVG 自适应填满）：
 *   icon   ≤ 32px：只留轮廓、标志性部件与状态色点，不动
 *   small  约 40–64px：完整形象，省掉文字角标等看不清的细节
 *   hero   约 96px 以上：全部细节与动画
 */
export type MascotSize = 'icon' | 'small' | 'hero';

/**
 * 宿主从当前主题解析好的颜色，每一项都是可以直接写进 SVG 的 CSS 颜色字符串。
 * 深浅主题与配色强度变化时宿主会重新解析并重新渲染。
 */
export interface MascotPalette {
  /** 品牌主色：通常用作身体 */
  primary: string;
  /** 主色的浅一档（肚皮、高光），两种主题下都比 primary 浅 */
  primarySoft: string;
  /** 主色的深色调（描边、呆毛），两种主题下都比 primary 深 */
  primaryDeep: string;
  /** 画在主色上的五官颜色，两种主题下都是深色 */
  ink: string;
  /** 腮红 */
  blush: string;
  /** 近白的暖色，适合当第二种身体色或信封纸 */
  cream: string;
  /** 吉祥物身后的底色：角标、状态点外圈用它和背景隔开 */
  surface: string;
  /** 状态语义色：正常 / 重试中 / 出错 / 空闲与睡眠 / 庆祝 */
  ok: string;
  retry: string;
  error: string;
  idle: string;
  star: string;
  /** 落地阴影的颜色（不透明度由吉祥物自己控制） */
  shadow: string;
}

export interface MascotState {
  mood: MascotMood;
  /** 正在重试的请求里重试次数最多的那个已经重试了几次；0 表示不显示次数 */
  retries: number;
  size: MascotSize;
  /** 用户开启了减少动效：不要播放位移、旋转、缩放类动画，表情照常切换 */
  reducedMotion: boolean;
  theme: 'light' | 'dark';
  palette: MascotPalette;
}

export interface MascotIconOptions {
  /** 目标像素尺寸（16 / 24 / 32 / 48 / 256…），吉祥物据此挑选细节层级，小尺寸要按像素网格设计 */
  px: number;
  /** null 表示软件图标（不带状态）；有值表示托盘状态版 */
  mood: MascotMood | null;
  /** 图标正后方那块底色的深浅（托盘是任务栏，软件图标是底板），不一定等于界面主题：小尺寸下细节只能按它取舍颜色 */
  theme: 'light' | 'dark';
  /** 托盘场景下 palette.surface 是任务栏的底色，状态点外圈用它「挖」出缝隙 */
  palette: MascotPalette;
}

export interface MascotDefinition {
  /** 稳定 id，写进设置；内置的用短名，自定义的建议带作者前缀避免冲突 */
  id: string;
  /** 显示名：至少要有 en，界面按当前语言取，缺了就退回 en */
  name: { en: string; [lang: string]: string };
  /** 一句话描述，显示在设置里的吉祥物选择处 */
  description?: { en: string; [lang: string]: string };
  /** 吉祥物自己的样式表（多半是动画），宿主放进它独占的 shadow root 或沙箱页；类名随便起，不会和界面冲突 */
  css?: string;
  /**
   * 按状态返回一段 <svg>…</svg>。必须是纯函数：同样的状态返回同样的标记。
   * 标记里不要写 style 属性、<style> 或 <script>：界面的 CSP 禁止内联样式与脚本，样式一律放进 css。
   */
  render(state: MascotState): string;
  /** 静态图标：软件图标与托盘图标。不含动画，返回带 xmlns 的完整 <svg>，可以直接存成文件 */
  icon(opts: MascotIconOptions): string;
}

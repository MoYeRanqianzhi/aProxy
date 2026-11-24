<!--
  自定义标题栏，布局约定与 tauri-custom-titlebar 一致，用的是它的「最小高度变体」：
  - 栏高 --bar = max(--titlebar-height, --titlebar-min)。--titlebar-height 是 Windows 原生按钮的高度（titlebar.js
    写入，150% 缩放时约 29px，最大化时约 22px），照它的高度，搜索框会被压扁；--titlebar-min 默认 44px（app.css，
    主题可改），所以标题栏至少有这么高，最大化时也不变矮。栏里的控件都按 --bar 取高度，栏高随主题变了也不溢出；
  - 原生按钮保持系统的高度、贴在顶部，比标题栏矮出来的那一截露出页面自己的底色，所以按钮的悬停高亮只盖住
    上半部分。这是这个变体有意接受的取舍；
  - 右侧 --titlebar-right 宽的矩形归 Windows 原生按钮，页面在那里画什么都看不见，这里用 padding 让出来
    （让出整个栏高：按钮下面那一截只露底色，不放内容）；
  - 顶部 --titlebar-top 是最大化时被屏幕吃掉的部分；
  - 整条 app-region: drag（拖动、双击最大化、右键系统菜单、Aero Snap 都由 Windows 提供），
    可交互的子元素 app-region: no-drag。
  macOS（Overlay 标题栏，见 aproxy-desktop/src/lib.rs）用同一条栏：左边 --titlebar-left 宽的一块让给系统
  画的交通灯，右边没有按钮；WKWebView 不认 app-region，拖动与双击缩放改由 Tauri 的
  data-tauri-drag-region="deep" 提供（栏里任何不可点的地方都能拖，按钮、链接自动排除），只在 macOS 上加，
  免得 Windows 上与 app-region 重复处理同一次按下。快捷键提示按平台写 ⌘K 或 Ctrl K。
  Linux（窗口没有装饰，见 aproxy-desktop/src/lib.rs）也用同一条栏，右边是页面自己画的最小化、最大化/还原、
  关闭三个按钮。WebKitGTK 不认 app-region；data-tauri-drag-region 在 X11 上双击不灵（第一下按下就交给了
  窗口管理器的移动，第二下到不了页面），所以拖动与双击最大化用 titlebar.js 的 enableDragRegion：只认按在
  栏本身上的那一下，品牌区因此设成不接收指针（pointer-events: none），按在上面等于按在栏上。栏上的右键
  不弹 WebKit 的页面菜单（后退、刷新、检查元素，对窗口没有意义）。
  后端给的是演示数据时（预览版），右侧常驻一个「预览版 · 演示数据」标签：标题栏在每一页都看得见，
  用户不会把假的实例当成自己电脑上正在跑的。悬停有完整说明，所以它也不参与拖动（no-drag 写在 app.css）。
-->
<script lang="ts">
  import { t } from '../lib/i18n';
  import { store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import { mascots } from '../lib/mascot/registry.svelte';
  import MascotView from './MascotView.svelte';
  import Icon from './Icon.svelte';
  import { ui } from '../lib/state.svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { enableDragRegion } from '../lib/titlebar.js';

  // 快捷键的监听同时认 Ctrl 与 ⌘（App.svelte），这里只决定提示怎么写
  const isMac = navigator.userAgent.includes('Mac');

  let bar: HTMLElement;
  /** 窗口是否最大化：决定中间那个按钮画「最大化」还是「还原」 */
  let maximized = $state(false);

  // 只有 Linux 的窗口（ownControls）才接管拖动与窗口按钮；getCurrentWindow 只在 Tauri 里能调，
  // 所以也只在这里调。窗口大小一变（拖拽改大小、最大化、还原）就重新问一次是否最大化
  $effect(() => {
    if (!ui.ownControls) return;
    const win = getCurrentWindow();
    const stopDrag = enableDragRegion(bar, win);
    const noMenu = (e: MouseEvent) => e.preventDefault();
    bar.addEventListener('contextmenu', noMenu);
    const sync = () => void win.isMaximized().then((m) => (maximized = m));
    sync();
    const unlisten = win.onResized(sync);
    return () => {
      stopDrag();
      bar.removeEventListener('contextmenu', noMenu);
      void unlisten.then((off) => off());
    };
  });
</script>

<header class="titlebar" class:own={ui.ownControls} bind:this={bar} data-tauri-drag-region={ui.trafficLights ? 'deep' : undefined}>
  <div class="brand">
    <!-- 标志就是吉祥物的图标版：状态点跟着整体状态变。选了「不显示」时退回一个中性的 ↻ 标记 -->
    {#if mascots.current}
      <MascotView mood={store.mood} size="icon" px={22} />
    {:else}
      <span class="mark"><Icon name="restart" size={13} stroke={2.6} /></span>
    {/if}
    <span class="wordmark">aProxy</span>
  </div>
  <button class="search" onclick={() => (overlays.palette = true)}>
    <Icon name="search" size={14} stroke={2.4} />
    <span class="ph">{t('search.placeholder')}</span>
    <span class="kbd">{isMac ? '⌘K' : 'Ctrl K'}</span>
  </button>
  {#if store.demo}
    <span class="demo chip solid butter" role="note" title={t('demo.explain')}>
      <Icon name="info" size={13} stroke={2.4} />{t('demo.label')}
    </span>
  {/if}
  {#if ui.ownControls}
    <div class="controls">
      <button class="ctl" onclick={() => getCurrentWindow().minimize()} title={t('win.minimize')} aria-label={t('win.minimize')}>
        <Icon name="minus" size={16} stroke={2.2} />
      </button>
      <button
        class="ctl"
        onclick={() => getCurrentWindow().toggleMaximize()}
        title={t(maximized ? 'win.restore' : 'win.maximize')}
        aria-label={t(maximized ? 'win.restore' : 'win.maximize')}
      >
        <Icon name={maximized ? 'restore' : 'maximize'} size={14} stroke={2.2} />
      </button>
      <button class="ctl close" onclick={() => getCurrentWindow().close()} title={t('win.close')} aria-label={t('win.close')}>
        <Icon name="x" size={16} stroke={2.2} />
      </button>
    </div>
  {/if}
</header>

<style>
  .titlebar {
    /* 最小高度变体（见上）。浏览器里与非 Windows 上没有 --titlebar-height，栏高就是 --titlebar-min：
       原生标题栏下面的这一行工具栏默认也是 44px，有意与 Windows 一致，给搜索框留够高度 */
    --bar: max(var(--titlebar-height, 32px), var(--titlebar-min, 44px));
    /* content-box：底边不会侵入原生按钮那块区域 */
    box-sizing: content-box;
    flex: none;
    height: var(--bar);
    padding: var(--titlebar-top, 0px) var(--titlebar-right, 146px) 0 var(--titlebar-left, 14px);
    display: flex;
    align-items: center;
    gap: 14px;
    background: var(--chrome-bg);
    color: var(--text);
    /* app-region（拖动区）写在 app.css：组件样式的 CSS 检查不认识这个属性 */
    user-select: none;
    position: relative;
    z-index: 5;
  }
  :global(body.inactive) .titlebar {
    color: var(--text-3);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 7px;
    /* 搜索框与下面的内容区左边对齐：品牌区宽度 = 导航宽度减去标题栏左内边距与 gap；
       macOS 的左内边距让给了交通灯，品牌区跟着变窄，至少留出标志与字标的宽度 */
    width: max(96px, calc(var(--nav-w) - var(--titlebar-left, 14px) - 14px));
  }
  .mark {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 7px;
    background: var(--lilac);
    color: var(--on-fill);
  }
  .wordmark {
    font-size: 15px;
    font-weight: 620;
    letter-spacing: 0.01em;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    width: min(380px, 42%);
    /* 默认 44px 的栏里是 32px；主题把栏压到 36px 时跟着降到 26px */
    height: min(32px, calc(var(--bar) - 10px));
    padding: 0 4px 0 11px;
    border-radius: var(--r-pill);
    border: var(--line-w-thin) solid var(--line-soft);
    background: color-mix(in oklab, var(--card) 70%, transparent);
    color: var(--text-3);
    font-size: var(--fs-sm);
    transition:
      border-color var(--dur-fast) linear,
      background-color var(--dur-fast) linear;
  }
  .search:hover {
    border-color: var(--line);
    background: var(--card);
    color: var(--text-2);
  }
  .ph {
    flex: 1;
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .search .kbd {
    height: min(20px, calc(var(--bar) - 22px));
    font-size: 10.5px;
  }
  /* 奶黄是「提醒」的语义色；高度跟着栏高走，比搜索框矮一档 */
  .demo {
    margin-left: auto;
    flex: none;
    height: min(26px, calc(var(--bar) - 14px));
    font-weight: 600;
    cursor: help;
  }

  /* ---------- Linux：页面自己画的窗口按钮 ---------- */
  /* 品牌区不接收指针：按在上面的那一下落到栏本身，enableDragRegion 才认作拖动或双击最大化 */
  .own .brand {
    pointer-events: none;
  }
  /* 三个按钮靠右；有「演示数据」标签时由它占住剩下的空白，按钮紧跟在后面 */
  .controls {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-left: auto;
    flex: none;
  }
  .demo ~ .controls {
    margin-left: 0;
  }
  /* 和页面上别的图标按钮一样是圆的：平时只有图标，悬停时浮出一块浅底；关闭按钮悬停变珊瑚色，
     与「停止」这类操作同一个警示色。尺寸跟着栏高走，主题把栏压矮时一起变小 */
  .ctl {
    display: grid;
    place-items: center;
    width: min(32px, calc(var(--bar) - 12px));
    height: min(32px, calc(var(--bar) - 12px));
    padding: 0;
    border: 0;
    border-radius: var(--r-pill);
    background: transparent;
    color: var(--text-2);
    transition:
      background-color var(--dur-fast) linear,
      color var(--dur-fast) linear,
      transform var(--dur-fast) var(--ease-spring);
  }
  .ctl:hover {
    background: var(--hover);
    color: var(--text);
  }
  .ctl.close:hover {
    background: var(--coral);
    color: var(--on-fill);
  }
  .ctl:active {
    transform: scale(0.92);
  }
  .ctl:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 1px;
  }
</style>

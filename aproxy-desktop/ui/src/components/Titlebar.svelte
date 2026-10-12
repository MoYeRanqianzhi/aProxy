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
</script>

<header class="titlebar">
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
    <span class="kbd">Ctrl K</span>
  </button>
  {#if store.demo}
    <span class="demo chip solid butter" role="note" title={t('demo.explain')}>
      <Icon name="info" size={13} stroke={2.4} />{t('demo.label')}
    </span>
  {/if}
</header>

<style>
  .titlebar {
    /* 最小高度变体（见上）。浏览器里与非 Windows 上没有 --titlebar-height，按 32px 算 */
    --bar: max(var(--titlebar-height, 32px), var(--titlebar-min, 44px));
    /* content-box：底边不会侵入原生按钮那块区域 */
    box-sizing: content-box;
    flex: none;
    height: var(--bar);
    padding: var(--titlebar-top, 0px) var(--titlebar-right, 146px) 0 14px;
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
    width: calc(var(--nav-w) - 14px - 14px);
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
</style>

<!--
  自定义标题栏，布局约定与 tauri-custom-titlebar 一致：
  - 高度 = --titlebar-height（Windows 上由 titlebar.js 写入，最大化时会变矮，所以控件高度跟着 --bar 缩放）；
  - 右侧 --titlebar-right 宽的矩形归 Windows 原生按钮，页面在那里画什么都看不见，这里用 padding 让出来；
  - 顶部 --titlebar-top 是最大化时被屏幕吃掉的部分；
  - 整条 app-region: drag（拖动、双击最大化、右键系统菜单、Aero Snap 都由 Windows 提供），
    可交互的子元素 app-region: no-drag。
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
</header>

<style>
  .titlebar {
    --bar: var(--titlebar-height, 32px);
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
    height: min(26px, calc(var(--bar) - 7px));
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
    height: 18px;
    font-size: 10.5px;
  }
</style>

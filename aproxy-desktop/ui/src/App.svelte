<!--
  窗口外壳：标题栏 + 左侧导航 + 内容纸面，以及全局浮层（确认框、命令面板、轻提示）。
  模式、主题、配色强度、Mica、动效偏好全部落到 <html> 的 data-* 与令牌上，样式只读令牌；
  在桌面窗口里，同样的状态再经 lib/shell.ts 交给窗口（窗口主题、Mica 材质、标题栏按钮后面的底色）。
  proto/ 下的组件只属于原型（仿原生标题栏按钮、Mica 壁纸模拟、审核工具条、吉祥物预览页）：只在浏览器开发
  模式里动态加载，桌面窗口里由真窗口提供这些，生产构建里它们整个不进产物。
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { ui, parseHash, pageHash } from './lib/state.svelte';
  import { store } from './lib/store.svelte';
  import { overlays } from './lib/overlays.svelte';
  import { mascotPalette } from './lib/mascot/palette.svelte';
  import { mascots } from './lib/mascot/registry.svelte';
  import { applyTheme } from './lib/theme';
  import { pageIn } from './lib/motion';
  import Titlebar from './components/Titlebar.svelte';
  import Nav from './components/Nav.svelte';
  import ConfirmDialog from './components/ConfirmDialog.svelte';
  import ConsentDialog from './components/ConsentDialog.svelte';
  import ThemePreviewBar from './components/ThemePreviewBar.svelte';
  import CommandPalette from './components/CommandPalette.svelte';
  import Toasts from './components/Toasts.svelte';
  import Overview from './pages/Overview.svelte';
  import InstanceDetail from './pages/InstanceDetail.svelte';
  import Logs from './pages/Logs.svelte';
  import Config from './pages/Config.svelte';
  import Update from './pages/Update.svelte';
  import Settings from './pages/Settings.svelte';
  import { inTauri, syncWindow } from './lib/shell';

  // 原型件只在浏览器开发模式里加载。生产构建里 import.meta.env.DEV 是常量 false，整个分支连同
  // 动态导入一起被删掉，产物里没有 proto/ 的代码（也就没有审核工具条与它暴露的 window.__aproxyProto）
  let proto = $state.raw<typeof import('./proto/parts') | null>(null);
  if (import.meta.env.DEV && !inTauri) {
    import('./proto/parts').then((parts) => (proto = parts));
  }

  $effect(() => {
    const root = document.documentElement;
    // 主题只是一张改令牌的样式表；data-theme 告诉主题与扩展的样式「现在穿的是不是内置主题」
    applyTheme(ui.activeTheme);
    root.dataset.theme = ui.activeTheme ? 'custom' : 'aproxy';
    root.dataset.variant = ui.variant;
    root.dataset.palette = ui.palette;
    root.dataset.mica = ui.micaActive ? 'on' : 'off';
    root.dataset.motion = ui.reducedMotion ? 'reduced' : 'full';
    root.lang = ui.lang;
    // 吉祥物不读 CSS 变量，主题一变就在这里重新解析它的调色板（必须在上面的样式表与属性都写完之后）
    mascotPalette.refresh();
    // 窗口这一侧同理：标题栏按钮后面的底色读的是刚算好的页面底色
    syncWindow();
  });

  // 已装的自定义吉祥物来自扩展列表（kind 为 mascot）。注册表只记名字和位置，代码由 MascotFrame 交给沙箱运行
  $effect(() => {
    mascots.setCustom(
      store.extensions
        .filter((e) => e.kind === 'mascot' && e.enabled)
        .map((e) => ({ id: e.id, name: e.name, description: e.description, origin: e.origin })),
    );
  });

  // 整体状态挂在根上：将来的主题与扩展可以按 [data-mood] 换样式，不必依赖内部 DOM 结构
  $effect(() => {
    document.documentElement.dataset.mood = store.mood;
  });

  // 页面 ⇄ 地址栏 hash：可以直接打开 #/logs、#/instance/12346 审核某一页
  $effect(() => {
    const h = pageHash(ui.page, ui.instancePort);
    if (location.hash !== h) history.replaceState(null, '', h);
  });

  onMount(() => {
    const fromHash = () => {
      const r = parseHash(location.hash);
      if (r) ui.go(r.page, r.port);
    };
    addEventListener('hashchange', fromHash);

    const dark = matchMedia('(prefers-color-scheme: dark)');
    const motion = matchMedia('(prefers-reduced-motion: reduce)');
    const sync = () => {
      ui.systemDark = dark.matches;
      ui.systemReducedMotion = motion.matches;
    };
    sync();
    dark.addEventListener('change', sync);
    motion.addEventListener('change', sync);
    ui.systemLang = navigator.language.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en';

    // 窗口失焦时标题栏文字变淡，与原生窗口一致
    const blur = () => document.body.classList.add('inactive');
    const focus = () => document.body.classList.remove('inactive');
    addEventListener('blur', blur);
    addEventListener('focus', focus);

    store.connect();

    return () => {
      removeEventListener('hashchange', fromHash);
      dark.removeEventListener('change', sync);
      motion.removeEventListener('change', sync);
      removeEventListener('blur', blur);
      removeEventListener('focus', focus);
    };
  });

  function onKey(ev: KeyboardEvent) {
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'k') {
      ev.preventDefault();
      overlays.palette = !overlays.palette;
    } else if (ev.altKey && ev.key.toLowerCase() === 'r') {
      ev.preventDefault();
      ui.toolbarHidden = !ui.toolbarHidden;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if proto}
  <proto.ProtoBackdrop />
{/if}

<div class="window">
  <Titlebar />
  <div class="body">
    <Nav />
    <main class="sheet">
      <ThemePreviewBar />
      {#key `${ui.page}:${ui.page === 'instance' || ui.page === 'config' ? ui.instancePort : ''}`}
        <div class="page" class:flush={ui.page === 'logs'} in:pageIn>
          {#if ui.page === 'overview'}
            <Overview />
          {:else if ui.page === 'instance'}
            <InstanceDetail />
          {:else if ui.page === 'logs'}
            <Logs />
          {:else if ui.page === 'config'}
            <Config />
          {:else if ui.page === 'update'}
            <Update />
          {:else if ui.page === 'mascots'}
            {#if proto}
              <proto.MascotLab />
            {/if}
          {:else}
            <Settings />
          {/if}
        </div>
      {/key}
    </main>
  </div>
</div>

<ConfirmDialog />
<ConsentDialog />
<CommandPalette />
<Toasts />
{#if proto}
  <proto.ProtoFrame />
  <proto.ReviewToolbar />
{/if}

<style>
  .window {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  /* 内容纸面：只露出上边与左边的浅色细边和一个大圆角，靠底色差与窗口外框分层 */
  .sheet {
    position: relative;
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--sheet-bg);
    border: var(--line-w) solid var(--line);
    border-right: 0;
    border-bottom: 0;
    border-top-left-radius: var(--r-xl);
    overflow: hidden;
  }
  .page {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 24px 28px 32px;
  }
  /* 日志页自己管理滚动（表头吸顶、详情抽屉），外层不滚 */
  .page.flush {
    overflow: hidden;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
</style>

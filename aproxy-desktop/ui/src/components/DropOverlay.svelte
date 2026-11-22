<!--
  把主题包或扩展包拖进窗口就开始安装。拖着文件经过窗口时盖一层提示，松开后交给安装流程
  （lib/packages.svelte.ts），之后的提醒与在设置页点「安装…」完全一样。

  桌面窗口里，Tauri 接管了系统的拖放（页面收不到 HTML 的 drop），交来的是路径，文件与解开的目录都行；
  浏览器里（开发、截图）用 HTML 的拖放，只拿得到文件。一次只装一个包：拖进来好几个就装第一个，并说一声。
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '../lib/i18n';
  import { inTauri } from '../lib/shell';
  import { overlays } from '../lib/overlays.svelte';
  import { packages } from '../lib/packages.svelte';
  import { fadeOut, pop } from '../lib/motion';
  import Icon from './Icon.svelte';

  let over = $state(false);

  function begin(count: number, start: () => void) {
    over = false;
    if (count === 0 || packages.busy) return;
    if (count > 1) overlays.toast(t('install.drop.many'), 'lilac');
    start();
  }

  onMount(() => {
    if (inTauri) {
      let stop: (() => void) | null = null;
      let disposed = false;
      void import('@tauri-apps/api/webview').then(({ getCurrentWebview }) =>
        getCurrentWebview()
          .onDragDropEvent((ev) => {
            const p = ev.payload;
            if (p.type === 'enter') over = p.paths.length > 0 && !packages.busy;
            else if (p.type === 'leave') over = false;
            else if (p.type === 'drop') begin(p.paths.length, () => void packages.beginPath(p.paths[0]));
          })
          .then((un) => {
            if (disposed) un();
            else stop = un;
          }),
      );
      return () => {
        disposed = true;
        stop?.();
      };
    }

    // 浏览器：dragenter/dragleave 在子元素之间来回触发，用计数判断是不是真的离开了窗口
    let depth = 0;
    const hasFiles = (ev: DragEvent) => !!ev.dataTransfer?.types.includes('Files');
    const enter = (ev: DragEvent) => {
      if (!hasFiles(ev)) return;
      depth++;
      over = !packages.busy;
    };
    const leave = (ev: DragEvent) => {
      if (!hasFiles(ev)) return;
      depth = Math.max(0, depth - 1);
      if (depth === 0) over = false;
    };
    const dragover = (ev: DragEvent) => {
      if (hasFiles(ev)) ev.preventDefault();
    };
    const drop = (ev: DragEvent) => {
      if (!hasFiles(ev)) return;
      ev.preventDefault();
      depth = 0;
      const files = [...(ev.dataTransfer?.files ?? [])];
      begin(files.length, () => void packages.beginFile(files[0]));
    };
    addEventListener('dragenter', enter);
    addEventListener('dragleave', leave);
    addEventListener('dragover', dragover);
    addEventListener('drop', drop);
    return () => {
      removeEventListener('dragenter', enter);
      removeEventListener('dragleave', leave);
      removeEventListener('dragover', dragover);
      removeEventListener('drop', drop);
    };
  });
</script>

{#if over}
  <div class="drop" in:fadeOut={{ duration: 160 }} out:fadeOut={{ duration: 140 }} aria-hidden="true">
    <div class="card" in:pop={{ y: 10, scale: 0.96, duration: 420 }}>
      <span class="icon"><Icon name="download" size={26} stroke={2.2} /></span>
      <strong>{t('install.drop.title')}</strong>
      <span>{t('install.drop.body')}</span>
    </div>
  </div>
{/if}

<style>
  .drop {
    position: fixed;
    inset: 10px;
    z-index: 90;
    display: grid;
    place-items: center;
    border-radius: var(--r-xl);
    border: 2px dashed var(--lilac);
    background: color-mix(in oklch, var(--lilac-soft) 78%, transparent);
    pointer-events: none;
  }
  .card {
    display: grid;
    justify-items: center;
    gap: 6px;
    padding: 22px 30px;
    border-radius: var(--r-lg);
    background: var(--overlay-bg);
    box-shadow: var(--shadow-3);
    text-align: center;
  }
  .icon {
    display: grid;
    place-items: center;
    width: 52px;
    height: 52px;
    margin-bottom: 4px;
    border-radius: 50%;
    background: var(--lilac-soft);
    color: var(--lilac-deep);
  }
  strong {
    font-size: var(--fs-lg);
  }
  span {
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
</style>

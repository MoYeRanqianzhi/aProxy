<!--
  【仅原型】仿 Windows 11 的最小化 / 最大化 / 关闭按钮。

  正式应用里这块矩形（--titlebar-right × --titlebar-height）由 Windows 原生按钮占据，页面画的东西在那里
  一概不可见；这里只是为了在浏览器里看到接近真实的整体观感。接入 Tauri 时整个 proto/ 目录删掉。
  层级放在最高：原生按钮永远盖在网页之上，连对话框的遮罩也盖不住它们。
-->
<script lang="ts">
  import { t } from '../lib/i18n';
</script>

<div class="caption" aria-hidden="true">
  <span class="cb" title={t('proto.minimize')}>
    <svg viewBox="0 0 10 10" width="10" height="10"><path d="M0 5h10" /></svg>
  </span>
  <span class="cb" title={t('proto.maximize')}>
    <svg viewBox="0 0 10 10" width="10" height="10"><rect x="0.5" y="0.5" width="9" height="9" rx="1.5" /></svg>
  </span>
  <span class="cb close" title={t('proto.closeWin')}>
    <svg viewBox="0 0 10 10" width="10" height="10"><path d="M0.5 0.5l9 9M9.5 0.5l-9 9" /></svg>
  </span>
</div>

<style>
  .caption {
    position: fixed;
    top: 0;
    right: 0;
    z-index: 1000;
    display: flex;
    width: var(--titlebar-right, 146px);
    height: calc(var(--titlebar-top, 0px) + var(--titlebar-height, 32px));
    padding-top: var(--titlebar-top, 0px);
  }
  .cb {
    flex: 1;
    display: grid;
    place-items: center;
    color: var(--caption-fg);
    transition: background-color 90ms linear;
  }
  .cb:hover {
    background: var(--caption-hover);
  }
  .cb.close:hover {
    background: #c42b1c;
    color: #fff;
  }
  svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1px;
    shape-rendering: geometricPrecision;
  }
</style>

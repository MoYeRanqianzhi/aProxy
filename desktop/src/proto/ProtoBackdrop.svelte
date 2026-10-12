<!--
  【仅原型】模拟 Mica：一张彩色「桌面壁纸」，经强模糊与主题色调和后垫在透明的页面下面。
  真实的 Mica 由 Windows 合成，页面只负责在需要透出的地方保持透明（见 app.css 的 data-mica 令牌）。

  系统关了透明效果时，Windows 把 Mica 画成纯色（浅 #f3f3f3 / 深 #202020），这里照样模拟。
-->
<script lang="ts">
  import { ui } from '../lib/state.svelte';
</script>

{#if ui.micaActive}
  <div class="backdrop" class:solid={ui.micaSupport === 'transparency-off'} aria-hidden="true">
    {#if ui.micaSupport !== 'transparency-off'}
      <div class="wallpaper"></div>
    {/if}
    <div class="tint"></div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: -1;
    overflow: hidden;
  }
  .wallpaper {
    position: absolute;
    inset: -80px;
    /* 仿 Windows 11 默认壁纸「Bloom」的几团颜色 */
    background:
      radial-gradient(40% 50% at 18% 30%, #2f6bff 0%, transparent 70%),
      radial-gradient(35% 45% at 70% 25%, #a34dff 0%, transparent 70%),
      radial-gradient(40% 40% at 85% 80%, #ff6fa8 0%, transparent 70%),
      radial-gradient(45% 50% at 30% 85%, #18c4c4 0%, transparent 70%),
      #5f7dff;
    filter: blur(70px) saturate(1.15);
  }
  /* Mica 的调和层：浅色主题偏白，深色主题偏黑，壁纸只剩一层色调 */
  .tint {
    position: absolute;
    inset: 0;
    background: rgb(243 243 243 / 0.72);
  }
  :global(:root[data-variant='dark']) .tint {
    background: rgb(32 32 32 / 0.78);
  }
  .solid .tint {
    background: #f3f3f3;
  }
  :global(:root[data-variant='dark']) .solid .tint {
    background: #202020;
  }
</style>

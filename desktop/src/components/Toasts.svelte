<!--
  轻提示：底部居中的小胶囊，弹出时轻轻回弹，2.8 秒后淡出。
  用 aria-live 播报，屏幕阅读器用户也能知道操作结果。
-->
<script lang="ts">
  import { flip } from 'svelte/animate';
  import { overlays } from '../lib/overlays.svelte';
  import { ui } from '../lib/state.svelte';
  import { pop, fadeOut } from '../lib/motion';
  import Icon from './Icon.svelte';
</script>

<div class="toasts" aria-live="polite">
  {#each overlays.toasts as toast (toast.id)}
    <div
      class="toast {toast.tone}"
      in:pop={{ y: 20, scale: 0.9, duration: 560 }}
      out:fadeOut={{ duration: 200 }}
      animate:flip={{ duration: ui.reducedMotion ? 0 : 300 }}
    >
      <span class="badge"><Icon name={toast.tone === 'coral' ? 'alert' : 'check'} size={13} stroke={2.8} /></span>
      {toast.text}
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    left: 50%;
    bottom: 22px;
    z-index: 90;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    transform: translateX(-50%);
    pointer-events: none;
  }
  .toast {
    display: inline-flex;
    align-items: center;
    gap: 9px;
    padding: 7px 16px 7px 8px;
    border-radius: var(--r-pill);
    border: var(--line-w) solid var(--line);
    background: var(--overlay-bg);
    color: var(--text);
    font-weight: 550;
    white-space: nowrap;
    box-shadow: var(--shadow-3);
  }
  .badge {
    display: inline-grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    color: var(--on-fill);
    background: var(--mint);
  }
  .lilac .badge {
    background: var(--lilac);
  }
  .coral .badge {
    background: var(--coral);
  }
</style>

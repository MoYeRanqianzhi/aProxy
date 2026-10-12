<!--
  二次确认：停止、重启、强制停止都要先过这一关。
  打开时焦点落在「取消」上——危险操作的默认选项必须是不做，回车不会误触。
-->
<script lang="ts">
  import { overlays } from '../lib/overlays.svelte';
  import { t } from '../lib/i18n';
  import { pop, fadeOut } from '../lib/motion';
  import Icon from './Icon.svelte';

  let cancelBtn: HTMLButtonElement | undefined = $state();
  let dialog: HTMLDivElement | undefined = $state();

  $effect(() => {
    if (overlays.confirm) queueMicrotask(() => cancelBtn?.focus());
  });

  function close() {
    overlays.confirm = null;
  }

  function ok() {
    const req = overlays.confirm;
    overlays.confirm = null;
    req?.onOk();
  }

  // 焦点只在对话框内循环
  function onKey(ev: KeyboardEvent) {
    if (ev.key === 'Escape') {
      ev.preventDefault();
      close();
    } else if (ev.key === 'Tab' && dialog) {
      const items = [...dialog.querySelectorAll<HTMLElement>('button')];
      const first = items[0];
      const last = items[items.length - 1];
      if (ev.shiftKey && document.activeElement === first) {
        ev.preventDefault();
        last.focus();
      } else if (!ev.shiftKey && document.activeElement === last) {
        ev.preventDefault();
        first.focus();
      }
    }
  }
</script>

{#if overlays.confirm}
  {@const req = overlays.confirm}
  <div class="scrim" in:fadeOut={{ duration: 180 }} out:fadeOut={{ duration: 140 }} onclick={close} aria-hidden="true"></div>
  <!-- 居中层不吃点击，点击空白处落到下面的遮罩上关闭对话框 -->
  <div class="layer">
    <div
      class="dialog"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="confirm-title"
      aria-describedby="confirm-body"
      tabindex="-1"
      bind:this={dialog}
      onkeydown={onKey}
      in:pop={{ y: 18, scale: 0.92, duration: 560 }}
      out:fadeOut={{ duration: 120 }}
    >
      <div class="head">
        <span class="badge" class:danger={req.danger} aria-hidden="true">
          <Icon name={req.danger ? 'alert' : 'info'} size={18} stroke={2.4} />
        </span>
        <h2 id="confirm-title">{req.title}</h2>
      </div>
      <p id="confirm-body">{typeof req.body === 'function' ? req.body() : req.body}</p>
      <div class="actions">
        <button class="btn" bind:this={cancelBtn} onclick={close}>{t('confirm.cancel')}</button>
        <button class="btn {req.danger ? 'danger' : 'primary'}" onclick={ok}>{req.okLabel}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 80;
    background: var(--scrim);
  }
  .layer {
    position: fixed;
    inset: 0;
    z-index: 81;
    display: grid;
    place-items: center;
    padding: 24px;
    pointer-events: none;
  }
  .dialog {
    position: relative;
    width: min(440px, 100%);
    padding: 22px 24px 20px;
    pointer-events: auto;
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--overlay-bg);
    box-shadow: var(--shadow-3);
    outline: none;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 10px;
  }
  /* 轻重一眼可辨：危险操作是珊瑚色，普通确认是丁香色。只是图标，不放吉祥物——
     吉祥物只出现在标题栏、总览状态区、空状态与更新进度这几处 */
  .badge {
    flex: none;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: var(--lilac-soft);
    color: var(--lilac-deep);
  }
  .badge.danger {
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  h2 {
    margin: 0;
    font-size: var(--fs-xl);
    font-weight: 600;
    line-height: 1.25;
  }
  p {
    margin: 0 0 20px;
    color: var(--text-2);
    max-width: 62ch;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }
</style>

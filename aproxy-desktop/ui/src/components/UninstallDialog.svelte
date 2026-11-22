<!--
  卸载一个主题包或扩展包。扩展还要问一句：它的设置（desktop.toml 里的那张表）与数据目录要不要一起删——
  默认保留，以后重装还能接着用；勾上才删。焦点默认在「取消」上，Esc 与点遮罩都是取消。
-->
<script lang="ts">
  import { t, bytes } from '../lib/i18n';
  import { overlays } from '../lib/overlays.svelte';
  import { packages, packageName } from '../lib/packages.svelte';
  import { toFailure } from '../lib/backend/link';
  import { faultText } from '../lib/faults';
  import { pop, fadeOut } from '../lib/motion';
  import type { PackageEntry } from '../lib/backend/contract';
  import Icon from './Icon.svelte';

  let { entry, onclose }: { entry: PackageEntry; onclose: () => void } = $props();

  const name = $derived(packageName(entry));
  let removeData = $state(false);
  let working = $state(false);
  let cancelBtn: HTMLButtonElement | undefined = $state();
  let dialog: HTMLDivElement | undefined = $state();

  $effect(() => {
    queueMicrotask(() => cancelBtn?.focus());
  });

  async function go() {
    working = true;
    try {
      await packages.uninstall(entry.id, entry.kind === 'extension' && removeData);
      overlays.toast(t('set.pkg.uninstalled', { name }), 'lilac');
      onclose();
    } catch (raw) {
      overlays.toast(faultText(toFailure(raw)), 'coral');
      working = false;
    }
  }

  function onKey(ev: KeyboardEvent) {
    if (ev.key === 'Escape' && !working) {
      ev.preventDefault();
      onclose();
    } else if (ev.key === 'Tab' && dialog) {
      const items = [...dialog.querySelectorAll<HTMLElement>('button:not([disabled]), input')];
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

<div class="scrim" in:fadeOut={{ duration: 180 }} out:fadeOut={{ duration: 140 }} onclick={() => !working && onclose()} aria-hidden="true"></div>
<div class="layer">
  <div
    class="dialog"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="un-title"
    aria-describedby="un-body"
    tabindex="-1"
    bind:this={dialog}
    onkeydown={onKey}
    in:pop={{ y: 18, scale: 0.94, duration: 520 }}
    out:fadeOut={{ duration: 120 }}
  >
    <div class="head">
      <span class="badge" aria-hidden="true"><Icon name="x" size={18} stroke={2.4} /></span>
      <h2 id="un-title">{t('set.pkg.uninstallTitle', { name })}</h2>
    </div>
    <p id="un-body">{t(entry.kind === 'theme' ? 'set.pkg.uninstallTheme' : 'set.pkg.uninstallExt')}</p>
    {#if entry.kind === 'extension'}
      <label class="check">
        <input type="checkbox" bind:checked={removeData} />
        <span>{t('set.pkg.removeData', { size: bytes(entry.dataBytes) })}</span>
      </label>
    {/if}
    <div class="actions">
      <button class="btn" bind:this={cancelBtn} onclick={onclose} disabled={working}>{t('confirm.cancel')}</button>
      <button class="btn danger" onclick={go} disabled={working}>{t('set.pkg.uninstall')}</button>
    </div>
  </div>
</div>

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
  .badge {
    flex: none;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: 50%;
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
    margin: 0 0 14px;
    color: var(--text-2);
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    margin: 0 0 18px;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .check input {
    margin-top: 3px;
    accent-color: var(--coral-deep);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }
</style>

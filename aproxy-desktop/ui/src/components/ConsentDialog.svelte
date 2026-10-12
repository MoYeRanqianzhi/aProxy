<!--
  扩展的写权限确认：扩展第一次调用某类写操作（控制实例、修改配置、安装更新）时，后端挂起那次调用，
  发来 consent-request，这里问用户。一次只问一个，先来的先问。

  要让人一眼看清三件事：谁在要（扩展名、版本、装在哪）、要做什么（具体到哪个实例、哪几个键）、
  允许意味着什么（以后这类操作不再问，可以撤销）。打开时焦点落在「拒绝」上：没看清就按回车，结果应该是不做。
-->
<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { ui } from '../lib/state.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import { t } from '../lib/i18n';
  import { pop, fadeOut } from '../lib/motion';
  import type { ExtensionAction, WriteCapability } from '../lib/types';
  import Icon, { type IconName } from './Icon.svelte';

  const req = $derived(store.consents[0]);
  const ext = $derived(req ? store.extension(req.extension) : undefined);
  const name = $derived(ext ? (ext.name[ui.lang] ?? ext.name.en) : (req?.extension ?? ''));

  const CAP_ICON: Record<WriteCapability, IconName> = { control: 'power', config: 'sliders', install: 'download' };

  let denyBtn: HTMLButtonElement | undefined = $state();
  let dialog: HTMLDivElement | undefined = $state();

  $effect(() => {
    if (req) queueMicrotask(() => denyBtn?.focus());
  });

  function target(port: number): string {
    const alias = store.instance(port)?.alias;
    if (!alias) return String(port);
    return ui.lang === 'en' ? `${port} (${alias})` : `${port}（${alias}）`;
  }

  function actionText(a: ExtensionAction): string {
    if (a.kind === 'install') return t('consent.act.install', { version: a.version });
    if (a.kind === 'write-config') return t('consent.act.write-config', { target: target(a.port), keys: a.keys.join(', ') });
    return t(`consent.act.${a.kind}`, { target: target(a.port) });
  }

  async function answer(allow: boolean) {
    if (!req) return;
    // 回答之后这条请求就出队了，req 与 name 跟着变成下一条（或空）：提示要用的文字先取好
    const vars = { name, cap: t(`ext.capName.${req.capability}`) };
    await store.answerConsent(req.id, allow);
    overlays.toast(t(allow ? 'toast.consentAllowed' : 'toast.consentDenied', vars), allow ? 'mint' : 'lilac');
  }

  // 焦点只在对话框内循环；Esc 等于拒绝
  function onKey(ev: KeyboardEvent) {
    if (ev.key === 'Escape') {
      ev.preventDefault();
      answer(false);
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

{#if req}
  <!-- 遮罩点了不关：这是一个必须回答的问题，扩展的那次调用正挂着等答复 -->
  <div class="scrim" in:fadeOut={{ duration: 180 }} out:fadeOut={{ duration: 140 }} aria-hidden="true"></div>
  <div class="layer">
    {#key req.id}
      <div
        class="dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="consent-title"
        aria-describedby="consent-action consent-first"
        tabindex="-1"
        bind:this={dialog}
        onkeydown={onKey}
        in:pop={{ y: 18, scale: 0.92, duration: 560 }}
        out:fadeOut={{ duration: 120 }}
      >
        <div class="who">
          <span class="ext-icon" aria-hidden="true">{name.slice(0, 1)}</span>
          <span class="who-text">
            <strong>{name}</strong>
            {#if ext}<span class="mono">v{ext.version} · {ext.origin}</span>{/if}
          </span>
        </div>

        <h2 id="consent-title">
          <span class="badge" aria-hidden="true"><Icon name={CAP_ICON[req.capability]} size={18} stroke={2.4} /></span>
          {t(`consent.title.${req.capability}`, { name })}
        </h2>
        <p id="consent-action" class="action">{actionText(req.action)}</p>
        <p id="consent-first" class="first"><Icon name="info" size={15} />{t('consent.first', { cap: t(`ext.capName.${req.capability}`) })}</p>

        <div class="actions">
          <button class="btn" bind:this={denyBtn} onclick={() => answer(false)}>{t('consent.deny')}</button>
          <button class="btn primary" onclick={() => answer(true)}>{t('consent.allow')}</button>
        </div>
      </div>
    {/key}
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 82;
    background: var(--scrim);
  }
  .layer {
    position: fixed;
    inset: 0;
    z-index: 83;
    display: grid;
    place-items: center;
    padding: 24px;
    pointer-events: none;
  }
  .dialog {
    width: min(480px, 100%);
    pointer-events: auto;
    overflow: hidden;
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--overlay-bg);
    box-shadow: var(--shadow-3);
    outline: none;
  }
  /* 顶上一条奶黄色的「来访者」栏：这个请求不是用户自己发起的，先说清楚是谁 */
  .who {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 24px;
    background: var(--butter-soft);
  }
  .ext-icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: var(--r-sm);
    background: var(--butter);
    color: var(--on-fill);
    font-size: var(--fs-lg);
    font-weight: 650;
    box-shadow: var(--shadow-1);
  }
  .who-text {
    min-width: 0;
    display: grid;
    gap: 1px;
  }
  .who-text .mono {
    overflow: hidden;
    color: var(--text-2);
    font-size: var(--fs-xs);
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 18px 24px 8px;
    font-size: var(--fs-xl);
    font-weight: 600;
    line-height: 1.25;
  }
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
  .action {
    margin: 0 24px 12px 72px;
    color: var(--text);
    font-weight: 550;
  }
  .first {
    display: flex;
    gap: 8px;
    margin: 0 24px 20px;
    padding: 10px 12px;
    border-radius: var(--r-md);
    background: var(--card-2-bg);
    color: var(--text-2);
    font-size: var(--fs-sm);
    line-height: 1.5;
  }
  .first :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    padding: 0 24px 20px;
  }
</style>

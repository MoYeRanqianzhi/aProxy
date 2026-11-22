<!--
  把扩展的 render(el, ctx) 画进一个容器：扩展加的页面与挂进来的面板都经这里。
  扩展的函数在 untrack 里调用：它读界面数据不会让这里跟着重跑，要更新由它自己订阅（api.*.onChange）。
  ctx 变了（例如实例页换了一个实例）才重新画一次。抛出的异常接住，显示在容器上方，不影响页面的其余部分。
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { t } from '../lib/i18n';
  import { errorText } from '../lib/extensions/host.svelte';
  import Icon from './Icon.svelte';

  let {
    render,
    ctx = {},
    name,
  }: {
    render: (el: HTMLElement, ctx: Record<string, unknown>) => void | (() => void);
    ctx?: Record<string, unknown>;
    /** 出错时说是哪个扩展 */
    name: string;
  } = $props();

  let box: HTMLDivElement | undefined = $state();
  let error = $state<string | null>(null);

  $effect(() => {
    const el = box;
    const c = { ...ctx };
    if (!el) return;
    let cleanup: void | (() => void);
    untrack(() => {
      error = null;
      try {
        cleanup = render(el, c);
      } catch (err) {
        console.warn(`[extension ${name}] render`, err);
        error = errorText(err);
      }
    });
    return () => {
      try {
        if (typeof cleanup === 'function') cleanup();
      } catch (err) {
        console.warn(`[extension ${name}] cleanup`, err);
      }
      el.replaceChildren();
    };
  });
</script>

{#if error}
  <div class="ext-error" role="alert">
    <Icon name="alert" size={15} />
    <span>{t('ext.renderFailed', { name, message: error })}</span>
  </div>
{/if}
<div class="ext-root" bind:this={box}></div>

<style>
  .ext-error {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin-bottom: 10px;
    padding: 8px 12px;
    border-radius: var(--r-md);
    background: var(--coral-soft);
    color: var(--text);
    font-size: var(--fs-sm);
  }
  .ext-error :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .ext-root:empty {
    display: none;
  }
</style>

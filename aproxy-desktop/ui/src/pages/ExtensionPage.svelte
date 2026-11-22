<!--
  扩展加的一页（api.ui.addPage）。页面的内容全由扩展画；这里只管：扩展还没加载完时说一声，
  加载完了却没有这一页（扩展停用了、出错了、版本里没有这一页）时给出去处。
-->
<script lang="ts">
  import { t } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { host } from '../lib/extensions/host.svelte';
  import { packages, packageName } from '../lib/packages.svelte';
  import ExtRender from '../components/ExtRender.svelte';
  import Icon from '../components/Icon.svelte';

  const page = $derived(host.page(ui.extPage));
  const extId = $derived(ui.extPage.split('/')[0] ?? '');
  const entry = $derived(packages.get(extId));
  const run = $derived(host.states[extId]);
  /** 扩展还在加载（或包的列表还没读到）：先别说「没有这一页」 */
  const waiting = $derived(!packages.loaded || run?.state === 'loading' || (!!entry?.enabled && !run));
</script>

{#if page}
  <!-- 换一页就换一个容器：上一页留下的东西随它的 cleanup 一起走 -->
  {#key page.key}
    <ExtRender render={page.spec.render} name={entry ? packageName(entry) : extId} />
  {/key}
{:else if waiting}
  <p class="quiet"><Icon name="hourglass" size={16} />{t('ext.page.loading')}</p>
{:else}
  <div class="missing">
    <h1>{t('ext.page.missing')}</h1>
    <p>{entry ? t('ext.page.missingBody', { name: packageName(entry) }) : t('ext.page.missingGone')}</p>
    <button class="btn" onclick={() => ui.go('settings')}><Icon name="puzzle" size={15} />{t('ext.page.toSettings')}</button>
  </div>
{/if}

<style>
  .quiet {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-2);
  }
  .missing {
    max-width: 52ch;
  }
  .missing h1 {
    margin: 0 0 8px;
    font-size: var(--fs-2xl);
    font-weight: 620;
  }
  .missing p {
    margin: 0 0 16px;
    color: var(--text-2);
  }
</style>

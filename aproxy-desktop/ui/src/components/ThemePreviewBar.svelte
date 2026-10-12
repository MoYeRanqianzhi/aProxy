<!--
  主题预览条：导入主题时可以先让整个窗口穿上它看看。预览期间这一条压在内容区最上方，
  切到别的页面也一直在，提醒「这还不是你的主题」，并随时可以应用或取消。
  它占内容区的一行而不是浮在上面：浮层会挡住页面顶部或撞上底部的轻提示。
-->
<script lang="ts">
  import { ui } from '../lib/state.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import { t } from '../lib/i18n';
  import Icon from './Icon.svelte';

  function apply() {
    const spec = ui.themePreview;
    if (!spec) return;
    ui.themeSpec = spec;
    ui.themePreview = null;
    overlays.toast(t('toast.themeApplied', { name: spec.name }));
  }
</script>

{#if ui.themePreview}
  <div class="bar" role="status">
    <Icon name="eye" size={16} />
    <span class="txt">
      <strong>{t('theme.previewing', { name: ui.themePreview.name })}</strong>
      <span class="hint">{t('theme.previewHint')}</span>
    </span>
    <button class="btn sm ghost" onclick={() => (ui.themePreview = null)}>{t('confirm.cancel')}</button>
    <button class="btn sm primary" onclick={apply}>{t('set.theme.apply')}</button>
  </div>
{/if}

<style>
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 16px 9px 22px;
    background: var(--lilac-soft);
    color: var(--lilac-deep);
    border-bottom: var(--line-w) solid var(--line);
  }
  .txt {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px 10px;
  }
  strong {
    font-weight: 600;
  }
  .hint {
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
</style>

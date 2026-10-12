<!--
  【仅原型】审核工具条：方便用户切换主题、语言、配色强度、Mica、减少动效、场景与页面。
  故意做成与应用完全不同的样子（石板灰底、黄色虚线框、系统字体），一眼看出它不是应用的一部分。
  平时收成左侧边缘的一个小标签；Alt+R 可以整个藏起来或找回来。
-->
<script lang="ts">
  import { ui, type Page } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import { mockControls } from '../lib/backend/mock';
  import { t } from '../lib/i18n';
  import type { Scenario } from '../lib/state.svelte';

  type Opt<T> = { v: T; label: string };

  const pages = $derived<Opt<Page>[]>([
    { v: 'overview', label: t('nav.overview') },
    { v: 'instance', label: t('nav.instances') },
    { v: 'logs', label: t('nav.logs') },
    { v: 'config', label: t('nav.config') },
    { v: 'update', label: t('nav.update') },
    { v: 'settings', label: t('nav.settings') },
    { v: 'mascots', label: t('proto.mascotLab') },
  ]);

  // 开发期给审核与截图脚本用的句柄，生产构建里不存在。
  // 放在这里而不是 App：只有 proto/ 下的原型件可以直接碰 mock 后端，应用本身只认 Backend 接口
  if (import.meta.env.DEV) {
    (window as unknown as Record<string, unknown>).__aproxyProto = { ui, store, overlays, mock: mockControls };
  }

  // 场景只存在于 mock 后端里：它是原型审核用的开关，不属于 Backend 接口
  function setScenario(s: Scenario) {
    mockControls.setScenario(s);
  }
</script>

{#if !ui.toolbarHidden}
  <aside class="proto" class:open={ui.toolbarOpen} aria-label={t('proto.title')}>
    {#if !ui.toolbarOpen}
      <button class="tab" onclick={() => (ui.toolbarOpen = true)} aria-label={t('proto.open')}>
        <span>{t('proto.title')}</span>
      </button>
    {:else}
      <div class="panel">
        <header>
          <div>
            <strong>{t('proto.title')}</strong>
            <small>{t('proto.note')}</small>
          </div>
          <button class="close" onclick={() => (ui.toolbarOpen = false)} aria-label={t('proto.collapse')}>×</button>
        </header>

        <!-- 每行的取值类型各不相同（字符串、布尔），setter 用 any 接住，调用处的箭头函数自带具体类型 -->
        {#snippet row(title: string, opts: Opt<unknown>[], current: unknown, set: (v: any) => void)}
          <div class="row">
            <span class="title">{title}</span>
            <div class="opts">
              {#each opts as o (o.label)}
                <button class:on={o.v === current} onclick={() => set(o.v)}>{o.label}</button>
              {/each}
            </div>
          </div>
        {/snippet}

        {@render row(t('proto.theme'), [{ v: 'light', label: t('set.light') }, { v: 'dark', label: t('set.dark') }], ui.variant, (v) => (ui.modePref = v))}
        {@render row(t('proto.lang'), [{ v: 'zh-CN', label: '中文' }, { v: 'en', label: 'English' }], ui.lang, (v) => (ui.langPref = v))}
        {@render row(
          t('proto.palette'),
          [
            { v: 'soft', label: t('proto.palette.soft') },
            { v: 'standard', label: t('proto.palette.standard') },
            { v: 'vivid', label: t('proto.palette.vivid') },
          ],
          ui.palette,
          (v) => (ui.palette = v),
        )}
        {@render row(t('proto.mica'), [{ v: false, label: t('proto.off') }, { v: true, label: t('proto.on') }], ui.micaEnabled, (v) => (ui.micaEnabled = v))}
        {@render row(
          t('proto.micaSystem'),
          [
            { v: 'available', label: t('proto.micaSystem.available') },
            { v: 'unsupported', label: t('proto.micaSystem.unsupported') },
            { v: 'transparency-off', label: t('proto.micaSystem.transparencyOff') },
          ],
          ui.micaSupport,
          (v) => (ui.micaSupport = v),
        )}
        {@render row(t('proto.motion'), [{ v: false, label: t('proto.off') }, { v: true, label: t('proto.on') }], ui.reducedMotion, (v) => (ui.reducedMotionOverride = v))}
        {@render row(
          t('proto.scenario'),
          [
            { v: 'normal', label: t('proto.scenario.normal') },
            { v: 'empty', label: t('proto.scenario.empty') },
            { v: 'error', label: t('proto.scenario.error') },
          ],
          ui.scenario,
          setScenario,
        )}
        {@render row(t('proto.jump'), pages, ui.page, (v) => ui.go(v))}

        <!-- 模拟流量里「长时间重试后救回」要等一阵才碰得上，审核时直接触发一次看总览的庆祝 -->
        <button class="hide" onclick={() => mockControls.celebrate()}>{t('proto.celebrate')}</button>
        <!-- 扩展第一次做写操作时的确认框：让「配置同步」先请求改配置、再请求重启 -->
        <button
          class="hide"
          onclick={() => {
            if (mockControls.extensionWrite() === 'all-granted') overlays.toast(t('proto.extAllGranted'), 'lilac');
          }}>{t('proto.extWrite')}</button
        >
        <button class="hide" onclick={() => (ui.toolbarHidden = true)}>{t('proto.hide')}</button>
      </div>
    {/if}
  </aside>
{/if}

<style>
  /* 刻意不用应用的令牌与字体：它必须看起来像外来的工具 */
  .proto {
    position: fixed;
    left: 0;
    bottom: 140px;
    z-index: 500;
    font-family: 'Segoe UI', system-ui, sans-serif;
    font-size: 12px;
    color: #e9edf5;
  }
  .tab {
    writing-mode: vertical-rl;
    padding: 12px 5px;
    border: 1.5px dashed #f5c542;
    border-left: 0;
    border-radius: 0 8px 8px 0;
    background: #222a38;
    color: #f5c542;
    font: inherit;
    font-weight: 600;
    letter-spacing: 0.08em;
    opacity: 0.85;
  }
  .tab:hover {
    opacity: 1;
  }
  .panel {
    width: 312px;
    margin-left: 8px;
    padding: 12px 14px 12px;
    border: 1.5px dashed #f5c542;
    border-radius: 10px;
    background: #222a38;
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.35);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 8px;
  }
  header strong {
    display: block;
    font-size: 13px;
    color: #f5c542;
  }
  header small {
    color: #9aa6ba;
  }
  .close {
    border: 0;
    background: transparent;
    color: #9aa6ba;
    font-size: 18px;
    line-height: 1;
  }
  .row {
    display: grid;
    grid-template-columns: 84px 1fr;
    align-items: center;
    gap: 8px;
    padding: 5px 0;
    border-top: 1px solid #333d50;
  }
  .title {
    color: #b7c1d3;
  }
  .opts {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .opts button {
    padding: 3px 8px;
    border: 1px solid #3c475c;
    border-radius: 4px;
    background: #2b3446;
    color: #d8deea;
    font: inherit;
  }
  .opts button:hover {
    border-color: #5d6b85;
  }
  .opts button.on {
    background: #f5c542;
    border-color: #f5c542;
    color: #1d2330;
    font-weight: 600;
  }
  .hide {
    width: 100%;
    margin-top: 8px;
    padding: 5px;
    border: 1px solid #3c475c;
    border-radius: 4px;
    background: transparent;
    color: #9aa6ba;
    font: inherit;
  }
</style>

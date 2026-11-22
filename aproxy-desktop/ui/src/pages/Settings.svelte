<!--
  设置：外观（语言、模式、配色强度、吉祥物、Mica）、后台（看护者、开机自启）、主题与扩展（PackagesPanel）、关于。

  Mica 的开关有三种样子：可用（开或关）；系统不支持（禁用并写明原因）；
  系统关了透明效果（可以开，但提示会显示为纯色）。桌面窗口里能力由 Rust 判定（见 lib/shell.ts），
  开关真的切换窗口材质；浏览器里从审核工具条切换。
-->
<script lang="ts">
  import { t } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { toFailure } from '../lib/backend/link';
  import { faultText } from '../lib/faults';
  import { overlays, copyText } from '../lib/overlays.svelte';
  import { pop } from '../lib/motion';
  import { mascots, mascotName, NO_MASCOT } from '../lib/mascot/registry.svelte';
  import Icon from '../components/Icon.svelte';
  import Segmented from '../components/Segmented.svelte';
  import Switch from '../components/Switch.svelte';
  import MascotView from '../components/MascotView.svelte';
  import PackagesPanel from '../components/PackagesPanel.svelte';
  import { packages } from '../lib/packages.svelte';

  const micaUnsupported = $derived(ui.micaSupport === 'unsupported');

  /**
   * 诊断信息：贴进问题报告里用。只放版本、系统、实例数与界面状态。
   * home 路径里常带着用户名，所以不原样写出，只说它是不是默认位置（~/.aproxy）。
   */
  async function copyDiag() {
    const a = store.about;
    const reg = store.autostart;
    const lines = [
      a ? `aProxy ${a.version} (${a.variant})` : 'aProxy (version unknown)',
      ui.windowsBuild === null ? 'platform: not Windows' : `Windows build ${ui.windowsBuild}`,
      `home: ${a ? (/[\\/]\.aproxy$/.test(a.home) ? 'default' : 'custom') : 'unknown'}`,
      `instances: ${store.cards.length}`,
      `watchdog: ${store.settings ? store.watchdogOn : 'unknown'}`,
      `autostart: ${reg ? `${reg.enabled}${reg.issue ? ` (${reg.issue})` : ''}` : 'unknown'}`,
      `mica: ${ui.micaSupport}`,
      `mode: ${ui.variant}`,
      `theme: ${ui.themeId === 'builtin' ? 'builtin' : 'package'}`,
      `extensions: ${packages.active.length} enabled of ${packages.extensions.length}`,
    ];
    await copyText(lines.join('\n'));
    overlays.toast(t('toast.copied'));
  }

  // ---------------------------------------------------------------------------
  // 后台：看护者与开机自启
  // ---------------------------------------------------------------------------

  // 开关组件按下就先翻过去。写失败时要翻回来，所以这里各留一份本地值：
  // 平时由 store 里读回的值驱动（写成功、别处改了都会带过来），写失败时在 finally 里拉回 store 的值。
  let watchdog = $state(false);
  let autostart = $state(false);
  let busy = $state<'watchdog' | 'autostart' | null>(null);
  $effect(() => {
    watchdog = store.watchdogOn;
  });
  $effect(() => {
    autostart = store.autostart?.enabled ?? false;
  });

  async function setWatchdog(on: boolean) {
    busy = 'watchdog';
    try {
      await store.setWatchdog(on);
    } catch (raw) {
      overlays.toast(faultText(toFailure(raw)), 'coral');
    } finally {
      watchdog = store.watchdogOn;
      busy = null;
    }
  }

  async function setAutostart(on: boolean) {
    busy = 'autostart';
    try {
      await store.setAutostart(on);
    } catch (raw) {
      overlays.toast(faultText(toFailure(raw)), 'coral');
    } finally {
      autostart = store.autostart?.enabled ?? false;
      busy = null;
    }
  }

  /** settings.json 里写错的地方：这时开关显示的不一定是文件里想写的值，要提醒一句 */
  const settingsProblems = $derived(store.settings?.diagnostics.filter((d) => d.severity === 'error').length ?? 0);

  // ---------------------------------------------------------------------------
  // 吉祥物
  // ---------------------------------------------------------------------------

  const choices = $derived([
    ...mascots.entries.map((e) => ({ id: e.id, name: mascotName(e), custom: e.source === 'extension' })),
    { id: NO_MASCOT, name: t('set.mascot.none'), custom: false },
  ]);
  let choiceEls: HTMLButtonElement[] = $state([]);

  // 单选组的键盘约定：只有选中的那个在 Tab 序列里，方向键换选择
  function onChoiceKey(ev: KeyboardEvent, i: number) {
    const d = ev.key === 'ArrowRight' || ev.key === 'ArrowDown' ? 1 : ev.key === 'ArrowLeft' || ev.key === 'ArrowUp' ? -1 : 0;
    if (!d) return;
    ev.preventDefault();
    const next = (i + d + choices.length) % choices.length;
    ui.mascotId = choices[next].id;
    choiceEls[next]?.focus();
  }

</script>

<h1>{t('nav.settings')}</h1>

{#if store.reset.length}
  <!-- 置顶的文件坏了、被重置成默认：用户之前的置顶没了，得说一声 -->
  <div class="reset-note" role="status">
    <Icon name="alert" size={16} />
    <p>{store.reset.map((file) => t(`set.reset.${file}`)).join(ui.lang === 'en' ? ' ' : '')}</p>
  </div>
{/if}
{#each store.tomlIssues as issue, i (i)}
  <!-- desktop.toml 是可以手改的文件：改坏了要说清是哪一行，改好之前界面沿用原来的设置 -->
  <div class="reset-note" role="status">
    <Icon name="alert" size={16} />
    <p>{t(`set.toml.${issue.kind}`, { line: issue.line ?? '?', column: issue.column ?? '?', key: issue.key ?? '?' })}</p>
  </div>
{/each}

<section class="panel group">
  <h2><Icon name="sun" size={17} />{t('set.appearance')}</h2>
  <div class="row">
    <div class="txt"><span class="name">{t('set.lang')}</span></div>
    <Segmented
      options={[
        { value: 'system', label: t('set.system'), icon: 'monitor' },
        { value: 'zh-CN', label: '中文' },
        { value: 'en', label: 'English' },
      ]}
      bind:value={ui.langPref}
      label={t('set.lang')}
    />
  </div>
  <div class="row">
    <div class="txt"><span class="name">{t('set.mode')}</span></div>
    <Segmented
      options={[
        { value: 'system', label: t('set.system'), icon: 'monitor' },
        { value: 'light', label: t('set.light'), icon: 'sun' },
        { value: 'dark', label: t('set.dark'), icon: 'moon' },
      ]}
      bind:value={ui.modePref}
      label={t('set.mode')}
    />
  </div>

  <div class="row">
    <div class="txt">
      <span class="name">{t('set.palette')}</span>
      <span class="desc">{t('set.palette.desc')}</span>
    </div>
    <Segmented
      options={[
        { value: 'soft', label: t('set.palette.soft') },
        { value: 'standard', label: t('set.palette.standard') },
        { value: 'vivid', label: t('set.palette.vivid') },
      ]}
      bind:value={ui.palette}
      label={t('set.palette')}
    />
  </div>

  <div class="row stack">
    <div class="txt">
      <span class="name">{t('set.mascot')}</span>
      <span class="desc">{t('set.mascot.desc')}</span>
    </div>
    <div class="mascots" role="radiogroup" aria-label={t('set.mascot')}>
      {#each choices as c, i (c.id)}
        {@const on = ui.mascotId === c.id}
        <button
          type="button"
          role="radio"
          aria-checked={on}
          tabindex={on ? 0 : -1}
          class="choice"
          class:on
          bind:this={choiceEls[i]}
          onclick={() => (ui.mascotId = c.id)}
          onkeydown={(ev) => onChoiceKey(ev, i)}
        >
          <span class="art">
            {#if c.id === NO_MASCOT}
              <span class="none-art"><Icon name="eyeOff" size={22} /></span>
            {:else}
              <!-- 选择器里一排都动会很吵：只让选中的那个动 -->
              <MascotView id={c.id} mood="idle" size="small" px={58} still={!on} />
            {/if}
          </span>
          <span class="choice-name">{c.name}</span>
          {#if c.custom}<span class="chip pink tiny">{t('set.mascot.custom')}</span>{/if}
          {#if on}<span class="tick" in:pop={{ y: 0, scale: 0.4, duration: 380 }}><Icon name="check" size={12} stroke={3} /></span>{/if}
        </button>
      {/each}
    </div>
  </div>

  <div class="row" class:disabled={micaUnsupported}>
    <div class="txt">
      <span class="name"><Icon name="layers" size={15} />{t('set.mica')}</span>
      <span class="desc">{t('set.mica.desc')}</span>
      {#if micaUnsupported}
        <span class="note muted" in:pop={{ y: -4, scale: 1, duration: 320 }}
          ><Icon name="info" size={14} />{ui.windowsBuild === null
            ? t('set.mica.notWindows')
            : t('set.mica.unsupported', { build: ui.windowsBuild })}</span
        >
      {:else if ui.micaSupport === 'transparency-off'}
        <span class="note warn" in:pop={{ y: -4, scale: 1, duration: 320 }}><Icon name="alert" size={14} />{t('set.mica.transparencyOff')}</span>
      {/if}
    </div>
    <Switch bind:checked={ui.micaEnabled} disabled={micaUnsupported} label={t('set.mica')} />
  </div>
</section>

<section class="panel group">
  <h2><Icon name="shield" size={17} />{t('set.background')}</h2>
  <div class="row">
    <div class="txt">
      <span class="name">{t('set.watchdog')}</span>
      <span class="desc">{t('set.watchdog.desc')}</span>
      {#if settingsProblems}
        <span class="note warn"><Icon name="alert" size={14} />{t('set.settingsProblems', { n: settingsProblems })}</span>
      {/if}
    </div>
    <!-- 看护者写在 settings.json 里，所有实例共用；开机自启是系统里的登录项，每次都现查 -->
    <Switch bind:checked={watchdog} disabled={!store.settings || busy === 'watchdog'} onchange={setWatchdog} label={t('set.watchdog')} />
  </div>
  <div class="row">
    <div class="txt">
      <span class="name">{t('set.autostart')}</span>
      <span class="desc">{t('set.autostart.desc')}</span>
      {#if store.autostart?.issue}
        <span class="note warn"><Icon name="alert" size={14} />{t(`set.autostart.${store.autostart.issue}`)}</span>
      {/if}
    </div>
    <Switch bind:checked={autostart} disabled={!store.autostart || busy === 'autostart'} onchange={setAutostart} label={t('set.autostart')} />
  </div>
</section>

<PackagesPanel />

<section class="panel group about">
  <h2><Icon name="info" size={17} />{t('set.about')}</h2>
  <div class="about-body">
    <!-- 「关于」里展示的是软件图标（默认吉祥物的静态大图标），与界面里选了哪个吉祥物、是否显示无关 -->
    <MascotView id="tumbler" mood="idle" icon={84} />
    <div class="about-text">
      <div class="about-name">
        <strong>aProxy</strong>
        {#if store.about}
          <span class="chip">v{store.about.version}</span>
          <span class="chip lilac"><Icon name="monitor" size={12} stroke={2.4} />{t(`up.variant.${store.about.variant}`)}</span>
        {/if}
      </div>
      <p>{t('set.about.desc')}</p>
      <div class="links">
        <button class="btn sm" onclick={() => ui.go('update')}><Icon name="download" size={14} />{t('set.checkUpdate')}</button>
        <button class="btn sm" onclick={copyDiag}><Icon name="copy" size={14} />{t('set.copyDiag')}</button>
        <span class="license">{t('set.license')}</span>
      </div>
    </div>
  </div>
</section>

<style>
  h1 {
    margin: 0 0 18px;
    font-size: var(--fs-2xl);
    font-weight: 620;
  }
  .group {
    max-width: 820px;
    margin-bottom: 18px;
    padding: 4px 20px 6px;
  }
  .group h2 {
    display: flex;
    align-items: center;
    gap: 9px;
    margin: 14px 0 6px;
    font-size: var(--fs-lg);
    font-weight: 620;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 24px;
    padding: 14px 0;
    border-top: var(--line-w-thin) dashed var(--line-soft);
  }
  .group h2 + .row {
    border-top: 0;
  }
  /* 说明在上、控件在下的行：主题与吉祥物的控件太宽，放不进右侧 */
  .row.stack {
    flex-direction: column;
    align-items: stretch;
    gap: 12px;
  }
  .txt {
    display: grid;
    gap: 3px;
    max-width: 56ch;
  }
  .name {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-weight: 600;
  }
  .desc {
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .row.disabled .name,
  .row.disabled .desc {
    color: var(--text-3);
  }
  .reset-note {
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: 820px;
    margin-bottom: 16px;
    padding: 10px 14px;
    border-radius: var(--r-md);
    background: var(--butter-soft);
    color: var(--text);
  }
  .reset-note p {
    flex: 1;
    margin: 0;
  }
  .note {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    margin-top: 6px;
    padding: 8px 12px;
    border-radius: var(--r-md);
    font-size: var(--fs-sm);
    line-height: 1.5;
  }
  .note :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .note.muted {
    background: var(--card-2-bg);
    color: var(--text-2);
  }
  .note.warn {
    background: var(--butter-soft);
    color: var(--text);
  }
  .muted {
    color: var(--text-2);
  }

  /* ---------- 吉祥物 ---------- */
  .mascots {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
  }
  .choice {
    position: relative;
    display: grid;
    justify-items: center;
    align-content: start;
    gap: 6px;
    width: 112px;
    padding: 12px 8px 10px;
    border-radius: var(--r-md);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    box-shadow: var(--shadow-1);
    transition:
      transform var(--dur-mid) var(--ease-spring),
      box-shadow var(--dur-mid) var(--ease-soft),
      background-color var(--dur-fast) linear;
  }
  .choice:hover {
    transform: translateY(var(--lift));
    box-shadow: var(--shadow-2-hover);
  }
  .choice.on {
    border-color: transparent;
    background: var(--lilac-soft);
    box-shadow: 0 0 0 2px var(--lilac), var(--shadow-1);
  }
  .art {
    display: grid;
    place-items: center;
    width: 58px;
    height: 58px;
  }
  .none-art {
    display: grid;
    place-items: center;
    width: 48px;
    height: 48px;
    border-radius: 50%;
    border: 1.5px dashed var(--line-dash);
    color: var(--text-3);
  }
  .choice-name {
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .chip.tiny {
    height: 19px;
    padding: 0 7px;
    font-size: var(--fs-xs);
  }
  .tick {
    position: absolute;
    top: 7px;
    right: 7px;
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--lilac);
    color: var(--on-fill);
  }

  /* ---------- 关于 ---------- */
  .about-body {
    display: flex;
    gap: 20px;
    align-items: center;
    padding: 8px 0 14px;
  }
  .about-name {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .about-name strong {
    font-size: var(--fs-xl);
    font-weight: 640;
  }
  .about-text p {
    margin: 6px 0 12px;
    color: var(--text-2);
  }
  .links {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .license {
    color: var(--text-3);
    font-size: var(--fs-sm);
    margin-left: 4px;
  }
</style>

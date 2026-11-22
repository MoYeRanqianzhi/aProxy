<!--
  设置页里的「主题与扩展」：安装入口、已装的主题（点一下换上）、已装的扩展（开关、详情、卸载）。

  只有两种包（用户 2026-11-23 定）：主题只换外观；扩展是在窗口里运行的代码，也可以改外观。
  列表上每个包都带着「从哪来」的标签：安装时核对过的、闭源的、没核对成的、手工拷进来没经过检查的，
  让人回头也看得出当初装的是什么。扩展的运行状态（运行中、出错了）来自扩展宿主。
-->
<script lang="ts">
  import { t, bytes, fmtNum } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import { packages, packageName, textOf } from '../lib/packages.svelte';
  import { host } from '../lib/extensions/host.svelte';
  import { toFailure } from '../lib/backend/link';
  import { faultText } from '../lib/faults';
  import { BUILTIN_THEME, parseTheme } from '../lib/theme';
  import { pop } from '../lib/motion';
  import type { GithubFacts, PackageEntry, WriteKind } from '../lib/backend/contract';
  import type { ThemeColors, ThemeSpec } from '../lib/types';
  import Icon, { type IconName } from './Icon.svelte';
  import Switch from './Switch.svelte';
  import UninstallDialog from './UninstallDialog.svelte';

  let fileInput: HTMLInputElement | undefined = $state();
  let openExt = $state<string | null>(null);
  let removing = $state<PackageEntry | null>(null);
  let busy = $state<string | null>(null);

  function pick() {
    if (packages.busy) return;
    fileInput?.click();
  }

  function onPick(ev: Event) {
    const input = ev.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    // 清空，下次选同一个文件也会触发 change
    input.value = '';
    if (file) void packages.beginFile(file);
  }

  // ---------------------------------------------------------------------------
  // 主题
  // ---------------------------------------------------------------------------

  function specOf(e: PackageEntry): ThemeSpec | null {
    if (!e.theme || e.issue) return null;
    const p = parseTheme(e.theme);
    return p.ok ? p.spec : null;
  }

  /** 小样用的颜色：当前深浅模式下这个主题的种子色，缺的用内置主题的补上 */
  function colorsOf(spec: ThemeSpec | null): Required<ThemeColors> {
    const base = BUILTIN_THEME[ui.variant] as Required<ThemeColors>;
    return { ...base, ...(spec?.[ui.variant] ?? {}) };
  }

  function useTheme(id: string, label: string) {
    if (ui.themeId === id) return;
    ui.themeId = id;
    overlays.toast(t('set.pkg.themeOn', { name: label }));
  }

  // ---------------------------------------------------------------------------
  // 扩展
  // ---------------------------------------------------------------------------

  async function toggle(e: PackageEntry, on: boolean) {
    busy = e.id;
    try {
      await packages.setEnabled(e.id, on);
      overlays.toast(t(on ? 'set.pkg.on' : 'set.pkg.off', { name: packageName(e) }), on ? 'mint' : 'lilac');
    } catch (raw) {
      overlays.toast(faultText(toFailure(raw)), 'coral');
    } finally {
      busy = null;
    }
  }

  /** 列表上的来源标签：安装时核对的结果，一眼分出放心的与要留心的 */
  function provenance(e: PackageEntry): { text: string; tone: string; icon: IconName; title?: string } {
    if (!e.source) return { text: t('set.pkg.manual'), tone: 'butter', icon: 'alert', title: t('set.pkg.manualTitle') };
    const g: GithubFacts | null = e.github;
    if (!g) return { text: t('set.pkg.unverified'), tone: 'butter', icon: 'shield' };
    if (g.state === 'no-repository') return { text: t('set.pkg.closed'), tone: 'coral', icon: 'lock' };
    if (g.state === 'checked' && g.release.state === 'match') {
      return { text: t('set.pkg.verified', { stars: compact(g.repo.stars) }), tone: 'mint', icon: 'check' };
    }
    if (g.state === 'checked' && g.release.state === 'mismatch') return { text: t('set.pkg.mismatch'), tone: 'coral', icon: 'shieldOff' };
    return { text: t('set.pkg.unverified'), tone: 'butter', icon: 'shield' };
  }

  /** 1.2k 这样的短写法：列表上的标签放不下长数字 */
  function compact(n: number): string {
    return new Intl.NumberFormat(ui.lang, { notation: 'compact', maximumFractionDigits: 1 }).format(n);
  }

  function status(e: PackageEntry): { text: string; tone: string } {
    if (e.issue || !e.manifest) return { text: t('set.pkg.broken'), tone: 'coral' };
    if (!e.enabled) return { text: t('set.pkg.disabled'), tone: '' };
    const s = host.states[e.id];
    if (!s || s.state === 'loading') return { text: t('set.pkg.loading'), tone: 'sky' };
    if (s.state === 'failed') return { text: t('set.pkg.failed'), tone: 'coral' };
    return { text: t('set.pkg.running'), tone: 'mint' };
  }

  /** 安装时核对的那一行字 */
  function checkText(e: PackageEntry): string {
    const g = e.github;
    if (!e.source) return t('set.pkg.f.noRecord');
    if (!g) return t('set.pkg.f.notChecked');
    switch (g.state) {
      case 'no-repository':
        return t('set.pkg.closed');
      case 'not-github':
        return t('install.source.notGithub');
      case 'unreachable':
        return t('install.source.unreachable');
      case 'repo-not-found':
        return t('install.source.repoMissing');
      case 'checked': {
        const r = g.release;
        const head =
          r.state === 'match'
            ? t('install.source.match', { tag: r.tag })
            : r.state === 'mismatch'
              ? t('install.source.mismatch', { tag: r.tag })
              : r.state === 'not-a-file'
                ? t('install.source.dir')
                : t('install.source.unverified');
        return `${head} · ${t('install.source.stars', { n: fmtNum(g.repo.stars) })}`;
      }
    }
  }

  const day = $derived(new Intl.DateTimeFormat(ui.lang, { year: 'numeric', month: 'long', day: 'numeric' }));

  function writeText(w: WriteKind): string {
    return t(`install.write.${w}`);
  }
</script>

<section class="panel group">
  <h2><Icon name="puzzle" size={17} />{t('set.pkg')}</h2>
  <p class="group-desc">{t('set.pkg.desc')}</p>

  <div class="install-bar">
    <button class="btn primary sm" onclick={pick} disabled={packages.busy}><Icon name="download" size={14} />{t('set.pkg.install')}</button>
    <span class="hint">{t('set.pkg.dropHint')}</span>
    <input class="file" type="file" accept=".zip,application/zip" bind:this={fileInput} onchange={onPick} tabindex="-1" aria-hidden="true" />
  </div>

  {#if packages.readFault}
    <p class="note warn"><Icon name="alert" size={14} />{faultText(packages.readFault)}</p>
  {/if}

  <h3>{t('set.pkg.themes')}</h3>
  <div class="themes" role="radiogroup" aria-label={t('set.pkg.themes')}>
    {@render themeCard('builtin', t('set.pkg.builtin'), 'aProxy', BUILTIN_THEME, null)}
    {#each packages.themes as e (e.id)}
      {@render themeCard(e.id, packageName(e), e.manifest?.author ?? '', specOf(e), e)}
    {/each}
  </div>

  <h3>{t('set.pkg.extensions')}</h3>
  {#if packages.extensions.length === 0}
    <p class="quiet">{t(packages.loaded ? 'set.pkg.noExtensions' : 'set.pkg.reading')}</p>
  {:else}
    <ul class="exts">
      {#each packages.extensions as e (e.id)}
        {@const open = openExt === e.id}
        {@const name = packageName(e)}
        {@const st = status(e)}
        {@const pv = provenance(e)}
        {@const run = host.states[e.id]}
        <li class="ext" class:open class:off={!e.enabled}>
          <div class="ext-row">
            <button class="ext-main" onclick={() => (openExt = open ? null : e.id)} aria-expanded={open} aria-controls="ext-{e.id}" aria-label={t('set.pkg.details', { name })}>
              <span class="ext-icon">{name.slice(0, 1).toUpperCase()}</span>
              <span class="ext-text">
                <span class="ext-name">{name}{#if e.manifest}<span class="ver">v{e.manifest.version}</span>{/if}</span>
                <span class="ext-desc">{textOf(e.manifest?.description, t('set.pkg.brokenDesc'))}</span>
              </span>
              <span class="tags">
                <span class="chip {pv.tone}" title={pv.title}><Icon name={pv.icon} size={12} stroke={2.4} />{pv.text}</span>
                <span class="chip {st.tone}">{st.text}</span>
              </span>
              <span class="chev"><Icon name="chevronDown" size={16} /></span>
            </button>
            <Switch
              checked={e.enabled}
              disabled={!!e.issue || !e.manifest || busy === e.id}
              onchange={(v) => toggle(e, v)}
              label={t('set.pkg.toggle', { name })}
            />
          </div>

          {#if open}
            <div class="ext-detail" id="ext-{e.id}" in:pop={{ y: -6, scale: 1, duration: 360 }}>
              {#if run?.state === 'failed'}
                <p class="note coral"><Icon name="alert" size={14} />{t('set.pkg.failedWhy', { message: run.message })}</p>
              {/if}
              {#if e.issue || !e.manifest}
                <p class="note coral"><Icon name="alert" size={14} />{t('set.pkg.brokenWhy')}</p>
              {/if}
              {#if !e.source}
                <p class="note warn"><Icon name="alert" size={14} />{t('set.pkg.manualTitle')}</p>
              {/if}
              <dl class="facts">
                <dt>ID</dt>
                <dd><code class="mono">{e.id}</code></dd>
                {#if e.manifest}
                  <dt>{t('set.pkg.f.author')}</dt>
                  <dd>{e.manifest.author}</dd>
                  <dt>{t('set.pkg.f.repo')}</dt>
                  <dd>{#if e.manifest.repository}<code class="mono">{e.manifest.repository}</code>{:else}{t('set.pkg.closed')}{/if}</dd>
                  <dt>{t('set.pkg.f.license')}</dt>
                  <dd>{e.manifest.license ?? t('set.pkg.f.none')}</dd>
                {/if}
                <dt>{t('set.pkg.f.check')}</dt>
                <dd>{checkText(e)}</dd>
                {#if e.installedAt}
                  <dt>{t('set.pkg.f.installed')}</dt>
                  <dd>{day.format(e.installedAt)}</dd>
                {/if}
                {#if e.source?.kind === 'file'}
                  <dt>{t('set.pkg.f.file')}</dt>
                  <dd><span class="file-name">{e.source.name}</span> <code class="mono sha" title={e.source.sha256}>sha256 {e.source.sha256.slice(0, 12)}…</code></dd>
                {/if}
                {#if e.manifest?.network.length}
                  <dt>{t('set.pkg.f.network')}</dt>
                  <dd>{e.manifest.network.join(', ')}</dd>
                {/if}
                {#if e.manifest?.writes.length}
                  <dt>{t('set.pkg.f.writes')}</dt>
                  <dd>{e.manifest.writes.map(writeText).join(ui.lang === 'en' ? ', ' : '、')}</dd>
                {/if}
                {#if e.manifest?.formats.length}
                  <dt>{t('set.pkg.f.formats')}</dt>
                  <dd>{e.manifest.formats.join(', ')}</dd>
                {/if}
                <dt>{t('set.pkg.f.data')}</dt>
                <dd>{bytes(e.dataBytes)}</dd>
              </dl>
              <div class="detail-actions">
                <button class="btn sm" onclick={() => (removing = e)}><Icon name="x" size={14} />{t('set.pkg.uninstall')}</button>
              </div>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

{#snippet themeCard(id: string, label: string, author: string, spec: ThemeSpec | null, e: PackageEntry | null)}
  {@const on = ui.themeId === id || (id === 'builtin' && !ui.themeSpec && !packages.themes.some((x) => x.id === ui.themeId))}
  {@const c = colorsOf(spec)}
  {@const pv = e ? provenance(e) : null}
  <div class="theme" class:on class:broken={!!e && !spec}>
    <button
      class="theme-pick"
      role="radio"
      aria-checked={on}
      disabled={!!e && !spec}
      onclick={() => useTheme(id, label)}
    >
      <!-- 一扇小窗：表面色的底、主色的按钮、成功/重试/失败三个点 -->
      <span class="mini" aria-hidden="true" style:--s={c.surface} style:--p={c.primary} style:--tx={c.text} style:--ok={c.ok} style:--rt={c.retry} style:--er={c.error}>
        <span class="mini-bar"></span>
        <span class="mini-sheet">
          <span class="mini-line"></span>
          <span class="mini-line short"></span>
          <span class="mini-dots"><i class="ok"></i><i class="rt"></i><i class="er"></i></span>
          <span class="mini-btn"></span>
        </span>
      </span>
      <span class="theme-name">{label}{#if e?.manifest}<span class="ver">v{e.manifest.version}</span>{/if}</span>
      <span class="theme-sub">{author}</span>
      {#if on}<span class="tick" in:pop={{ y: 0, scale: 0.4, duration: 380 }}><Icon name="check" size={12} stroke={3} /></span>{/if}
    </button>
    {#if e}
      <div class="theme-foot">
        {#if pv}<span class="chip tiny {pv.tone}" title={pv.title}><Icon name={pv.icon} size={11} stroke={2.4} />{pv.text}</span>{/if}
        <button class="icon-btn" onclick={() => (removing = e)} aria-label={t('set.pkg.uninstallNamed', { name: label })} title={t('set.pkg.uninstall')}>
          <Icon name="x" size={14} />
        </button>
      </div>
    {/if}
  </div>
{/snippet}

{#if removing}
  <UninstallDialog entry={removing} onclose={() => (removing = null)} />
{/if}

<style>
  .group {
    max-width: 820px;
    margin-bottom: 18px;
    padding: 4px 20px 16px;
  }
  .group h2 {
    display: flex;
    align-items: center;
    gap: 9px;
    margin: 14px 0 6px;
    font-size: var(--fs-lg);
    font-weight: 620;
  }
  .group-desc {
    margin: 0 0 12px;
    max-width: 68ch;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  h3 {
    margin: 18px 0 10px;
    font-size: var(--fs-md, 1rem);
    font-weight: 620;
  }
  .install-bar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px 14px;
  }
  .hint {
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  /* 文件选择框只是个入口：藏起来，由「安装…」按钮打开 */
  .file {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }
  .note {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    margin: 8px 0;
    padding: 8px 12px;
    border-radius: var(--r-md);
    font-size: var(--fs-sm);
    line-height: 1.5;
  }
  .note :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .note.warn {
    background: var(--butter-soft);
  }
  .note.coral {
    background: var(--coral-soft);
  }
  .quiet {
    margin: 0;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .ver {
    margin-left: 6px;
    color: var(--text-2);
    font-weight: 500;
    font-size: var(--fs-sm);
  }

  /* ---------- 主题卡片 ---------- */
  .themes {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(172px, 1fr));
    gap: 12px;
  }
  .theme {
    position: relative;
    display: grid;
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    transition:
      border-color calc(var(--motion) * 0.2s),
      box-shadow calc(var(--motion) * 0.2s);
  }
  .theme:hover {
    border-color: var(--lilac);
  }
  .theme.on {
    border-color: var(--lilac-deep);
    box-shadow: 0 0 0 3px var(--lilac-soft);
  }
  .theme.broken {
    opacity: 0.6;
  }
  .theme-pick {
    position: relative;
    display: grid;
    align-content: start;
    gap: 3px;
    padding: 10px 10px 8px;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
    border-radius: var(--r-lg);
  }
  .theme-pick:disabled {
    cursor: not-allowed;
  }
  .mini {
    display: grid;
    grid-template-rows: 9px 1fr;
    height: 74px;
    margin-bottom: 6px;
    overflow: hidden;
    border-radius: var(--r-md);
    background: var(--s);
    border: var(--line-w-thin) solid color-mix(in oklch, var(--tx) 14%, transparent);
  }
  .mini-bar {
    background: color-mix(in oklch, var(--tx) 6%, var(--s));
  }
  .mini-sheet {
    position: relative;
    margin: 6px 0 0 14px;
    padding: 8px 9px;
    border-top-left-radius: 8px;
    background: color-mix(in oklch, var(--s) 86%, white);
    display: grid;
    align-content: start;
    gap: 4px;
  }
  .mini-line {
    height: 4px;
    width: 70%;
    border-radius: 2px;
    background: color-mix(in oklch, var(--tx) 70%, transparent);
  }
  .mini-line.short {
    width: 45%;
    background: color-mix(in oklch, var(--tx) 35%, transparent);
  }
  .mini-dots {
    display: flex;
    gap: 3px;
    margin-top: 2px;
  }
  .mini-dots i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }
  .mini-dots .ok {
    background: var(--ok);
  }
  .mini-dots .rt {
    background: var(--rt);
  }
  .mini-dots .er {
    background: var(--er);
  }
  .mini-btn {
    position: absolute;
    right: 9px;
    bottom: 8px;
    width: 26px;
    height: 10px;
    border-radius: 5px;
    background: var(--p);
  }
  .theme-name {
    display: flex;
    align-items: baseline;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .theme-sub {
    color: var(--text-2);
    font-size: var(--fs-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tick {
    position: absolute;
    top: 14px;
    right: 14px;
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--lilac-deep);
    color: var(--on-accent, white);
  }
  .theme-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 0 8px 8px 10px;
  }
  .icon-btn {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: 0;
    border-radius: var(--r-sm, 6px);
    background: none;
    color: var(--text-2);
    cursor: pointer;
  }
  .icon-btn:hover {
    background: var(--card-2-bg);
    color: var(--text);
  }
  .chip.tiny {
    font-size: var(--fs-xs);
  }

  /* ---------- 扩展列表 ---------- */
  .exts {
    display: grid;
    gap: 8px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .ext {
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
  }
  .ext.open {
    border-color: var(--lilac);
  }
  .ext-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding-right: 14px;
  }
  .ext-main {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 0 12px 14px;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .ext-icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: var(--r-md);
    background: var(--butter-soft);
    color: var(--text);
    font-weight: 700;
  }
  .ext.off .ext-icon {
    background: var(--card-2-bg);
    color: var(--text-2);
  }
  .ext-text {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 2px;
  }
  .ext-name {
    display: flex;
    align-items: baseline;
    font-weight: 600;
  }
  .ext-desc {
    color: var(--text-2);
    font-size: var(--fs-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ext.open .ext-desc {
    white-space: normal;
  }
  .tags {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 6px;
  }
  .chev {
    flex: none;
    display: grid;
    color: var(--text-2);
    transition: transform calc(var(--motion) * 0.25s);
  }
  .ext.open .chev {
    transform: rotate(180deg);
  }
  .ext-detail {
    padding: 4px 16px 14px 62px;
  }
  .facts {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 6px 16px;
    margin: 6px 0 12px;
    font-size: var(--fs-sm);
  }
  .facts dt {
    color: var(--text-2);
  }
  .facts dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .sha {
    color: var(--text-2);
    font-size: var(--fs-xs);
  }
  .detail-actions {
    display: flex;
    justify-content: flex-end;
  }
  @media (max-width: 640px) {
    .tags {
      display: none;
    }
    .ext-detail {
      padding-left: 16px;
    }
  }
</style>

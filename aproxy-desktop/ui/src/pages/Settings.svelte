<!--
  设置：外观（语言、模式、配色强度、吉祥物、Mica）、后台（看护者、开机自启）、扩展（主题 / 吉祥物 / 扩展包三层）、关于。

  Mica 的开关有三种样子：可用（开或关）；系统不支持（禁用并写明原因）；
  系统关了透明效果（可以开，但提示会显示为纯色）。能力由系统给出，原型里从审核工具条切换。

  扩展按能做的事分三层，由少到多：
    主题：一段可以复制分享的字符串（lib/theme.ts），只改 CSS 变量。粘贴后当场校验并说明它改了什么，
          可以先整窗预览（预览条压在内容区顶上，切到别的页面也在），满意再应用；导出就是复制当前主题的字符串。
    吉祥物：别人写的代码，但关在沙箱 iframe 里（MascotFrame），只拿到状态数据，没有写操作；在「外观」里选中就是在用。
    扩展包：在界面里运行的代码。读不分级（看到的就是界面看到的），只有写操作要用户同意：列表上的标签说明
          每个扩展会做哪类写操作，展开详情能看到哪些已经允许、最近做过什么，并撤销已允许的。
-->
<script lang="ts">
  import { t } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { overlays, copyText } from '../lib/overlays.svelte';
  import { pop } from '../lib/motion';
  import { BUILTIN_THEME, SAMPLE_THEME, parseTheme, serializeTheme, type ThemeError } from '../lib/theme';
  import { mascots, mascotName, NO_MASCOT } from '../lib/mascot/registry.svelte';
  import type { ExtensionAction, ExtensionInfo, LocalizedText, ThemeColors, ThemeSpec, WriteCapability } from '../lib/types';
  import Icon, { type IconName } from '../components/Icon.svelte';
  import Segmented from '../components/Segmented.svelte';
  import Switch from '../components/Switch.svelte';
  import MascotView from '../components/MascotView.svelte';

  const micaUnsupported = $derived(ui.micaSupport === 'unsupported');

  async function copyDiag() {
    await copyText(
      ['aProxy 0.2.0 (desktop)', 'Windows 11 build 26200', `mica: ${ui.micaSupport}`, `mode: ${ui.variant}`, `theme: ${ui.themeSpec?.name ?? 'aProxy'}`].join(
        '\n',
      ),
    );
    overlays.toast(t('toast.copied'));
  }

  /** 扩展、发布说明这类后端给的文字：按界面语言取，缺了退回英文 */
  const loc = (x: LocalizedText) => x[ui.lang] ?? x.en;

  // ---------------------------------------------------------------------------
  // 主题
  // ---------------------------------------------------------------------------

  let importing = $state(false);
  let draft = $state('');
  const parsed = $derived(parseTheme(draft));
  // 这一栏说的是「窗口上现在穿的」：预览中就是预览的那个，复制出去的也是它——看到什么就分享什么
  const current = $derived(ui.activeTheme ?? BUILTIN_THEME);
  const currentKind = $derived(ui.themePreview ? 'previewing' : ui.themeSpec ? 'imported' : 'builtin');

  function errorText(e: ThemeError): string {
    return t(`theme.err.${e.key}`, { field: e.field ?? '', detail: e.detail ?? '' });
  }

  /** 解析结果里值得告诉用户的几件事：变体、圆角、字体、动效、对比度 */
  function facts(spec: ThemeSpec): string[] {
    const out: string[] = [];
    if (spec.light && spec.dark) out.push(t('set.theme.both'));
    else if (spec.light) out.push(t('set.theme.lightOnly'));
    else if (spec.dark) out.push(t('set.theme.darkOnly'));
    else out.push(t('set.theme.noColors'));
    if (spec.radius !== undefined && spec.radius !== 1) out.push(t('set.theme.radius', { n: spec.radius }));
    if (spec.font && spec.font !== 'rounded') out.push(t(`set.theme.font.${spec.font}`));
    if (spec.motion === 0) out.push(t('set.theme.noMotion'));
    else if (spec.motion !== undefined && spec.motion !== 1) out.push(t('set.theme.motion', { n: spec.motion }));
    if (spec.contrast === 'more') out.push(t('set.theme.contrast'));
    return out;
  }

  const ROLES: (keyof ThemeColors)[] = ['primary', 'text', 'surface', 'ok', 'retry', 'error', 'warn'];

  function openImport() {
    importing = !importing;
    if (!importing) cancelPreview();
  }

  function fillSample() {
    draft = serializeTheme(SAMPLE_THEME);
  }

  function preview() {
    if (parsed.ok) ui.themePreview = parsed.spec;
  }

  function cancelPreview() {
    ui.themePreview = null;
  }

  function applyDraft() {
    if (!parsed.ok) return;
    ui.themeSpec = parsed.spec;
    ui.themePreview = null;
    importing = false;
    draft = '';
    overlays.toast(t('toast.themeApplied', { name: parsed.spec.name }));
  }

  async function exportTheme() {
    await copyText(serializeTheme(current));
    overlays.toast(t('toast.themeCopied'));
  }

  function resetTheme() {
    ui.themeSpec = null;
    ui.themePreview = null;
    overlays.toast(t('toast.themeReset'));
  }

  // ---------------------------------------------------------------------------
  // 吉祥物
  // ---------------------------------------------------------------------------

  const choices = $derived([
    ...mascots.entries.map((e) => ({ id: e.id, name: mascotName(e), custom: e.source === 'custom' })),
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

  // ---------------------------------------------------------------------------
  // 扩展
  // ---------------------------------------------------------------------------

  let openExt = $state<string | null>(null);
  const customMascots = $derived(store.extensions.filter((e) => e.kind === 'mascot'));
  const packages = $derived(store.extensions.filter((e) => e.kind === 'extension'));

  const CAP_ICON: Record<WriteCapability, IconName> = { control: 'power', config: 'sliders', install: 'download' };
  const CAP_TONE: Record<WriteCapability, string> = { control: 'sky', config: 'butter', install: 'lilac' };

  function grantOf(ext: ExtensionInfo, cap: WriteCapability) {
    return ext.granted.find((g) => g.capability === cap);
  }

  function target(port: number): string {
    const alias = store.instance(port)?.alias;
    if (!alias) return String(port);
    return ui.lang === 'en' ? `${port} (${alias})` : `${port}（${alias}）`;
  }

  function actionText(a: ExtensionAction): string {
    if (a.kind === 'install') return t('ext.act.install', { version: a.version });
    if (a.kind === 'write-config') return t('ext.act.write-config', { target: target(a.port), keys: a.keys.join(', ') });
    return t(`ext.act.${a.kind}`, { target: target(a.port) });
  }

  const day = $derived(new Intl.DateTimeFormat(ui.lang, { month: 'long', day: 'numeric' }));
  const dayTime = $derived(new Intl.DateTimeFormat(ui.lang, { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }));

  async function toggleExt(ext: ExtensionInfo, on: boolean) {
    await store.setExtensionEnabled(ext.id, on);
    overlays.toast(t(on ? 'toast.extOn' : 'toast.extOff', { name: loc(ext.name) }), on ? 'mint' : 'lilac');
  }

  async function revoke(ext: ExtensionInfo, cap: WriteCapability) {
    await store.revokeGrant(ext.id, cap);
    overlays.toast(t('toast.revoked', { name: loc(ext.name), cap: t(`ext.capName.${cap}`) }), 'lilac');
  }
</script>

<h1>{t('nav.settings')}</h1>

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
        <span class="note muted" in:pop={{ y: -4, scale: 1, duration: 320 }}><Icon name="info" size={14} />{t('set.mica.unsupported')}</span>
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
    </div>
    <!-- 看护者与开机自启是守护进程那边的设置（settings.json），经 Backend 读写 -->
    <Switch checked={store.watchdogOn} onchange={(v) => store.updateSettings({ watchdog: v })} label={t('set.watchdog')} />
  </div>
  <div class="row">
    <div class="txt">
      <span class="name">{t('set.autostart')}</span>
      <span class="desc">{t('set.autostart.desc')}</span>
    </div>
    <Switch checked={store.settings?.autostart ?? false} onchange={(v) => store.updateSettings({ autostart: v })} label={t('set.autostart')} />
  </div>
</section>

<section class="panel group">
  <h2><Icon name="puzzle" size={17} />{t('set.ext')}</h2>
  <p class="group-desc">{t('set.ext.desc')}</p>

  <!-- 三层按能做的事由少到多排：主题只改变量；吉祥物是代码但关在沙箱里；扩展包在界面里运行，写操作要同意 -->
  <div class="tier">
    <div class="tier-head">
      <span class="tier-icon lilac"><Icon name="palette" size={16} /></span>
      <h3>{t('ext.tier.theme')}</h3>
      <span class="chip tiny">{t('ext.tier.theme.tag')}</span>
    </div>
    <p class="tier-desc">{t('ext.tier.theme.desc')}</p>
    <div class="theme-bar">
      <!-- 色板读的是活的令牌：预览或换主题时它跟着变，不需要自己算颜色 -->
      <span class="swatches" aria-hidden="true">
        <i class="sw lilac"></i><i class="sw mint"></i><i class="sw sky"></i><i class="sw coral"></i><i class="sw butter"></i>
      </span>
      <span class="theme-name">{current.name}</span>
      <span class="chip" class:lilac={currentKind !== 'builtin'}>{t(`set.theme.${currentKind}`)}</span>
      <span class="grow"></span>
      <button class="btn sm" class:on={importing} onclick={openImport} aria-expanded={importing} aria-controls="theme-import">
        <Icon name="download" size={14} />{t('set.theme.import')}
      </button>
      <button class="btn sm" onclick={exportTheme}><Icon name="copy" size={14} />{t('set.theme.export')}</button>
      {#if ui.themeSpec}
        <button class="btn sm ghost" onclick={resetTheme}><Icon name="undo" size={14} />{t('set.theme.reset')}</button>
      {/if}
    </div>

    {#if importing}
      <div class="import" id="theme-import" in:pop={{ y: -6, scale: 1, duration: 380 }}>
        <div class="import-head">
          <label for="theme-input">{t('set.theme.paste')}</label>
          <button class="link" onclick={fillSample}>{t('set.theme.sample')}</button>
        </div>
        <textarea
          id="theme-input"
          class="mono"
          rows="3"
          spellcheck="false"
          autocomplete="off"
          placeholder={'aproxy-theme-v1:{"name":"…"}'}
          bind:value={draft}
          aria-invalid={!!draft.trim() && !parsed.ok}
          aria-describedby="theme-status"
        ></textarea>

        <div id="theme-status" aria-live="polite">
          {#if draft.trim()}
            {#if parsed.ok}
              {@const spec = parsed.spec}
              {@const colors = spec[ui.variant]}
              <div class="verdict ok">
                <Icon name="check" size={15} stroke={2.6} />
                <strong>{spec.name}</strong>
                {#if spec.author}<span class="muted">{t('set.theme.by', { name: spec.author })}</span>{/if}
                {#each facts(spec) as f (f)}<span class="chip">{f}</span>{/each}
              </div>
              {#if colors}
                <div class="seeds">
                  <span class="seeds-title">{t(ui.variant === 'dark' ? 'set.theme.seeds.dark' : 'set.theme.seeds.light')}</span>
                  {#each ROLES as role (role)}
                    {#if colors[role]}
                      <span class="seed"><i class="sw" style:background={colors[role]}></i>{t(`theme.role.${role}`)}</span>
                    {/if}
                  {/each}
                </div>
                <p class="hint">{t('set.theme.lightness')}</p>
              {/if}
            {:else}
              <div class="verdict bad" role="alert"><Icon name="alert" size={15} stroke={2.4} />{errorText(parsed.error)}</div>
            {/if}
          {/if}
        </div>

        <div class="import-actions">
          <button class="btn sm ghost" onclick={openImport}>{t('confirm.cancel')}</button>
          <button class="btn sm" disabled={!parsed.ok} onclick={preview}><Icon name="eye" size={14} />{t('set.theme.preview')}</button>
          <button class="btn sm primary" disabled={!parsed.ok} onclick={applyDraft}>{t('set.theme.apply')}</button>
        </div>
      </div>
    {/if}
  </div>

  <div class="tier">
    <div class="tier-head">
      <span class="tier-icon pink"><Icon name="shield" size={16} /></span>
      <h3>{t('ext.tier.mascot')}</h3>
      <span class="chip tiny">{t('ext.tier.mascot.tag')}</span>
    </div>
    <p class="tier-desc">{t('ext.tier.mascot.desc')}</p>
    <ul class="exts">
      {#each customMascots as m (m.id)}
        {@const name = loc(m.name)}
        <li class="ext">
          <div class="ext-row mascot-row">
            <span class="ext-icon mascot"><MascotView id={m.id} mood="idle" icon={30} /></span>
            <span class="ext-text">
              <span class="ext-name">{name}<span class="ver">v{m.version}</span></span>
              <span class="ext-desc">{loc(m.description)}</span>
              <code class="mono origin">{m.origin}</code>
            </span>
            {#if ui.mascotId === m.id}
              <span class="chip lilac in-use"><Icon name="check" size={12} stroke={2.6} />{t('ext.inUse')}</span>
            {:else}
              <button class="btn sm" onclick={() => (ui.mascotId = m.id)}>{t('ext.useMascot')}</button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  </div>

  <div class="tier">
    <div class="tier-head">
      <span class="tier-icon butter"><Icon name="puzzle" size={16} /></span>
      <h3>{t('ext.tier.ext')}</h3>
      <span class="chip tiny">{t('ext.tier.ext.tag')}</span>
    </div>
    <p class="tier-desc">{t('ext.tier.ext.desc')}</p>
    <ul class="exts">
      {#each packages as ext (ext.id)}
        {@const open = openExt === ext.id}
        {@const name = loc(ext.name)}
        <li class="ext" class:open class:off={!ext.enabled}>
          <div class="ext-row">
            <button
              class="ext-main"
              onclick={() => (openExt = open ? null : ext.id)}
              aria-expanded={open}
              aria-controls="ext-{ext.id}"
              aria-label={t('ext.details', { name })}
            >
              <span class="ext-icon">{name.slice(0, 1)}</span>
              <span class="ext-text">
                <span class="ext-name">{name}<span class="ver">v{ext.version}</span></span>
                <span class="ext-desc">{loc(ext.description)}</span>
              </span>
              <!-- 标签只说「它会做哪类写操作」；已经允许过的带一个勾 -->
              <span class="caps">
                {#if ext.declares.length === 0}
                  <span class="chip mint"><Icon name="eye" size={12} stroke={2.4} />{t('ext.cap.read')}</span>
                {:else}
                  {#each ext.declares as cap (cap)}
                    <span class="chip {CAP_TONE[cap]}">
                      <Icon name={grantOf(ext, cap) ? 'check' : CAP_ICON[cap]} size={12} stroke={2.4} />{t(`ext.cap.${cap}`)}
                    </span>
                  {/each}
                {/if}
              </span>
              <span class="chev"><Icon name="chevronDown" size={16} /></span>
            </button>
            <Switch checked={ext.enabled} onchange={(v) => toggleExt(ext, v)} label={t('ext.toggle', { name })} />
          </div>

          {#if open}
            <div class="ext-detail" id="ext-{ext.id}" in:pop={{ y: -6, scale: 1, duration: 360 }}>
              <dl class="facts">
                <dt>{t('ext.origin')}</dt>
                <dd><code class="mono">{ext.origin}</code></dd>
                <dt>{t('ext.author')}</dt>
                <dd>{ext.author}</dd>
                <dt>{t('ext.reads')}</dt>
                <dd>{t('ext.readsWhat')}</dd>
              </dl>

              <h3>{t('ext.writes')}</h3>
              {#if ext.declares.length === 0}
                <p class="quiet">{t('ext.noWrites')}</p>
              {:else}
                <ul class="grants">
                  {#each ext.declares as cap (cap)}
                    {@const g = grantOf(ext, cap)}
                    <li>
                      <span class="g-icon {CAP_TONE[cap]}"><Icon name={CAP_ICON[cap]} size={15} /></span>
                      <span class="g-text">
                        <strong>{t(`ext.capName.${cap}`)}</strong>
                        <span>{t(`ext.capDesc.${cap}`)}</span>
                      </span>
                      {#if g}
                        <span class="g-state ok"><Icon name="check" size={13} stroke={2.6} />{t('ext.granted', { date: day.format(g.at) })}</span>
                        <button class="btn sm" onclick={() => revoke(ext, cap)}>{t('ext.revoke')}</button>
                      {:else}
                        <span class="g-state">{t('ext.askFirst')}</span>
                      {/if}
                    </li>
                  {/each}
                </ul>
              {/if}
              {#if ext.recent.length}
                <h3>{t('ext.recent')}</h3>
                <ul class="recent">
                  {#each ext.recent.slice(0, 4) as r (r.at)}
                    <li><span class="when num">{dayTime.format(r.at)}</span>{actionText(r.action)}</li>
                  {/each}
                </ul>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  </div>
</section>

<section class="panel group about">
  <h2><Icon name="info" size={17} />{t('set.about')}</h2>
  <div class="about-body">
    <!-- 「关于」里展示的是软件图标（默认吉祥物的静态大图标），与界面里选了哪个吉祥物、是否显示无关 -->
    <MascotView id="tumbler" mood="idle" icon={84} />
    <div class="about-text">
      <div class="about-name">
        <strong>aProxy</strong>
        <span class="chip">v0.2.0</span>
        <span class="chip lilac"><Icon name="monitor" size={12} stroke={2.4} />{t('up.variant.desktop')}</span>
      </div>
      <p>{t('set.about.desc')}</p>
      <div class="links">
        <button class="btn sm" onclick={() => overlays.toast(t('toast.protoOnly'), 'lilac')}><Icon name="external" size={14} />{t('set.homepage')}</button>
        <button class="btn sm" onclick={() => ui.go('update')}><Icon name="file" size={14} />{t('set.changelog')}</button>
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
  .group-desc {
    margin: 0 0 12px;
    max-width: 68ch;
    color: var(--text-2);
    font-size: var(--fs-sm);
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

  /* ---------- 主题 ---------- */
  .theme-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 10px;
    padding: 10px 12px 10px 14px;
    border-radius: var(--r-md);
    background: var(--card-2-bg);
  }
  .swatches {
    display: inline-flex;
  }
  .sw {
    display: inline-block;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    box-shadow: 0 0 0 1.5px var(--card-2-bg);
  }
  .swatches .sw + .sw {
    margin-left: -4px;
  }
  .sw.lilac { background: var(--lilac); }
  .sw.mint { background: var(--mint); }
  .sw.sky { background: var(--sky); }
  .sw.coral { background: var(--coral); }
  .sw.butter { background: var(--butter); }
  .theme-name {
    font-weight: 600;
  }
  .grow {
    flex: 1;
  }
  .btn.on {
    background: var(--lilac-soft);
    color: var(--lilac-deep);
  }
  .import {
    display: grid;
    gap: 10px;
    padding: 14px;
    border-radius: var(--r-md);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    box-shadow: var(--shadow-1);
  }
  .import-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--lilac-deep);
    font-size: var(--fs-sm);
    font-weight: 600;
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  textarea {
    width: 100%;
    min-height: 76px;
    resize: vertical;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    border: var(--line-w) solid var(--line-strong);
    background: var(--card-bg);
    color: var(--text);
    font-size: var(--fs-sm);
    line-height: 1.5;
    outline: none;
    word-break: break-all;
    transition: box-shadow var(--dur-mid) var(--ease-spring), border-color var(--dur-fast) linear;
  }
  textarea:focus {
    border-color: var(--lilac-deep);
    box-shadow: 0 0 0 3.5px color-mix(in oklab, var(--lilac) 55%, transparent);
  }
  textarea[aria-invalid='true'] {
    border-color: var(--coral-deep);
  }
  .verdict {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 8px;
    font-size: var(--fs-sm);
  }
  .verdict.ok :global(svg) {
    color: var(--mint-deep);
  }
  .verdict.bad {
    padding: 8px 12px;
    border-radius: var(--r-sm);
    background: var(--coral-soft);
    color: var(--coral-deep);
    font-weight: 550;
  }
  .seeds {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 14px;
    margin-top: 10px;
    font-size: var(--fs-sm);
    color: var(--text-2);
  }
  .seeds-title {
    color: var(--text-3);
  }
  .seed {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .seed .sw {
    box-shadow: 0 0 0 1px color-mix(in oklab, var(--text) 18%, transparent);
  }
  .hint {
    margin: 6px 0 0;
    color: var(--text-3);
    font-size: var(--fs-xs);
  }
  .import-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  /* ---------- 扩展的三层 ---------- */
  .tier {
    padding: 14px 0 6px;
    border-top: var(--line-w-thin) dashed var(--line-soft);
  }
  .group-desc + .tier {
    border-top: 0;
    padding-top: 2px;
  }
  .tier-head {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  .tier-head h3 {
    margin: 0;
    font-size: var(--fs-md);
    font-weight: 650;
  }
  .tier-icon {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 50%;
  }
  .tier-icon.lilac { background: var(--lilac-soft); color: var(--lilac-deep); }
  .tier-icon.pink { background: var(--pink-soft); color: var(--pink-deep); }
  .tier-icon.butter { background: var(--butter-soft); color: var(--butter-deep); }
  .tier-desc {
    margin: 6px 0 12px 37px;
    max-width: 66ch;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .mascot-row {
    padding: 10px 14px 10px 12px;
  }
  .origin {
    color: var(--text-3);
    font-size: var(--fs-xs);
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

  /* ---------- 扩展 ---------- */
  .exts {
    margin: 0 0 12px;
    padding: 0;
    list-style: none;
    display: grid;
    gap: 8px;
  }
  .ext {
    border-radius: var(--r-md);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    transition: box-shadow var(--dur-mid) var(--ease-soft);
  }
  .ext.open {
    box-shadow: var(--shadow-2);
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
    padding: 12px 6px 12px 12px;
    border: 0;
    border-radius: var(--r-md);
    background: none;
    text-align: left;
  }
  .ext-icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    border-radius: var(--r-sm);
    background: var(--lilac-soft);
    color: var(--lilac-deep);
    font-size: var(--fs-lg);
    font-weight: 650;
  }
  .ext-icon.mascot {
    background: var(--pink-soft);
  }
  .ext.off .ext-icon {
    background: var(--card-2-bg);
    color: var(--text-3);
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
    gap: 8px;
    font-weight: 600;
  }
  .ver {
    color: var(--text-3);
    font-size: var(--fs-xs);
    font-weight: 500;
  }
  .ext-desc {
    overflow: hidden;
    color: var(--text-2);
    font-size: var(--fs-sm);
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .ext.open .ext-desc {
    white-space: normal;
  }
  .ext.off .ext-name,
  .ext.off .ext-desc {
    color: var(--text-3);
  }
  .caps {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 5px;
    max-width: 260px;
  }
  .chev {
    flex: none;
    color: var(--text-3);
    transition: transform var(--dur-mid) var(--ease-spring);
  }
  .ext.open .chev {
    transform: rotate(180deg);
  }
  .in-use {
    flex: none;
  }
  .ext-detail {
    display: grid;
    gap: 10px;
    margin: 0 14px 14px 62px;
    padding-top: 12px;
    border-top: var(--line-w-thin) dashed var(--line-soft);
    font-size: var(--fs-sm);
  }
  .ext-detail h3 {
    margin: 4px 0 0;
    font-size: var(--fs-sm);
    font-weight: 650;
  }
  .facts {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 6px 16px;
    margin: 0;
  }
  .facts dt {
    color: var(--text-3);
  }
  .facts dd {
    margin: 0;
    color: var(--text-2);
    overflow-wrap: anywhere;
  }
  .quiet {
    margin: 0;
    color: var(--text-3);
  }
  .grants {
    margin: 0;
    padding: 0;
    list-style: none;
    display: grid;
    gap: 6px;
  }
  .grants li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: var(--r-sm);
    background: var(--card-2-bg);
  }
  .g-icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 50%;
  }
  .g-icon.sky { background: var(--sky-soft); color: var(--sky-deep); }
  .g-icon.butter { background: var(--butter-soft); color: var(--butter-deep); }
  .g-icon.lilac { background: var(--lilac-soft); color: var(--lilac-deep); }
  .g-text {
    flex: 1;
    display: grid;
    gap: 1px;
  }
  .g-text span {
    color: var(--text-2);
  }
  .g-state {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .g-state.ok {
    color: var(--mint-deep);
    font-weight: 600;
  }
  .recent {
    margin: 0;
    padding: 0;
    list-style: none;
    display: grid;
    gap: 4px;
    color: var(--text-2);
  }
  .when {
    display: inline-block;
    min-width: 92px;
    color: var(--text-3);
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

<!--
  安装主题包或扩展包时的逐条提醒（流程与顺序在 lib/packages.svelte.ts）。

  这是整套扩展设计里唯一的保护，所以每一条都要让人停下来看：
    - 头部始终是「装的是什么」：名字、版本、种类、作者、id，升级或降级也写在这里；
    - 第一条讲清它装上之后能做什么，写具体的后果（「把上游地址指到别处，API key 跟着发过去」），
      不写空泛的「能做一切」；同时显示来源，GitHub 的核对结果回来就地更新；
    - 之后每一条只讲一件事：闭源、核对不了、文件对不上、关注的人少……颜色按轻重分：对不上、闭源、仓库不存在是珊瑚色，
      其余核对不了的是黄色；
    - 焦点默认落在「取消安装」上，Esc 与点遮罩都是取消：没看清就按回车，结果应该是不装。
  包作者写的文字（名字、描述、作者、主机名）只当文本显示。
-->
<script lang="ts">
  import { t, bytes, fmtNum } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { packages, textOf, type Warning } from '../lib/packages.svelte';
  import { faultText } from '../lib/faults';
  import { parseTheme } from '../lib/theme';
  import { pop, fadeOut } from '../lib/motion';
  import type { GithubFacts, PackageCheck, ReleaseFacts, Unreachable, WriteKind } from '../lib/backend/contract';
  import type { ThemeColors } from '../lib/types';
  import Icon, { type IconName } from './Icon.svelte';

  const flow = $derived(packages.flow);
  const review = $derived(flow?.step === 'review' ? flow : null);
  const check = $derived(flow?.step === 'review' || flow?.step === 'installing' ? flow.check : null);
  const warning = $derived<Warning | null>(review && review.index < review.warnings.length ? review.warnings[review.index] : null);
  /** 前面的都确认过了、在等 GitHub 的结果 */
  const waiting = $derived(!!review && review.index >= review.warnings.length);
  /** GitHub 的结果回来了，提醒一共几条才定下来 */
  const total = $derived(review && (review.facts || review.verifyFailed) ? review.warnings.length : null);
  const last = $derived(!!review && total !== null && review.index === total - 1);

  const name = $derived(check ? textOf(check.manifest.name, check.manifest.id) : '');
  const kind = $derived(check?.manifest.kind ?? 'extension');
  const theme = $derived(check?.theme ? parseTheme(check.theme) : null);
  /** 主题包带不带 theme.css：带了，提醒里要讲样式表能做什么 */
  const hasCss = $derived(!!check?.files.some((f) => f.path === 'theme.css'));
  const repoName = $derived(check?.manifest.repository?.replace(/^https:\/\/github\.com\//, '') ?? '');

  let showFiles = $state(false);
  let cancelBtn: HTMLButtonElement | undefined = $state();
  let dialog: HTMLDivElement | undefined = $state();

  // 每换一条提醒，焦点回到「取消安装」，文件列表收起
  $effect(() => {
    void flow?.step;
    void review?.index;
    showFiles = false;
    queueMicrotask(() => cancelBtn?.focus());
  });

  const day = $derived(new Intl.DateTimeFormat(ui.lang, { year: 'numeric', month: 'long', day: 'numeric' }));

  /** 这一条提醒的图标与轻重 */
  function tone(w: Warning): { icon: IconName; tone: 'lilac' | 'butter' | 'coral' } {
    switch (w.kind) {
      case 'trust':
        return { icon: kind === 'theme' ? 'palette' : 'puzzle', tone: 'lilac' };
      case 'closed':
        return { icon: 'lock', tone: 'coral' };
      case 'repo-missing':
        return { icon: 'alert', tone: 'coral' };
      case 'release':
        return w.release.state === 'mismatch' ? { icon: 'shieldOff', tone: 'coral' } : { icon: 'shield', tone: 'butter' };
      case 'reputation':
        return { icon: 'star', tone: 'butter' };
      case 'not-github':
        return { icon: 'globe', tone: 'butter' };
      default:
        return { icon: 'cloudRain', tone: 'butter' };
    }
  }

  function reasonText(reason: Unreachable, status: number | null): string {
    return t(`install.unreachable.${reason}`, { status: status ?? '?' });
  }

  function releaseTitle(r: Exclude<ReleaseFacts, { state: 'match' }>): string {
    return t(`install.release.${r.state}.title`);
  }

  function releaseBody(r: Exclude<ReleaseFacts, { state: 'match' }>, repo: string, version: string): string {
    switch (r.state) {
      case 'mismatch':
        return t('install.release.mismatch.body', { repo, tag: r.tag, asset: r.asset });
      case 'no-asset':
      case 'no-digest':
        return t(`install.release.${r.state}.body`, { repo, tag: r.tag });
      case 'no-release':
        return t('install.release.no-release.body', { repo, version });
      case 'not-a-file':
        return t('install.release.not-a-file.body');
      case 'unreachable':
        return t('install.release.unreachable.body', { repo, reason: reasonText(r.reason, r.status) });
    }
  }

  /** 来源一栏的一句话：GitHub 的结果回来前后不同 */
  function sourceLine(c: PackageCheck, facts: GithubFacts | null, failed: boolean): { icon: IconName; text: string; tone: string } {
    if (!c.manifest.repository) return { icon: 'lock', text: t('install.source.closed'), tone: 'coral' };
    if (failed) return { icon: 'cloudRain', text: t('install.source.unverified'), tone: 'butter' };
    if (!facts) return { icon: 'hourglass', text: t('install.source.checking'), tone: 'muted' };
    switch (facts.state) {
      case 'not-github':
        return { icon: 'globe', text: t('install.source.notGithub'), tone: 'butter' };
      case 'unreachable':
        return { icon: 'cloudRain', text: t('install.source.unreachable'), tone: 'butter' };
      case 'repo-not-found':
        return { icon: 'alert', text: t('install.source.repoMissing'), tone: 'coral' };
      case 'no-repository':
        return { icon: 'lock', text: t('install.source.closed'), tone: 'coral' };
      case 'checked': {
        const r = facts.release;
        if (r.state === 'match') return { icon: 'check', text: t('install.source.match', { tag: r.tag }), tone: 'mint' };
        if (r.state === 'mismatch') return { icon: 'shieldOff', text: t('install.source.mismatch', { tag: r.tag }), tone: 'coral' };
        if (r.state === 'not-a-file') return { icon: 'file', text: t('install.source.dir'), tone: 'butter' };
        return { icon: 'shield', text: t('install.source.unverified'), tone: 'butter' };
      }
    }
  }

  /** 同一个 id 已经装着时：这次是重装、升级还是降级。降级要更显眼：旧版本可能带着已经修掉的问题 */
  function upgradeOf(c: PackageCheck): { text: string; down: boolean } | null {
    if (!c.installed) return null;
    const old = c.installed.version;
    const now = c.manifest.version;
    if (old === now) return { text: t('install.reinstall', { version: now }), down: false };
    const down = compareVersions(now, old) < 0;
    return { text: t(down ? 'install.downgrade' : 'install.upgrade', { old, new: now }), down };
  }

  /** 只比主、次、补丁三段与有没有预发布：够判断升级还是降级，不追求 semver 的全部规则 */
  function compareVersions(a: string, b: string): number {
    const parse = (v: string) => {
      const [core, pre] = v.split('-', 2);
      return { nums: core.split('.').map((x) => Number(x) || 0), pre: pre ?? null };
    };
    const x = parse(a);
    const y = parse(b);
    for (let i = 0; i < 3; i++) {
      const d = (x.nums[i] ?? 0) - (y.nums[i] ?? 0);
      if (d) return d;
    }
    if (x.pre === y.pre) return 0;
    if (x.pre === null) return 1;
    if (y.pre === null) return -1;
    return x.pre < y.pre ? -1 : 1;
  }

  const ROLES: (keyof ThemeColors)[] = ['primary', 'surface', 'text', 'ok', 'retry', 'error', 'warn'];

  function onKey(ev: KeyboardEvent) {
    if (ev.key === 'Escape') {
      ev.preventDefault();
      packages.cancel();
    } else if (ev.key === 'Tab' && dialog) {
      const items = [...dialog.querySelectorAll<HTMLElement>('button:not([disabled])')];
      const first = items[0];
      const lastItem = items[items.length - 1];
      if (ev.shiftKey && document.activeElement === first) {
        ev.preventDefault();
        lastItem.focus();
      } else if (!ev.shiftKey && document.activeElement === lastItem) {
        ev.preventDefault();
        first.focus();
      }
    }
  }

  function writeText(w: WriteKind): string {
    return t(`install.write.${w}`);
  }
</script>

{#if flow}
  <div class="scrim" in:fadeOut={{ duration: 180 }} out:fadeOut={{ duration: 140 }} onclick={() => packages.cancel()} aria-hidden="true"></div>
  <div class="layer">
    <div
      class="dialog"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="inst-title"
      aria-describedby="inst-body"
      tabindex="-1"
      bind:this={dialog}
      onkeydown={onKey}
      in:pop={{ y: 18, scale: 0.94, duration: 560 }}
      out:fadeOut={{ duration: 120 }}
    >
      {#if flow.step === 'reading' || flow.step === 'installing'}
        <div class="busy" id="inst-body">
          <span class="spinner" aria-hidden="true"></span>
          <h2 id="inst-title">{flow.step === 'reading' ? t('install.reading', { name: flow.name }) : t('install.installing', { name })}</h2>
        </div>
        {#if flow.step === 'reading'}
          <div class="actions">
            <button class="btn" bind:this={cancelBtn} onclick={() => packages.cancel()}>{t('install.cancel')}</button>
          </div>
        {/if}
      {:else if flow.step === 'failed'}
        <div class="head">
          <span class="badge coral" aria-hidden="true"><Icon name="alert" size={19} stroke={2.4} /></span>
          <h2 id="inst-title">{t('install.failed.title', { name: flow.name })}</h2>
        </div>
        <p class="body" id="inst-body">{faultText(flow.failure)}</p>
        <div class="actions">
          <button class="btn primary" bind:this={cancelBtn} onclick={() => packages.cancel()}>{t('install.ok')}</button>
        </div>
      {:else if review && check}
        {@const m = check.manifest}
        <!-- 装的是什么：每一条提醒都带着，免得看到后面忘了前面 -->
        <div class="who">
          {#if kind === 'theme' && theme?.ok}
            {@const colors = theme.spec[ui.variant] ?? theme.spec.light ?? theme.spec.dark ?? {}}
            <span class="avatar swatch" aria-hidden="true" style:--c1={colors.primary ?? 'var(--lilac)'} style:--c2={colors.surface ?? 'var(--surface)'}></span>
          {:else}
            <span class="avatar" aria-hidden="true">{name.slice(0, 1).toUpperCase()}</span>
          {/if}
          <span class="who-text">
            <span class="who-name">{name}<span class="ver">v{m.version}</span><span class="chip tiny">{t(`install.kind.${kind}`)}</span></span>
            <span class="who-sub">{t('install.by', { name: m.author })} · <code class="mono">{m.id}</code></span>
          </span>
          {#if total !== null && total > 1}
            <span class="steps" aria-label={t('install.step', { i: Math.min(review.index + 1, total), n: total })}>
              {#each review.warnings as _, i (i)}<i class:on={i <= review.index}></i>{/each}
            </span>
          {/if}
        </div>
        {@const up = upgradeOf(check)}
        {#if up}<p class="upgrade" class:down={up.down}><Icon name="repeat" size={14} />{up.text}</p>{/if}

        {#if waiting}
          <div class="busy" id="inst-body">
            <span class="spinner" aria-hidden="true"></span>
            <h2 id="inst-title">{t('install.waiting')}</h2>
          </div>
          <p class="body muted">{t('install.waitingBody')}</p>
        {:else if warning}
          {@const look = tone(warning)}
          <div class="head">
            <span class="badge {look.tone}" aria-hidden="true"><Icon name={look.icon} size={19} stroke={2.3} /></span>
            <h2 id="inst-title">
              {#if warning.kind === 'trust'}
                {t(kind === 'theme' ? 'install.trust.theme.title' : 'install.trust.ext.title', { name })}
              {:else if warning.kind === 'closed'}
                {t(kind === 'theme' ? 'install.closed.title.theme' : 'install.closed.title.extension')}
              {:else if warning.kind === 'not-github'}
                {t('install.notGithub.title')}
              {:else if warning.kind === 'unreachable'}
                {t('install.unreachable.title')}
              {:else if warning.kind === 'verify-failed'}
                {t('install.verifyFailed.title')}
              {:else if warning.kind === 'repo-missing'}
                {t('install.repoMissing.title')}
              {:else if warning.kind === 'release'}
                {releaseTitle(warning.release)}
              {:else}
                {t(warning.repo.stars < 50 ? 'install.rep.title.stars' : warning.repo.renamed ? 'install.rep.title.renamed' : 'install.rep.title.archived')}
              {/if}
            </h2>
          </div>

          <div class="body" id="inst-body">
            {#if warning.kind === 'trust'}
              {#if kind === 'theme'}
                <p>{t('install.trust.theme.lead')}</p>
                {#if theme && !theme.ok}
                  <p class="note coral"><Icon name="alert" size={14} />{t('install.trust.theme.invalid')}</p>
                {:else if theme?.ok}
                  <div class="preview" aria-hidden="true">
                    {#each ['light', 'dark'] as const as v (v)}
                      {@const colors = theme.spec[v]}
                      <span class="pv {v}">
                        <span class="pv-label">{t(`install.preview.${v}`)}</span>
                        {#each ROLES as role (role)}
                          <i class="sw" class:off={!colors?.[role]} style:background={colors?.[role] ?? 'transparent'}></i>
                        {/each}
                      </span>
                    {/each}
                  </div>
                {/if}
                {#if hasCss}
                  <p>{t('install.trust.theme.css')}</p>
                  <ul class="can">
                    <li><Icon name="globe" size={15} />{t('install.trust.theme.cssNet')}</li>
                    <li><Icon name="eyeOff" size={15} />{t('install.trust.theme.cssHide')}</li>
                  </ul>
                {/if}
              {:else}
                <p>{t('install.trust.ext.lead')}</p>
                <ul class="can">
                  <li><Icon name="eye" size={15} />{t('install.trust.ext.read')}</li>
                  <li><Icon name="key" size={15} />{t('install.trust.ext.config')}</li>
                  <li><Icon name="power" size={15} />{t('install.trust.ext.control')}</li>
                  <li><Icon name="globe" size={15} />{t('install.trust.ext.net')}</li>
                </ul>
              {/if}

              {#if m.network.length || m.writes.length || m.formats.length}
                <div class="declared">
                  <span class="declared-title">{t('install.declared')}</span>
                  {#if m.network.length}<span>{t('install.declared.network', { hosts: m.network.join(', ') })}</span>{/if}
                  {#if m.writes.length}<span>{t('install.declared.writes', { what: m.writes.map(writeText).join(ui.lang === 'en' ? ', ' : '、') })}</span>{/if}
                  {#if m.formats.length}<span>{t('install.declared.formats', { names: m.formats.join(', ') })}</span>{/if}
                </div>
              {/if}

              {@const src = sourceLine(check, review.facts, review.verifyFailed)}
              <div class="source">
                <span class="source-title">{t('install.source')}</span>
                {#if m.repository}<code class="mono repo">{m.repository}</code>{/if}
                <span class="source-line {src.tone}">
                  {#if src.icon === 'hourglass'}<span class="spinner tiny" aria-hidden="true"></span>{:else}<Icon name={src.icon} size={14} stroke={2.4} />{/if}
                  {src.text}
                </span>
                {#if review.facts?.state === 'checked'}
                  {@const r = review.facts.repo}
                  <span class="repo-facts">
                    <span><Icon name="star" size={13} />{t('install.source.stars', { n: fmtNum(r.stars) })}</span>
                    <span>{t('install.source.created', { date: day.format(r.createdAt) })}</span>
                    <span>{r.license ? t('install.source.license', { license: r.license }) : t('install.source.noLicense')}</span>
                  </span>
                {/if}
              </div>

              <div class="files">
                <button class="link" onclick={() => (showFiles = !showFiles)} aria-expanded={showFiles}>
                  <Icon name={showFiles ? 'chevronUp' : 'chevronDown'} size={14} />{t('install.files', { n: check.files.length, size: bytes(check.totalBytes) })}
                </button>
                {#if showFiles}
                  <ul class="file-list mono" in:pop={{ y: -4, scale: 1, duration: 260 }}>
                    {#each check.files as f (f.path)}<li><span>{f.path}</span><span class="num">{bytes(f.bytes)}</span></li>{/each}
                  </ul>
                {/if}
              </div>

              <p class="note">{t(kind === 'theme' ? 'install.trust.theme.note' : 'install.trust.ext.note')}</p>
            {:else if warning.kind === 'closed'}
              <p>{t(kind === 'theme' ? 'install.closed.body.theme' : 'install.closed.body.extension')}</p>
              <p class="note">{t('install.closed.advice')}</p>
            {:else if warning.kind === 'not-github'}
              <p>{t('install.notGithub.body', { url: warning.url })}</p>
            {:else if warning.kind === 'unreachable'}
              <p>{reasonText(warning.reason, warning.status)}</p>
              <p>{t('install.unreachable.body', { repo: repoName })}</p>
            {:else if warning.kind === 'verify-failed'}
              <p>{t('install.verifyFailed.body')}</p>
            {:else if warning.kind === 'repo-missing'}
              <p>{t('install.repoMissing.body', { repo: repoName })}</p>
            {:else if warning.kind === 'release'}
              <p>{releaseBody(warning.release, warning.repo.fullName, m.version)}</p>
              {#if warning.release.state !== 'not-a-file'}
                <p class="note">{t('install.release.borrowed')}</p>
              {/if}
            {:else}
              {@const r = warning.repo}
              {#if r.stars < 50}
                <p>{t('install.rep.stars', { repo: r.fullName, n: fmtNum(r.stars), forks: fmtNum(r.forks), date: day.format(r.createdAt) })}</p>
              {/if}
              {#if r.renamed}<p>{t('install.rep.renamed', { claimed: repoName, repo: r.fullName })}</p>{/if}
              {#if r.archived}<p>{t('install.rep.archived')}</p>{/if}
            {/if}
          </div>
        {/if}

        <div class="actions">
          <button class="btn" bind:this={cancelBtn} onclick={() => packages.cancel()}>{t('install.cancel')}</button>
          {#if !waiting}
            <button
              class="btn {warning && warning.kind !== 'trust' && tone(warning).tone === 'coral' ? 'danger' : 'primary'}"
              disabled={kind === 'theme' && !!theme && !theme.ok}
              onclick={() => packages.confirm()}
            >
              {#if !last}
                {t('install.continue')}
              {:else if warning?.kind === 'trust'}
                {t('install.install')}
              {:else}
                {t('install.installAnyway')}
              {/if}
            </button>
          {/if}
        </div>
      {/if}
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
    width: min(520px, 100%);
    max-height: calc(100vh - 48px);
    overflow-y: auto;
    padding: 20px 24px 20px;
    pointer-events: auto;
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--overlay-bg);
    box-shadow: var(--shadow-3);
    outline: none;
  }

  /* ---------- 装的是什么 ---------- */
  .who {
    display: flex;
    align-items: center;
    gap: 12px;
    padding-bottom: 14px;
    margin-bottom: 14px;
    border-bottom: var(--line-w-thin) dashed var(--line-soft);
  }
  .avatar {
    flex: none;
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: var(--r-md);
    background: var(--lilac-soft);
    color: var(--lilac-deep);
    font-weight: 700;
    font-size: var(--fs-lg);
  }
  /* 主题包的头像：主色压在表面色上的一块小样 */
  .avatar.swatch {
    background: linear-gradient(135deg, var(--c1) 0 50%, var(--c2) 50% 100%);
    border: var(--line-w-thin) solid var(--line);
  }
  .who-text {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 2px;
  }
  .who-name {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    font-weight: 620;
  }
  .ver {
    color: var(--text-2);
    font-weight: 500;
    font-size: var(--fs-sm);
  }
  .who-sub {
    color: var(--text-2);
    font-size: var(--fs-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .who-sub code {
    font-size: var(--fs-xs);
  }
  /* 第几条：每条一个小点，看过的填色 */
  .steps {
    flex: none;
    display: flex;
    gap: 4px;
  }
  .steps i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--line);
    transition: background calc(var(--motion) * 0.3s);
  }
  .steps i.on {
    background: var(--lilac);
  }
  .upgrade {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: -6px 0 12px;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .upgrade.down {
    color: var(--text);
    padding: 6px 10px;
    border-radius: var(--r-md);
    background: var(--butter-soft);
  }

  /* ---------- 这一条 ---------- */
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
    width: 38px;
    height: 38px;
    border-radius: 50%;
  }
  .badge.lilac {
    background: var(--lilac-soft);
    color: var(--lilac-deep);
  }
  .badge.butter {
    background: var(--butter-soft);
    color: var(--butter-deep);
  }
  .badge.coral {
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  h2 {
    margin: 0;
    font-size: var(--fs-xl);
    font-weight: 600;
    line-height: 1.3;
  }
  .body {
    margin: 0 0 18px;
    color: var(--text-2);
  }
  .body p {
    margin: 0 0 10px;
  }
  .body.muted {
    font-size: var(--fs-sm);
  }
  .can {
    display: grid;
    gap: 7px;
    margin: 0 0 14px;
    padding: 0;
    list-style: none;
    color: var(--text);
  }
  .can li {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    line-height: 1.45;
  }
  .can :global(svg) {
    flex: none;
    margin-top: 3px;
    color: var(--lilac-deep);
  }
  .note {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    padding: 9px 12px;
    border-radius: var(--r-md);
    background: var(--card-2-bg);
    color: var(--text);
    font-size: var(--fs-sm);
    line-height: 1.5;
  }
  .note.coral {
    background: var(--coral-soft);
  }
  .note :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .declared {
    display: grid;
    gap: 3px;
    margin: 0 0 12px;
    padding: 9px 12px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) dashed var(--line);
    font-size: var(--fs-sm);
    color: var(--text);
    overflow-wrap: anywhere;
  }
  .declared-title,
  .source-title {
    color: var(--text-2);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.02em;
  }
  .source {
    display: grid;
    gap: 4px;
    margin: 0 0 10px;
    padding: 10px 12px;
    border-radius: var(--r-md);
    background: var(--card-2-bg);
    font-size: var(--fs-sm);
  }
  .repo {
    color: var(--text);
    font-size: var(--fs-xs);
    overflow-wrap: anywhere;
  }
  .source-line {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text);
    font-weight: 560;
  }
  .source-line.mint :global(svg) {
    color: var(--mint-deep);
  }
  .source-line.coral {
    color: var(--coral-deep);
  }
  .source-line.butter :global(svg) {
    color: var(--butter-deep);
  }
  .source-line.muted {
    color: var(--text-2);
    font-weight: 500;
  }
  .repo-facts {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    color: var(--text-2);
  }
  .repo-facts span {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .files {
    margin: 0 0 12px;
  }
  .link {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 0;
    border: 0;
    background: none;
    color: var(--text-2);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .link:hover {
    color: var(--text);
  }
  .file-list {
    display: grid;
    gap: 2px;
    max-height: 160px;
    overflow-y: auto;
    margin: 6px 0 0;
    padding: 8px 10px;
    list-style: none;
    border-radius: var(--r-md);
    background: var(--card-2-bg);
    font-size: var(--fs-xs);
  }
  .file-list li {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }
  .file-list li span:first-child {
    overflow-wrap: anywhere;
  }
  .file-list .num {
    flex: none;
    color: var(--text-2);
  }

  /* 主题的小样：浅色、深色各一排色块 */
  .preview {
    display: grid;
    gap: 6px;
    margin: 0 0 12px;
  }
  .pv {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 7px 10px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line);
  }
  .pv-label {
    width: 4.5em;
    color: var(--text-2);
    font-size: var(--fs-xs);
  }
  .sw {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: var(--line-w-thin) solid var(--line);
  }
  .sw.off {
    border-style: dashed;
  }

  /* ---------- 等待与按钮 ---------- */
  .busy {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 6px 0 14px;
  }
  .busy h2 {
    font-size: var(--fs-lg);
  }
  .spinner {
    flex: none;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 2.5px solid var(--lilac-soft);
    border-top-color: var(--lilac-deep);
    animation: spin 0.9s linear infinite;
  }
  .spinner.tiny {
    width: 13px;
    height: 13px;
    border-width: 2px;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  :global([data-motion='reduced']) .spinner {
    animation-duration: 2.4s;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }
</style>

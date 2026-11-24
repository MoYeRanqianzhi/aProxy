/*
 * 已装的主题包与扩展包，以及安装的整个流程。
 *
 * 安装前的提醒是这套设计里唯一的保护（用户 2026-11-23 定：不做沙箱、不做逐项授权，
 * 装进一个危险的包本身就已经被攻陷了，防御只在安装之前有效）。所以流程是：
 *   1. 读包（check）：Rust 读、校验、算 sha256，立刻返回；同时发出 verify，去 GitHub 查仓库与 release。
 *   2. 第一个提醒总是有：这是什么、谁写的、装上之后能做什么。用户看这一条的时候，GitHub 的结果在路上。
 *   3. 按 GitHub 的结果再加提醒，每一条都要点确认：闭源、来源不在 GitHub、连不上 GitHub、仓库不存在、
 *      文件与 release 对不上（或没法比对）、star 少或仓库归档、改名。
 *   4. 最后一条确认之后才安装（install 带着检查时的 token，装的就是检查过的那份）。
 * 任何一步取消都丢掉暂存的包（discard）。一个窗口同时只走一个安装流程。
 *
 * 提醒怎么写、按什么顺序出现，在这里定（warningsFor）；文字在 i18n 的 inst.*，由 InstallDialog 排版。
 */
import { link } from './backend';
import type { GithubFacts, PackageCheck, PackageEntry, ReleaseFacts, RepoFacts, Text, Unreachable } from './backend/contract';
import { newGeneration, toFailure, type Failure } from './backend/link';
import { t } from './i18n';
import { overlays } from './overlays.svelte';
import { ui } from './state.svelte';
import { applyThemeCss, parseTheme } from './theme';

/** star 少于这个数就额外提醒一次（用户 2026-11-23 定） */
export const LOW_STARS = 50;
/** 文件选择框里选的包最大多少：与 Rust 一侧的上限相同，超了就不必传过去再被拒绝 */
const MAX_FILE = 64 * 1024 * 1024;

/** 安装前的一条提醒。trust 总是第一条；其余的按 GitHub 的结果决定有没有 */
export type Warning =
  | { kind: 'trust' }
  | { kind: 'closed' }
  | { kind: 'not-github'; url: string }
  | { kind: 'unreachable'; reason: Unreachable; status: number | null }
  | { kind: 'verify-failed' }
  | { kind: 'repo-missing' }
  | { kind: 'release'; release: Exclude<ReleaseFacts, { state: 'match' }>; repo: RepoFacts }
  | { kind: 'reputation'; repo: RepoFacts };

export type Flow =
  | { step: 'reading'; name: string }
  | { step: 'failed'; name: string; failure: Failure }
  /**
   * 正在逐条提醒。facts 为 null 时 GitHub 的结果还没回来；index 指向当前这一条，
   * 等于 warnings.length 时说明前面的都确认了、在等 GitHub
   */
  | { step: 'review'; check: PackageCheck; facts: GithubFacts | null; verifyFailed: boolean; warnings: Warning[]; index: number }
  | { step: 'installing'; check: PackageCheck };

/**
 * GitHub 的结果之后还要哪些提醒。
 * release 没对上时，仓库的 star 等数字说明不了这个包（谁都能在 manifest 里写一个热门仓库），
 * 所以那时只提醒对不上，不再按 star 提醒。
 */
export function warningsFor(facts: GithubFacts | null, verifyFailed: boolean): Warning[] {
  if (verifyFailed) return [{ kind: 'verify-failed' }];
  if (!facts) return [];
  switch (facts.state) {
    case 'no-repository':
      return [{ kind: 'closed' }];
    case 'not-github':
      return [{ kind: 'not-github', url: facts.url }];
    case 'unreachable':
      return [{ kind: 'unreachable', reason: facts.reason, status: facts.status }];
    case 'repo-not-found':
      return [{ kind: 'repo-missing' }];
    case 'checked': {
      const { repo, release } = facts;
      if (release.state !== 'match') return [{ kind: 'release', release, repo }];
      return repo.stars < LOW_STARS || repo.archived || repo.renamed ? [{ kind: 'reputation', repo }] : [];
    }
  }
}

/** 包作者写的一段文字按界面语言取：先找当前语言，再找英文，再随便取一份 */
export function textOf(x: Text | null | undefined, fallback = ''): string {
  if (x === null || x === undefined) return fallback;
  if (typeof x === 'string') return x;
  return x[ui.lang] ?? x.en ?? Object.values(x)[0] ?? fallback;
}

/** 一个包在界面上叫什么：manifest 不合格的包只剩 id 可以显示 */
export function packageName(e: PackageEntry): string {
  return textOf(e.manifest?.name, e.id);
}

class Packages {
  /** 按 id 排序；raw：每次整份替换 */
  list = $state.raw<PackageEntry[]>([]);
  /** 第一次读完了：之前设置页显示「正在读取」，主题也先不动 */
  loaded = $state(false);
  /** 最近一次读列表失败的原因 */
  readFault = $state.raw<Failure | null>(null);
  /** 启用的扩展，按加载的先后（desktop.toml 的 [extensions] enabled）；null 是这次没读成，沿用上一次的 */
  enabledOrder = $state.raw<string[]>([]);
  flow = $state.raw<Flow | null>(null);

  get themes(): PackageEntry[] {
    return this.list.filter((e) => e.kind === 'theme');
  }

  get extensions(): PackageEntry[] {
    return this.list.filter((e) => e.kind === 'extension');
  }

  get(id: string): PackageEntry | undefined {
    return this.list.find((e) => e.id === id);
  }

  /** 该加载的扩展：按 enabled 的先后，只取装着、合格、确实是扩展的 */
  get active(): PackageEntry[] {
    return this.enabledOrder.flatMap((id) => {
      const e = this.get(id);
      return e && e.kind === 'extension' && !e.issue && e.manifest ? [e] : [];
    });
  }

  async refresh(): Promise<void> {
    try {
      this.list = await link.call('package_list', {});
      this.loaded = true;
      this.readFault = null;
    } catch (raw) {
      this.readFault = toFailure(raw);
    }
  }

  /** desktop.toml 读到的启用列表；读不成（null）时保留手上的 */
  setEnabledOrder(order: string[] | null) {
    if (order) this.enabledOrder = order;
  }

  async setEnabled(id: string, enabled: boolean): Promise<void> {
    const entry = await link.call('package_set_enabled', { id, enabled });
    this.list = this.list.map((e) => (e.id === id ? entry : e));
    this.enabledOrder = enabled
      ? this.enabledOrder.includes(id)
        ? this.enabledOrder
        : [...this.enabledOrder, id]
      : this.enabledOrder.filter((x) => x !== id);
  }

  async uninstall(id: string, removeData: boolean): Promise<void> {
    await link.call('package_uninstall', { id, removeData });
    this.enabledOrder = this.enabledOrder.filter((x) => x !== id);
    // Rust 已把偏好里的主题换回 builtin；这里跟着换，免得等下一次读偏好才变
    if (ui.themeId === id) ui.themeId = 'builtin';
    await this.refresh();
  }

  // -------------------------------------------------------------------------
  // 安装
  // -------------------------------------------------------------------------

  /** 能不能开始一次新的安装：已经有一个在走（不算失败的那种）就不行 */
  get busy(): boolean {
    return !!this.flow && this.flow.step !== 'failed';
  }

  /** 文件选择框或浏览器里拖进来的文件 */
  async beginFile(file: File): Promise<void> {
    if (this.busy) return;
    if (file.size > MAX_FILE) {
      this.flow = {
        step: 'failed',
        name: file.name,
        failure: { code: 'desktop_package', details: { reason: 'limit', field: null, path: null, limit: 'zip-bytes', max: MAX_FILE } },
      };
      return;
    }
    this.flow = { step: 'reading', name: file.name };
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      this.review(await link.call('package_check_file', { name: file.name, bytes }));
    } catch (raw) {
      this.flow = { step: 'failed', name: file.name, failure: toFailure(raw) };
    }
  }

  /** 拖进桌面窗口的文件或目录（Tauri 给的是路径） */
  async beginPath(path: string): Promise<void> {
    if (this.busy) return;
    const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
    this.flow = { step: 'reading', name };
    try {
      this.review(await link.call('package_check_path', { path }));
    } catch (raw) {
      this.flow = { step: 'failed', name, failure: toFailure(raw) };
    }
  }

  private review(check: PackageCheck) {
    this.flow = { step: 'review', check, facts: null, verifyFailed: false, warnings: [{ kind: 'trust' }], index: 0 };
    const token = check.token;
    link.call('package_verify', { token }).then(
      (facts) => this.onFacts(token, facts, false),
      () => this.onFacts(token, null, true),
    );
  }

  private onFacts(token: string, facts: GithubFacts | null, verifyFailed: boolean) {
    const f = this.flow;
    if (f?.step !== 'review' || f.check.token !== token) return;
    const warnings: Warning[] = [{ kind: 'trust' }, ...warningsFor(facts, verifyFailed)];
    const next = { ...f, facts, verifyFailed, warnings };
    // 前面的都确认过了、只在等 GitHub，而 GitHub 的结果没有新的提醒：直接装
    if (f.index >= warnings.length) void this.install(next.check);
    else this.flow = next;
  }

  /** 用户确认了当前这一条 */
  confirm() {
    const f = this.flow;
    if (f?.step !== 'review') return;
    const index = f.index + 1;
    if (index < f.warnings.length || (f.facts === null && !f.verifyFailed)) this.flow = { ...f, index };
    else void this.install(f.check);
  }

  /** 用户在任何一步取消：丢掉暂存的包 */
  cancel() {
    const f = this.flow;
    if (f?.step === 'installing') return;
    this.flow = null;
    if (f?.step === 'review') void link.call('package_discard', { token: f.check.token }).catch(() => {});
  }

  private async install(check: PackageCheck) {
    this.flow = { step: 'installing', check };
    const name = textOf(check.manifest.name, check.manifest.id);
    try {
      const entry = await link.call('package_install', { token: check.token, enable: true });
      await this.refresh();
      if (entry.kind === 'theme') {
        // 装主题就是为了用它：直接换上，不喜欢可以在设置里换回去
        ui.themeId = entry.id;
        overlays.toast(t('install.done.theme', { name }));
      } else {
        if (entry.enabled && !this.enabledOrder.includes(entry.id)) this.enabledOrder = [...this.enabledOrder, entry.id];
        overlays.toast(t(entry.enabled ? 'install.done.extension' : 'install.done.extensionOff', { name }));
      }
      this.flow = null;
    } catch (raw) {
      this.flow = { step: 'failed', name, failure: toFailure(raw) };
    }
  }

  // -------------------------------------------------------------------------
  // 主题：偏好里的 theme 选中哪个包，就把它的 theme.json 解析成令牌、theme.css 挂上去
  // -------------------------------------------------------------------------

  /** 上一次挂上去的样式表属于哪个包的哪个版本：列表重读时不必重新挂（演示里每次取地址都是新的 blob:） */
  private cssKey = '';

  /** 在根组件里调用一次：之后主题跟着偏好与列表自动变 */
  watchTheme() {
    $effect.root(() => {
      $effect(() => {
        if (!this.loaded) return;
        const id = ui.themeId;
        const entry = id === 'builtin' ? undefined : this.list.find((e) => e.id === id && e.kind === 'theme' && !e.issue);
        const parsed = entry?.theme ? parseTheme(entry.theme) : null;
        ui.themeSpec = parsed?.ok ? parsed.spec : null;
        const key = entry ? `${entry.id}@${entry.manifest?.version}@${entry.installedAt}` : '';
        if (key === this.cssKey) return;
        this.cssKey = key;
        // 每换一次都取新代号：升级后的样式表与它相对引用的图片、字体不会沿用页面缓存里的旧内容
        applyThemeCss(entry ? link.packageUrl(entry.id, 'theme.css', newGeneration()) : null);
      });
    });
  }
}

export const packages = new Packages();

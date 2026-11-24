/*
 * 扩展的宿主：按 desktop.toml 的启用列表加载扩展，交给每个扩展一个 api（接口第 1 版，见 api.ts 的类型），
 * 记下它加的页面、挂进来的面板与吉祥物，停用或卸载时全部收回。
 *
 * 扩展直接在窗口里运行（用户 2026-11-23 定：不做沙箱）：它是一个 ES module，从包的地址加载
 * （link.packageUrl，桌面窗口里是 aproxy-pkg 协议），导出 activate(api) 与可选的 deactivate()。
 * 这里能做的只是让一个出错的扩展不拖垮别的：activate、页面的 render、各种回调都包在 try/catch 里，
 * 出错的扩展在设置页标出原因。挡不住一个有意作恶的扩展——那由安装前的提醒负责。
 *
 * 同一个模块地址在页面的一生里只会执行一次，所以每次加载都取一个新的激活代号，写进包地址的第一段
 * （见 link.packageUrl）：升级、停用后再启用，入口与它相对 import 的子模块、引用的图片字体都是新地址、新代码。
 * 旧模块的代码还在内存里，但它的页面、面板、订阅都已经收回。
 */
import { untrack } from 'svelte';
import { link } from '../backend';
import type { PackageEntry } from '../backend/contract';
import { newGeneration, toFailure } from '../backend/link';
import { faultText } from '../faults';
import { mascots } from '../mascot/registry.svelte';
import type { MascotDefinition } from '../mascot/types';
import { packages } from '../packages.svelte';
import { ui } from '../state.svelte';
import { createApi, type ExtensionModule, type MountPoint, type PageSpec, type Registrar } from './api.svelte';

/** 一个扩展此刻的状态，设置页据此显示「运行中」「出错了」 */
export type RunState = { state: 'loading' } | { state: 'running' } | { state: 'failed'; message: string };

/** 扩展经 api.ui.addPage 加的一页 */
export interface ExtPage {
  /** 「扩展 id/页面 id」：路由与导航用它 */
  key: string;
  extension: string;
  spec: PageSpec;
}

/** 扩展经 api.ui.mount 挂进某个位置的一块 */
export interface ExtMount {
  id: number;
  extension: string;
  point: MountPoint;
  render: (el: HTMLElement, ctx: Record<string, unknown>) => void | (() => void);
}

/** 一个加载着的扩展：收回它时要撤掉的东西都记在 cleanups 里 */
interface Loaded {
  id: string;
  /** 加载的是哪个版本、哪次安装：变了就重新加载 */
  stamp: string;
  module: ExtensionModule | null;
  cleanups: (() => void)[];
}

/** 把扩展的错误整理成一句给人看的话：Fault 按码翻译，普通异常取它的 message（截短） */
export function errorText(err: unknown): string {
  const f = toFailure(err);
  if (f.code !== 'transport') return faultText(f);
  const msg = err instanceof Error ? err.message : String(err);
  return msg.length > 300 ? `${msg.slice(0, 300)}…` : msg;
}

class ExtensionHost {
  states = $state.raw<Record<string, RunState>>({});
  pages = $state.raw<ExtPage[]>([]);
  mounts = $state.raw<ExtMount[]>([]);

  private loaded = new Map<string, Loaded>();
  private serial = 0;
  /** 同一时刻只做一次同步：加载慢的扩展不会和下一次同步交错 */
  private syncing: Promise<void> = Promise.resolve();

  private setState(id: string, s: RunState | null) {
    const next = { ...this.states };
    if (s) next[id] = s;
    else delete next[id];
    this.states = next;
  }

  /** 在根组件里调用一次：之后启用列表或包的列表一变，就加载新启用的、收回停用的 */
  start() {
    $effect.root(() => {
      $effect(() => {
        if (!packages.loaded) return;
        const want = packages.active.map((e) => ({ id: e.id, stamp: stampOf(e), entry: e }));
        untrack(() => {
          this.syncing = this.syncing.then(() => this.sync(want));
        });
      });
    });
  }

  private async sync(want: { id: string; stamp: string; entry: PackageEntry }[]) {
    const keep = new Set(want.map((w) => `${w.id}@${w.stamp}`));
    for (const l of [...this.loaded.values()]) {
      if (!keep.has(`${l.id}@${l.stamp}`)) await this.unload(l.id);
    }
    for (const w of want) {
      if (!this.loaded.has(w.id)) await this.load(w.entry, w.stamp);
    }
  }

  private async load(entry: PackageEntry, stamp: string) {
    const id = entry.id;
    const l: Loaded = { id, stamp, module: null, cleanups: [] };
    this.loaded.set(id, l);
    this.setState(id, { state: 'loading' });
    // 这一次加载的代号：入口模块与 api.assets.url 给出的地址都用它，扩展自己拼的相对地址也会带上它
    const generation = newGeneration();
    const url = link.packageUrl(id, 'main.mjs', generation);
    try {
      if (!url) throw new Error('main.mjs is missing');
      const mod = (await import(/* @vite-ignore */ url)) as ExtensionModule;
      if (this.loaded.get(id) !== l) return;
      if (typeof mod.activate !== 'function') throw new Error('main.mjs does not export activate(api)');
      l.module = mod;
      await mod.activate(createApi(entry, this.registrar(l), generation));
      if (this.loaded.get(id) !== l) return;
      this.setState(id, { state: 'running' });
    } catch (err) {
      if (this.loaded.get(id) !== l) return;
      console.warn(`[extension ${id}]`, err);
      // 出错的扩展把已经挂上的东西收回，但留在 loaded 里：同一个版本不反复重试，换了版本或重新启用才再加载
      this.revoke(l);
      this.setState(id, { state: 'failed', message: errorText(err) });
    }
  }

  private async unload(id: string) {
    const l = this.loaded.get(id);
    if (!l) return;
    this.loaded.delete(id);
    try {
      await l.module?.deactivate?.();
    } catch (err) {
      console.warn(`[extension ${id}] deactivate`, err);
    }
    this.revoke(l);
    this.setState(id, null);
  }

  private revoke(l: Loaded) {
    for (const c of l.cleanups.splice(0).reverse()) {
      try {
        c();
      } catch (err) {
        console.warn(`[extension ${l.id}] cleanup`, err);
      }
    }
  }

  /** api 里登记页面、面板、吉祥物与订阅的那几个函数：登记的东西都记到这个扩展名下，收回时一起撤 */
  private registrar(l: Loaded): Registrar {
    const alive = () => this.loaded.get(l.id) === l;
    return {
      alive,
      track: (cleanup: () => void) => {
        if (alive()) l.cleanups.push(cleanup);
        else cleanup();
        return cleanup;
      },
      addPage: (spec: PageSpec) => {
        const key = `${l.id}/${spec.id}`;
        this.pages = [...this.pages.filter((p) => p.key !== key), { key, extension: l.id, spec }];
        const remove = () => {
          this.pages = this.pages.filter((p) => p.key !== key || p.spec !== spec);
          if (ui.page === 'ext' && ui.extPage === key && !this.pages.some((p) => p.key === key)) ui.go('settings');
        };
        l.cleanups.push(remove);
        return remove;
      },
      mount: (point: MountPoint, render: ExtMount['render']) => {
        const m: ExtMount = { id: ++this.serial, extension: l.id, point, render };
        this.mounts = [...this.mounts, m];
        const remove = () => (this.mounts = this.mounts.filter((x) => x !== m));
        l.cleanups.push(remove);
        return remove;
      },
      mascot: (def: MascotDefinition) => {
        const remove = mascots.addExtension(l.id, def);
        l.cleanups.push(remove);
        return remove;
      },
    };
  }

  page(key: string): ExtPage | undefined {
    return this.pages.find((p) => p.key === key);
  }
}

/** 同一个扩展换了版本或重新装过，就当成要重新加载的另一份 */
function stampOf(e: PackageEntry): string {
  return `${e.manifest?.version ?? ''}@${e.installedAt ?? ''}`;
}

export const host = new ExtensionHost();

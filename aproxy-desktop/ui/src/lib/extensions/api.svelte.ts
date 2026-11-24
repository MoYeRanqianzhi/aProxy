/*
 * 交给扩展的 api：扩展接口的第 1 版。以后只加不改：已有的名字、参数与返回值保持原样，扩展可以靠它们长期工作。
 *
 * 它是界面 store 的一层薄包装：读的是界面看得到的那些数据（实例卡片、最近的请求、日志、请求详情，
 * 密钥都已在 Rust 一侧打码），写的是界面能做的那些操作（启停实例、改配置、装更新）。写操作不另外问用户：
 * 扩展能做这些，安装前的提醒已经讲清楚了（用户 2026-11-23 定）。
 * 扩展自己的东西都在标准的位置：设置在 desktop.toml 的 [extensions.settings."<id>"]（api.settings），
 * 数据在 <home>/desktop/extension-data/<id>/（api.storage、api.files），包里的文件经 api.assets 取地址。
 *
 * 订阅（onChange 之类）返回一个取消函数；扩展停用时宿主会替它全部取消。回调里抛出的异常只记到控制台。
 */
import { untrack } from 'svelte';
import { link } from '../backend';
import type {
  BodyPart,
  Card,
  Choice,
  Chunk,
  ConfigTarget,
  ConfigView,
  DataFile,
  Detail,
  Edit,
  FormatView,
  InstallEnd,
  LogPage,
  PackageEntry,
  Ref,
  RestartOutcome,
  Row,
  Run,
  Saved,
  StartTarget,
  Text,
  Token,
  UpdateInfo,
  Validation,
} from '../backend/contract';
import type { MascotDefinition, MascotMood } from '../mascot/types';
import { overlays } from '../overlays.svelte';
import type { Page } from '../state.svelte';
import { ui } from '../state.svelte';
import { store } from '../store.svelte';

/** 扩展模块导出的东西 */
export interface ExtensionModule {
  activate(api: ExtensionApi): void | Promise<void>;
  deactivate?(): void | Promise<void>;
}

/** 扩展可以挂面板的位置：总览页顶上、实例详情页顶上（ctx 带着这个实例的端口） */
export type MountPoint = 'overview.top' | 'instance.top';

/** 扩展加的一页 */
export interface PageSpec {
  /** 在这个扩展里唯一：小写字母、数字与 -，1–40 个字符 */
  id: string;
  title: Text;
  /** 界面自带的图标名（Icon.svelte 里的那些）；认不出的用拼图 */
  icon?: string;
  /** 页面打开时调用，把内容画进 el；返回的函数在页面关闭时调用 */
  render(el: HTMLElement): void | (() => void);
}

/** 宿主替一个扩展登记东西的入口：登记的都记在这个扩展名下，停用时一起撤掉 */
export interface Registrar {
  alive(): boolean;
  track(cleanup: () => void): () => void;
  addPage(spec: PageSpec): () => void;
  mount(point: MountPoint, render: (el: HTMLElement, ctx: Record<string, unknown>) => void | (() => void)): () => void;
  mascot(def: MascotDefinition): () => void;
}

/** 界面此刻的概况 */
export interface UiStateView {
  page: Page;
  lang: 'zh-CN' | 'en';
  variant: 'light' | 'dark';
  mood: MascotMood;
  retrying: number;
  inFlight: number;
}

export interface ExtensionApi {
  readonly version: 1;
  /** 这个扩展自己（来自装着的 manifest） */
  readonly self: { id: string; version: string };

  instances: {
    /** 当前的实例卡片，按端口升序 */
    list(): Card[];
    onChange(cb: () => void): () => void;
    /** 能启动的配置 */
    configs(): Promise<Choice[]>;
    start(target: StartTarget): Promise<Run>;
    stop(run: Run): Promise<null>;
    restart(run: Run): Promise<Run>;
  };
  requests: {
    /** 整个 home 最近的 50 条，新的在前 */
    recent(): Row[];
    onChange(cb: () => void): () => void;
    /** 按运行（null 为整个 home）分页读请求，cursor 来自上一页 */
    query(run: Run | null, cursor: Token | null, limit: number): Promise<LogPage>;
    /** 打开一条请求：拿到详情，再按块读正文；用完调用 close */
    open(ref: Ref): Promise<{ detail: Detail; read(part: BodyPart, offset: number, limit: number): Promise<Chunk>; close(): Promise<void> }>;
  };
  config: {
    read(target: ConfigTarget): Promise<ConfigView>;
    validate(target: ConfigTarget, edits: Edit[]): Promise<Validation>;
    write(target: ConfigTarget, edits: Edit[], acknowledgeExternalAcl?: boolean): Promise<Saved>;
  };
  updates: {
    check(channel: 'auto' | 'pre'): Promise<UpdateInfo>;
    install(check: Token): Promise<InstallEnd>;
  };
  /**
   * 一个运行中实例的 [[format]] 条目。用条目名（配置里的 name）选条目；FormatView.program 是程序在握手时报的名字，
   * 扩展据它认出与自己配对的 format（manifest 的 formats），但两个条目可以跑同一个程序，要按条目名操作
   */
  formats: {
    /** 这次运行的条目，按配置里的顺序 */
    list(run: Run): Promise<FormatView[]>;
    /**
     * 调用条目的 control 方法，兑现的是程序给的结果（可能是 null）。程序明确拒绝时以 desktop_format_fail 拒绝，
     * code 与 reason 是程序自己写的；宿主这边没办成时以 desktop_format 拒绝。params 省略时发 null
     */
    call(run: Run, name: string, method: string, params?: unknown): Promise<unknown>;
    /** 不换实例、只重启这些条目（null 是全部）；config.toml 不重读，在途的请求在旧进程上照常收尾 */
    restart(run: Run, names: string[] | null): Promise<RestartOutcome[]>;
  };
  ui: {
    state(): UiStateView;
    onState(cb: (s: UiStateView) => void): () => void;
    /** 界面的颜色令牌此刻解析出来的值（--lilac、--text、--surface…），主题或深浅模式变了会不同 */
    tokens(): Record<string, string>;
    addPage(spec: PageSpec): () => void;
    mount(point: MountPoint, render: (el: HTMLElement, ctx: Record<string, unknown>) => void | (() => void)): () => void;
    /** 加一个吉祥物，用户可以在设置里选它；定义的写法见 MascotDefinition */
    registerMascot(def: MascotDefinition): () => void;
    toast(text: string, tone?: 'mint' | 'lilac' | 'coral'): void;
    /** 打开界面自己的某一页 */
    navigate(page: 'overview' | 'logs' | 'update' | 'settings'): void;
  };
  settings: {
    get(): Promise<Record<string, unknown>>;
    /** 值为 null 的键被删掉；返回改完的整张表 */
    set(changes: Record<string, unknown>): Promise<Record<string, unknown>>;
    /** 用户手改了 desktop.toml 里这张表（窗口回到前台时发现） */
    onChange(cb: (values: Record<string, unknown>) => void): () => void;
  };
  storage: {
    get(): Promise<Record<string, unknown>>;
    set(changes: Record<string, unknown>): Promise<Record<string, unknown>>;
    /** 从空对象（或给定的内容）重新开始 */
    reset(values?: Record<string, unknown>): Promise<Record<string, unknown>>;
  };
  files: {
    list(): Promise<DataFile[]>;
    read(name: string): Promise<Uint8Array>;
    readText(name: string): Promise<string>;
    write(name: string, data: Uint8Array | string): Promise<DataFile>;
    delete(name: string): Promise<void>;
  };
  assets: {
    /** 包里一个文件的地址，例如 url('assets/logo.svg') */
    url(path: string): string | null;
  };
}

/** 令牌里扩展最常用的几个；完整的一套在 app.css */
const TOKENS = [
  '--text',
  '--text-2',
  '--text-3',
  '--surface',
  '--sheet-bg',
  '--card-bg',
  '--card-2-bg',
  '--line',
  '--lilac',
  '--lilac-soft',
  '--lilac-deep',
  '--mint',
  '--mint-soft',
  '--sky',
  '--sky-soft',
  '--coral',
  '--coral-soft',
  '--butter',
  '--butter-soft',
  '--font-ui',
  '--font-mono',
];

const PAGE_ID = /^[a-z0-9-]{1,40}$/;

/**
 * 跟踪 read 读到的响应式数据，变了就调用 cb（第一次不调用：扩展自己先读一次现状）。
 * 回调延到微任务里、并且不跟踪：扩展在回调里读别的数据，不会让这个订阅连带着重跑
 */
function watch(read: () => unknown, cb: () => void): () => void {
  let first = true;
  return $effect.root(() => {
    $effect(() => {
      read();
      if (first) {
        first = false;
        return;
      }
      untrack(() =>
        queueMicrotask(() => {
          try {
            cb();
          } catch (err) {
            console.warn('[extension callback]', err);
          }
        }),
      );
    });
  });
}

function uiState(): UiStateView {
  return { page: ui.page, lang: ui.lang, variant: ui.variant, mood: store.mood, retrying: store.retrying, inFlight: store.inflight };
}

export function createApi(entry: PackageEntry, reg: Registrar, generation: string): ExtensionApi {
  const id = entry.id;
  const sub = (read: () => unknown, cb: () => void) => reg.track(watch(read, cb));

  return Object.freeze({
    version: 1,
    self: Object.freeze({ id, version: entry.manifest?.version ?? '' }),

    instances: {
      list: () => structuredClone(store.cards) as Card[],
      onChange: (cb: () => void) => sub(() => store.cards, cb),
      configs: () => store.listConfigs(),
      start: (target: StartTarget) => store.start(target),
      stop: (run: Run) => store.stop(run),
      restart: (run: Run) => store.restart(run),
    },

    requests: {
      recent: () => structuredClone(store.recent) as Row[],
      onChange: (cb: () => void) => sub(() => store.recent, cb),
      query: (run: Run | null, cursor: Token | null, limit: number) => store.queryLogs(run, cursor, limit),
      async open(ref: Ref) {
        const { detail, selection } = await store.openDetail(ref);
        return {
          detail,
          read: (part: BodyPart, offset: number, limit: number) => store.bodyChunk(selection, ref, part, offset, limit),
          close: async () => {
            await store.cancelDetail(selection);
          },
        };
      },
    },

    config: {
      read: (target: ConfigTarget) => store.readConfig(target),
      validate: (target: ConfigTarget, edits: Edit[]) => store.validateConfig(target, edits),
      write: (target: ConfigTarget, edits: Edit[], acknowledgeExternalAcl = false) => store.writeConfig(target, edits, acknowledgeExternalAcl),
    },

    updates: {
      check: (channel: 'auto' | 'pre') => store.checkUpdate(channel),
      install: (check: Token) => store.installUpdate(check),
    },

    formats: {
      list: (run: Run) => store.formatList(run),
      call: (run: Run, name: string, method: string, params: unknown = null) => store.formatCall(run, name, method, params),
      restart: (run: Run, names: string[] | null) => store.formatRestart(run, names),
    },

    ui: {
      state: uiState,
      onState: (cb: (s: UiStateView) => void) => sub(uiState, () => cb(uiState())),
      tokens() {
        const style = getComputedStyle(document.documentElement);
        return Object.fromEntries(TOKENS.map((name) => [name, style.getPropertyValue(name).trim()]));
      },
      addPage(spec: PageSpec) {
        if (!spec || !PAGE_ID.test(String(spec.id)) || typeof spec.render !== 'function') {
          throw new TypeError('addPage needs { id: /^[a-z0-9-]{1,40}$/, title, render(el) }');
        }
        return reg.addPage({ id: spec.id, title: spec.title, icon: spec.icon, render: spec.render });
      },
      mount(point: MountPoint, render: (el: HTMLElement, ctx: Record<string, unknown>) => void | (() => void)) {
        if (point !== 'overview.top' && point !== 'instance.top') throw new TypeError(`unknown mount point: ${String(point)}`);
        if (typeof render !== 'function') throw new TypeError('mount needs a render(el, ctx) function');
        return reg.mount(point, render);
      },
      registerMascot(def: MascotDefinition) {
        if (!def || typeof def.id !== 'string' || typeof def.render !== 'function' || typeof def.icon !== 'function') {
          throw new TypeError('registerMascot needs { id, name: { en }, render(state), icon(opts) }');
        }
        return reg.mascot(def);
      },
      toast(text: string, tone: 'mint' | 'lilac' | 'coral' = 'mint') {
        if (reg.alive()) overlays.toast(String(text).slice(0, 200), tone);
      },
      navigate(page: 'overview' | 'logs' | 'update' | 'settings') {
        if (['overview', 'logs', 'update', 'settings'].includes(page)) ui.go(page);
      },
    },

    settings: {
      get: () => link.call('extension_settings_read', { id }),
      set: (changes: Record<string, unknown>) => link.call('extension_settings_write', { id, changes }),
      onChange(cb: (values: Record<string, unknown>) => void) {
        // 窗口回到前台、重读 desktop.toml 之后比一比：这张表变了才告诉扩展
        let last: string | null = null;
        void link.call('extension_settings_read', { id }).then(
          (v) => (last = JSON.stringify(v)),
          () => {},
        );
        return sub(
          () => store.desktopReads,
          () => {
            void link.call('extension_settings_read', { id }).then(
              (v) => {
                const now = JSON.stringify(v);
                if (last !== null && now !== last && reg.alive()) cb(v);
                last = now;
              },
              () => {},
            );
          },
        );
      },
    },

    storage: {
      get: () => link.call('extension_storage_read', { id }),
      set: (changes: Record<string, unknown>) => link.call('extension_storage_write', { id, changes, reset: false }),
      reset: (values: Record<string, unknown> = {}) => link.call('extension_storage_write', { id, changes: values, reset: true }),
    },

    files: {
      list: () => link.call('extension_files_list', { id }),
      read: (name: string) => link.call('extension_file_read', { id, name }),
      readText: async (name: string) => new TextDecoder().decode(await link.call('extension_file_read', { id, name })),
      write: (name: string, data: Uint8Array | string) =>
        link.call('extension_file_write', { id, name, bytes: typeof data === 'string' ? new TextEncoder().encode(data) : data }),
      delete: async (name: string) => {
        await link.call('extension_file_delete', { id, name });
      },
    },

    assets: {
      url: (path: string) => link.packageUrl(id, String(path).replace(/^\.?\//, ''), generation),
    },
  } satisfies ExtensionApi);
}

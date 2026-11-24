/*
 * 传输层的演示实现：浏览器里开发与截图时用，背后是模拟引擎而不是真的实例。
 *
 * 对 store 而言它与桌面窗口里的实现（../tauri.ts）没有区别：同一张命令表、同一种事件、同一种错误。
 * 为了让「在演示上跑通」能说明问题，这里刻意照着真实的规矩来：
 *   - 每个命令都异步、带一点延迟，交出去的都是新造的对象；
 *   - 事件只说「哪块数据旧了」，不带数据；上一个事件处理完才发下一个，积压的同类通知合并成一个；
 *   - 操作带着运行的标识来，标识对不上（实例已经重启过）就拒绝，和真实的一样以 Fault 拒绝。
 */
import type {
  About,
  Bookmark,
  Choice,
  CommandName,
  Commands,
  ConfigTarget,
  DesktopEvent,
  Fault,
  InstallEnd,
  InstallPhase,
  Job,
  Preferences,
  Registration,
  Row,
  Run,
  Snapshot,
  StartTarget,
  UpdateInfo,
} from '../contract';
import type { EventHandler, Link } from '../link';
import type { Instance } from '../../types';
import type { BackendEvent } from '../types';
import { Engine } from '../mock/engine';
import type { SimEntry } from '../mock/fake';
import { applyEdits, docFor, fileToken, listenPort, validate, viewOf, type Doc } from './config';
import { bodyBytes, cardOf, detailOf, restoreToken, rowOf, RunBook, runOf, toBase64, watchdogOf } from './project';
import { demoPackageUrl, enabledExtensions, packageHandlers } from './packages';

const SESSION = 'demo';
/** 快照里带多少条最近的请求 */
const RECENT = 50;

const engine = new Engine();
const book = new RunBook();

/** 模拟跨进程的往返时间：短到察觉不到，但足以让「以为是同步的」写法露馅 */
function later<T>(value: T, ms = 25): Promise<T> {
  return new Promise((resolve) => setTimeout(() => resolve(value), ms));
}

function fault<C extends Fault['code']>(code: C, details: Extract<Fault, { code: C }>['details']): Fault {
  return { code, details } as Fault;
}

// ---------------------------------------------------------------------------
// 实例：哪些出现在界面上
// ---------------------------------------------------------------------------

/** 停着、但留着「应当在运行」记录的实例：以「等待恢复」出现在界面上 */
const restorePending = new Set<number>([12360]);

/** 界面上有卡片的实例：在运行的，加上等待恢复的。用户自己停掉的不出现 */
function shown(): Instance[] {
  return engine.instances.filter((i) => i.status !== 'stopped' || restorePending.has(i.port)).sort((a, b) => a.port - b.port);
}

/** 按运行的标识找实例；标识对不上说明它已经不是那次运行了 */
function liveRun(run: Run): Instance {
  const inst = engine.instances.find((i) => i.port === run.port);
  if (!inst || runOf(inst)?.instanceId !== run.instanceId) throw fault('desktop_gone', {});
  return inst;
}

function rows(entries: SimEntry[]): Row[] {
  return entries.map((e) => {
    const inst = engine.instance(e.port);
    return rowOf(e, book.bind(e, inst), inst);
  });
}

function findEntry(run: Run, requestId: string): SimEntry {
  const e = engine.visibleLogs.find((x) => x.port === run.port && String(x.seq) === requestId);
  if (!e || book.bind(e, engine.instance(e.port)).instanceId !== run.instanceId) throw fault('desktop_history', {});
  return e;
}

// 引擎里已有的请求都记在各实例当前这次运行名下
rows(engine.logs);

function snapshot(): Snapshot {
  const logs = engine.visibleLogs;
  return {
    instances: shown().map((i) => cardOf(i, logs, restorePending.has(i.port))),
    recent: rows(logs.slice(-RECENT)).reverse(),
    watchdog: watchdogOf(engine.stats.watchdog),
    update: job,
  };
}

// ---------------------------------------------------------------------------
// 事件：引擎的增量变成「哪块数据旧了」的通知
// ---------------------------------------------------------------------------

let handler: EventHandler | null = null;
const queue: DesktopEvent[] = [];
let draining = false;

/** 同一种通知积压时只留一个；庆祝各算各的；安装进度只留最新的那份 */
function sameNotice(a: DesktopEvent, b: DesktopEvent): boolean {
  if (a.type !== b.type || a.type === 'rescued') return false;
  if (a.type === 'requests-changed' && b.type === 'requests-changed') return a.run.port === b.run.port && a.run.instanceId === b.run.instanceId;
  return true;
}

function notify(event: DesktopEvent) {
  const i = queue.findIndex((q) => sameNotice(q, event));
  if (i >= 0) queue[i] = event;
  else queue.push(event);
  void drain();
}

async function drain() {
  if (draining || !handler) return;
  draining = true;
  while (queue.length) {
    const event = queue.shift()!;
    try {
      await handler(event);
    } catch (err) {
      console.warn('[demo] event handler failed', err);
    }
  }
  draining = false;
}

/** 上次通知时每条请求的样子：引擎每拍都报在途请求的耗时，耗时由界面自己按时钟算，不值得为它通知 */
const seen = new Map<string, string>();

function requestsChanged(entries: { key: string }[]) {
  const runs = new Map<string, Run>();
  for (const { key } of entries) {
    const e = engine.entry(key);
    if (!e) continue;
    const mark = `${e.outcome}/${e.phase}/${e.attempts.length}/${e.heartbeats}`;
    if (seen.get(key) === mark) continue;
    seen.set(key, mark);
    const run = book.bind(e, engine.instance(e.port));
    runs.set(`${run.port}/${run.instanceId}`, run);
  }
  for (const run of runs.values()) notify({ type: 'requests-changed', run });
  // 在途、重试中的数目在实例的卡片上
  if (runs.size) notify({ type: 'instances-changed' });
}

function onEngine(ev: BackendEvent) {
  switch (ev.type) {
    case 'snapshot':
      notify({ type: 'resync' });
      break;
    case 'instances':
    case 'stats':
      notify({ type: 'instances-changed' });
      break;
    case 'logs-appended':
    case 'logs-updated':
      requestsChanged(ev.entries);
      break;
    case 'logs-removed':
      for (const key of ev.keys) seen.delete(key);
      notify({ type: 'instances-changed' });
      break;
    case 'rescued': {
      const e = engine.entry(ev.key);
      if (e) notify({ type: 'rescued', ref: { run: book.bind(e, engine.instance(e.port)), requestId: String(e.seq) }, at: ev.at });
      break;
    }
  }
}

// ---------------------------------------------------------------------------
// 启动目标与配置文件
// ---------------------------------------------------------------------------

const docs = new Map<number, Doc>();

function docOf(inst: Instance): Doc {
  let doc = docs.get(inst.port);
  if (!doc) docs.set(inst.port, (doc = docFor(inst)));
  return doc;
}

function targetOf(inst: Instance): StartTarget {
  return { kind: inst.alias === 'default' ? 'default' : 'alias', id: `target:${inst.port}` };
}

function choices(): Choice[] {
  // 「没有实例」的场景下配置文件还在，只是没有实例在跑
  return engine.allInstances.map((i) => ({
    target: targetOf(i),
    label: i.alias ?? String(i.port),
    path: i.configPath,
    exists: true,
    isDefault: i.alias === 'default',
  }));
}

function instanceOfTarget(target: ConfigTarget): Instance {
  if (target.kind === 'run') {
    const inst = liveRun(target.run);
    if (target.expectedFile !== undefined && target.expectedFile !== fileToken(inst.port, docOf(inst))) throw fault('desktop_changed', {});
    return inst;
  }
  const inst = engine.instance(Number(target.id.split(':')[1]));
  if (!inst || targetOf(inst).id !== target.id) throw fault('desktop_target', {});
  return inst;
}

/** 配置里写的监听端口有没有被别的实例占着 */
function occupiedBy(self: Instance) {
  return (port: number) => engine.instances.some((i) => i.port !== self.port && i.status !== 'stopped' && listenPort(i.listen) === port);
}

/** 实例启动时读一遍配置文件：把文件里的值用到运行中的实例上 */
function loadConfig(inst: Instance) {
  const doc = docs.get(inst.port);
  if (!doc) return;
  if (doc.text.base_url !== undefined) inst.baseUrl = doc.text.base_url;
  if (doc.text.listen_addr !== undefined) inst.listen = doc.text.listen_addr;
  inst.proxy = doc.text.proxy || null;
  inst.keepalive.interval = doc.secs['keepalive.interval_secs'] ?? 15;
  inst.keepalive.trigger = doc.trigger === 'body-stream' ? 'body_stream' : (doc.trigger ?? 'any');
}

async function bringUp(inst: Instance): Promise<Run> {
  restorePending.delete(inst.port);
  loadConfig(inst);
  await engine.start(inst.port);
  return runOf(inst)!;
}

// ---------------------------------------------------------------------------
// 设置、界面偏好、置顶、开机自启
// ---------------------------------------------------------------------------

let watchdogEnabled = true;
let registration: Registration = { enabled: false, issue: null };
let preferences: Preferences = { lang: 'system', mode: 'system', palette: 'standard', mica: false, mascot: 'tumbler', theme: 'builtin' };
/** 演示 desktop.toml 被手改出语法错误：读到的偏好为 null，写入被拒绝（审核工具条上切换） */
let tomlBroken = false;
const TOML_ISSUE = { kind: 'syntax', line: 7, column: 12, key: null } as const;
/** 新置顶的在前 */
let bookmarks: Bookmark[] = rows(engine.logs.filter((e) => e.pinned))
  .reverse()
  .map(bookmarkOf);

function bookmarkOf(r: Row): Bookmark {
  return { ref: r.ref, startedAt: r.startedAt, method: r.method, path: r.path, model: r.model, outcome: r.outcome, attempts: r.attempts };
}

const MAX_BOOKMARKS = 200;

// ---------------------------------------------------------------------------
// 更新
// ---------------------------------------------------------------------------

const CURRENT = '0.2.0';
const RELEASES = {
  auto: { channel: 'stable', latest: '0.2.1', publishedAt: new Date(2026, 9, 9).getTime() },
  pre: { channel: 'pre', latest: '0.3.0-beta.1', publishedAt: new Date(2026, 9, 11).getTime() },
} as const;
const TOTAL_BYTES = 8_178_892;

/** 检查更新时发出去的句柄对应哪个版本 */
const checks = new Map<string, string>();
let checkSeq = 0;
let job: Job | null = null;
let installing: Promise<InstallEnd> | null = null;

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** 演示一次安装：按真实的阶段顺序走一遍，每一步把整份状态发出去。实例照真实的那样逐个重启 */
async function runInstall(version: string): Promise<InstallEnd> {
  const targets = engine.instances.filter((i) => i.status !== 'stopped').map((i) => i.port);
  job = { id: `job:${Date.now()}`, requestedVersion: version, targets: null, phase: null, phaseAt: null, download: null, instances: [], end: null, fault: null };
  const step = async (phase: InstallPhase, ms: number, change: Partial<Job> = {}) => {
    job = { ...job!, ...change, phase, phaseAt: Date.now() };
    notify({ type: 'update-progress', job: structuredClone(job) });
    await wait(ms);
  };

  await step('marking', 400, { targets });
  for (let i = 0; i <= 12; i++) {
    await step('downloading', 280, { download: { bytes: Math.round((TOTAL_BYTES * i) / 12), total: TOTAL_BYTES } });
  }
  await step('downloaded', 900);
  await step('broadcasting', 700);
  await step('acked', 500);
  await step('swapping', 700);
  await step('swapped', 300);
  for (const port of targets) {
    await step('restarting', 200);
    const inst = engine.instance(port);
    if (inst) {
      await engine.restart(port);
      inst.version = version;
    }
    job = { ...job!, instances: [...job!.instances, { port, outcome: 'restarted' }] };
  }
  await step('verifying', 600);
  await step('cleaning', 400);
  const end: InstallEnd = { outcome: 'installed', version };
  await step('done', 0, { end });
  notify({ type: 'instances-changed' });
  return end;
}

// ---------------------------------------------------------------------------
// 命令表
// ---------------------------------------------------------------------------

type Handlers = { [N in CommandName]: (args: Commands[N][0]) => Promise<Commands[N][1]> };

const handlers: Handlers = {
  // 浏览器里没有窗口外壳：没有自绘标题栏，也没有 Mica
  desktop_info: () => later({ customTitlebar: false, mica: 'unsupported', windowsBuild: null }),
  titlebar_insets: () => later({ top: 0, height: 0, right: 0 }),
  set_caption_color: () => later(null),

  // 事件通道由 attach() 自己管，不经命令
  attach: () => later(SESSION),
  detach: () => later(null),
  ack: () => later(null),

  snapshot: () => later(snapshot()),

  async query_logs({ run, cursor, limit }) {
    let list = engine.visibleLogs;
    if (run) list = list.filter((e) => e.port === run.port && book.bind(e, engine.instance(e.port)).instanceId === run.instanceId);
    // 游标记的是「上一页最早那条」的开始时刻与序号，往前翻就是取比它更早的
    if (cursor !== null) {
      const [time, seq] = cursor.split(':').slice(1).map(Number);
      if (Number.isNaN(time) || Number.isNaN(seq)) throw fault('desktop_cursor', {});
      list = list.filter((e) => e.time < time || (e.time === time && e.seq < seq));
    }
    const page = list.slice(-Math.max(1, limit));
    const oldest = page[0];
    return later({
      entries: rows(page).reverse(),
      next: oldest && list.length > page.length ? `cursor:${oldest.time}:${oldest.seq}` : null,
      missing: [],
    });
  },

  async request_detail({ ref }) {
    const e = findEntry(ref.run, ref.requestId);
    return later(detailOf(e, ref.run, engine.instance(e.port)), 60);
  },

  async body_chunk({ ref, part, offset, limit }) {
    const e = findEntry(ref.run, ref.requestId);
    const bytes = bodyBytes(e, part);
    if (!bytes) throw fault('desktop_history', {});
    const end = Math.min(bytes.length, offset + Math.max(1, limit));
    return later({ totalBytes: bytes.length, offset, dataB64: toBase64(bytes.subarray(offset, end)), more: end < bytes.length, complete: end >= bytes.length }, 40);
  },

  cancel_detail: () => later(null),
  rescan: () => later(null, 300),
  list_configs: () => later(choices()),

  async start_instance({ target }) {
    const inst = instanceOfTarget(target);
    // 「没有实例」场景里启动第一个：回到有实例的正常场景，原来在跑的实例也就都回来了
    if (engine.scenario === 'empty') {
      engine.setScenario('normal');
      const run = runOf(inst);
      if (run) return later(run, 600);
    } else if (inst.status !== 'stopped') {
      throw fault('desktop_port', { port: inst.port });
    }
    return bringUp(inst);
  },

  async stop_instance({ run }) {
    const inst = liveRun(run);
    // 不答复的实例没法好好停下：只能强制停止
    if (inst.status === 'unresponsive') throw fault('desktop_unresponsive', { port: inst.port });
    engine.stop(inst.port);
    notify({ type: 'instances-changed' });
    return later(null, 200);
  },

  async restart_instance({ run }) {
    const inst = liveRun(run);
    if (inst.status === 'unresponsive') throw fault('desktop_unresponsive', { port: inst.port });
    loadConfig(inst);
    await engine.restart(inst.port);
    return runOf(inst)!;
  },

  async force_stop_instance({ run }) {
    const inst = liveRun(run);
    // 只有不答复的实例可以强制停止：答复得了的要走正常的停止，让在途请求有机会收尾
    if (inst.status !== 'unresponsive') throw fault('desktop_force_refused', { port: inst.port });
    engine.stop(inst.port);
    notify({ type: 'instances-changed' });
    return later(null, 300);
  },

  async restore_instance({ port, expectedRestore }) {
    const inst = engine.instance(port);
    if (!inst || !restorePending.has(port) || expectedRestore !== restoreToken(port)) throw fault('desktop_changed', {});
    return bringUp(inst);
  },

  async forget_restore({ port, expectedRestore }) {
    if (!restorePending.has(port) || expectedRestore !== restoreToken(port)) throw fault('desktop_changed', {});
    restorePending.delete(port);
    notify({ type: 'instances-changed' });
    return later(null);
  },

  async read_config({ target }) {
    const inst = instanceOfTarget(target);
    return later(viewOf(inst.port, docOf(inst), inst));
  },

  async validate_config({ target, changes }) {
    const inst = instanceOfTarget(target);
    return later(validate(applyEdits(docOf(inst), changes), inst.configPath, occupiedBy(inst)), 220);
  },

  async write_config({ target, changes }) {
    const inst = instanceOfTarget(target);
    const before = docOf(inst);
    const next = applyEdits(before, changes);
    const check = validate(next, inst.configPath, occupiedBy(inst));
    if (!check.canSave) throw fault('desktop_config', { diagnostics: check.diagnostics });
    const { version: _a, ...old } = before;
    const { version: _b, ...now } = next;
    const changed = JSON.stringify(old) !== JSON.stringify(now);
    if (changed) docs.set(inst.port, { ...next, version: before.version + 1 });
    return later({ changed, instancesToRestart: changed && inst.status !== 'stopped' ? [inst.port] : [] }, 120);
  },

  read_settings: () =>
    later({
      watchdogEnabled,
      readOnly: [
        { key: 'update.channel', value: { kind: 'text', text: 'stable' } },
        { key: 'update.download_chain', value: { kind: 'list', items: [{ kind: 'text', text: 'github' }, { kind: 'text', text: 'npm' }] } },
      ],
      diagnostics: [],
    }),

  async set_watchdog({ enabled }) {
    watchdogEnabled = enabled;
    notify({ type: 'settings-changed' });
    return later(watchdogEnabled, 150);
  },

  read_desktop_state: () =>
    later({
      preferences: tomlBroken ? null : { ...preferences },
      enabledExtensions: tomlBroken ? null : enabledExtensions(),
      bookmarks: structuredClone(bookmarks),
      reset: [],
      desktopToml: tomlBroken ? [{ ...TOML_ISSUE }] : [],
    }),

  async set_preferences({ changes }) {
    if (tomlBroken) throw fault('desktop_toml', { ...TOML_ISSUE });
    preferences = { ...preferences, ...changes };
    return later({ ...preferences });
  },

  async set_pinned({ ref, pinned }) {
    const same = (b: Bookmark) => b.ref.requestId === ref.requestId && b.ref.run.port === ref.run.port && b.ref.run.instanceId === ref.run.instanceId;
    if (!pinned) bookmarks = bookmarks.filter((b) => !same(b));
    else if (!bookmarks.some(same)) {
      if (bookmarks.length >= MAX_BOOKMARKS) throw fault('desktop_limit', { resource: 'bookmark-count', max: MAX_BOOKMARKS });
      const e = findEntry(ref.run, ref.requestId);
      bookmarks = [bookmarkOf(rowOf(e, ref.run, engine.instance(e.port))), ...bookmarks];
    }
    return later(structuredClone(bookmarks));
  },

  read_autostart: () => later({ ...registration }),

  async set_autostart({ enabled }) {
    registration = { enabled, issue: null };
    return later({ ...registration }, 200);
  },

  async check_update({ channel }) {
    const r = RELEASES[channel];
    const current = engine.instances[0]?.version === r.latest ? r.latest : CURRENT;
    const updateAvailable = current !== r.latest;
    let check: string | null = null;
    if (updateAvailable) {
      check = `check:${++checkSeq}`;
      checks.set(check, r.latest);
    }
    const info: UpdateInfo = { channel: r.channel, current, latest: r.latest, updateAvailable, variant: 'desktop', publishedAt: r.publishedAt, checkedAt: Date.now(), check };
    return later(info, 400);
  },

  install_update({ check }) {
    const version = checks.get(check);
    if (!version) return Promise.reject(fault('desktop_changed', {}));
    if (installing) return Promise.reject(fault('install_in_progress', {}));
    installing = runInstall(version).finally(() => (installing = null));
    return installing;
  },

  about: () => later<About>({ version: CURRENT, variant: 'desktop', home: '~/.aproxy' }),

  ...packageHandlers,
};

class DemoLink implements Link {
  readonly demo = true;
  private attached = false;

  call<N extends CommandName>(name: N, args: Commands[N][0]): Promise<Commands[N][1]> {
    // 命令里同步抛出的 Fault 也以拒绝的 Promise 交出去，和跨进程的调用一样
    const run = handlers[name] as (args: Commands[N][0]) => Promise<Commands[N][1]>;
    return Promise.resolve().then(() => run(args));
  }

  attach(next: EventHandler): void {
    handler = next;
    if (!this.attached) {
      this.attached = true;
      engine.subscribe(onEngine);
    }
    queue.length = 0;
    notify({ type: 'resync' });
  }

  session(): string | null {
    return this.attached ? SESSION : null;
  }

  /** 演示每次都做一个新的 blob: 地址，用不着激活代号 */
  packageUrl(id: string, path: string): string | null {
    return demoPackageUrl(id, path);
  }
}

export const demoLink: Link = new DemoLink();

/**
 * 只给开发时的审核工具用：切换演示场景、立刻触发一次庆祝。不属于契约，桌面窗口里没有对应的东西
 */
export const demoControls = {
  setScenario: (s: Parameters<Engine['setScenario']>[0]) => engine.setScenario(s),
  celebrate: () => engine.celebrate(),
  /** 切换「desktop.toml 有语法错误」，并像窗口重新获得焦点那样让页面重读 */
  breakDesktopToml(on: boolean) {
    tomlBroken = on;
    notify({ type: 'desktop-state-changed' });
  },
};

/*
 * 原型唯一的 Backend 实现：在浏览器里用模拟引擎冒充实例。
 *
 * 尽量像隔着 IPC：每个方法都异步、带一点点延迟，交出去的都是拷贝。界面要是在 mock 上能跑通，
 * 换成 Tauri 实现时就不会因为「原来是同步的」「原来共享了同一个对象」这类偷懒而出问题。
 * 配置、设置、更新、扩展这几块状态只存在内存里，刷新页面就回到初始值。
 */
import type { Backend } from '../types';
import type {
  AppSettings,
  ConfigIssue,
  ConfigKey,
  ConfigValues,
  ConsentRequest,
  ExtensionAction,
  ExtensionInfo,
  Instance,
  InstanceConfig,
  UpdateInfo,
  UpdateProgress,
  UpdateStage,
  WriteCapability,
} from '../../types';
import { Engine } from './engine';
import { buildDetail } from './fake';

const engine = new Engine();

/** 模拟跨进程的往返时间：短到察觉不到，但足以让「以为是同步的」写法露馅 */
function later<T>(value: T, ms = 25): Promise<T> {
  return new Promise((resolve) => setTimeout(() => resolve(value), ms));
}

// ---------------------------------------------------------------------------
// 配置：首次读取时按实例的现状生成一份，之后读写这份
// ---------------------------------------------------------------------------

/** 改了立即生效的键；其余要重启实例。这是原型阶段的提案，接入时以实例的实现为准 */
const LIVE: ConfigKey[] = [
  'connect_timeout_secs',
  'read_timeout_secs',
  'max_retry_backoff_secs',
  'bounded_retry_paths',
  'keepalive_interval_secs',
  'keepalive_trigger',
  'keepalive_heartbeat',
  'allowed_hosts',
  'allowed_origins',
  'max_body_mb',
];

const configs = new Map<number, { values: ConfigValues; apiKeyMasked: string | null }>();
/** 需要重启才生效的改动：写进配置文件，但运行中的实例要等重启才用上 */
const pending = new Map<number, Partial<ConfigValues>>();

function configOf(inst: Instance) {
  let c = configs.get(inst.port);
  if (!c) {
    c = {
      values: {
        base_url: inst.baseUrl,
        listen_addr: inst.listen,
        proxy: inst.proxy ?? '',
        connect_timeout_secs: 30,
        read_timeout_secs: 300,
        max_retry_backoff_secs: inst.backoffCap,
        bounded_retry_paths: [...inst.boundedPaths],
        forward_only: inst.forwardOnly,
        spool_limit_mb: 256,
        max_body_mb: 128,
        disk_cache: inst.diskCache,
        keepalive_interval_secs: inst.keepalive.interval || 15,
        keepalive_trigger: inst.keepalive.trigger,
        keepalive_heartbeat: ': keepalive\\n\\n',
        request_transform: inst.transform
          ? { command: '~/.aproxy/bin/aproxy-format', args: 'run', mode: inst.transform.mode, extra: '~/.aproxy/openrouter-agg.toml', every_attempt: false }
          : null,
        allowed_hosts: [],
        allowed_origins: [],
      },
      apiKeyMasked: inst.port === 12346 ? 'sk-or-***' : null,
    };
    configs.set(inst.port, c);
  }
  return c;
}

/** 把配置里的值反映到实例的运行状态上（界面的实例卡片、详情页读的是实例状态） */
function applyToInstance(inst: Instance, v: Partial<ConfigValues>) {
  if (v.max_retry_backoff_secs !== undefined) inst.backoffCap = v.max_retry_backoff_secs;
  if (v.bounded_retry_paths) inst.boundedPaths = [...v.bounded_retry_paths];
  if (v.keepalive_interval_secs !== undefined) inst.keepalive.interval = v.keepalive_interval_secs;
  if (v.keepalive_trigger) inst.keepalive.trigger = v.keepalive_trigger;
  if (v.base_url !== undefined) inst.baseUrl = v.base_url;
  if (v.listen_addr !== undefined) inst.listen = v.listen_addr;
  if (v.proxy !== undefined) inst.proxy = v.proxy || null;
  if (v.forward_only !== undefined) inst.forwardOnly = v.forward_only;
  if (v.disk_cache !== undefined) inst.diskCache = v.disk_cache;
}

// ---------------------------------------------------------------------------
// 设置与更新
// ---------------------------------------------------------------------------

let settings: AppSettings = {
  watchdog: true,
  autostart: false,
  ui: { lang: 'system', mode: 'system', palette: 'standard', mica: false, mascot: 'tumbler', theme: null },
};

const CURRENT = '0.2.0';
const boot = Date.now();
let installed = CURRENT;
let installing: Promise<void> | null = null;

const RELEASES: Record<UpdateInfo['channel'], Omit<UpdateInfo, 'current' | 'variant' | 'checkedAt'>> = {
  stable: {
    channel: 'stable',
    latest: '0.2.1',
    publishedAt: new Date(2026, 9, 9).getTime(),
    notes: [
      { kind: 'feature', text: { en: 'Compare responses across attempts in request details', 'zh-CN': '日志详情可以在多次尝试之间对比响应' } },
      { kind: 'feature', text: { en: 'The watchdog now spots a hung instance within 30 s', 'zh-CN': '看护者识别卡死实例的速度提高到 30 秒' } },
      { kind: 'fix', text: { en: 'Fixed an extra heartbeat when the backoff cap is 0', 'zh-CN': '修复：保活心跳在退避封顶为 0 时多发一拍' } },
      { kind: 'fix', text: { en: 'Fixed a console window flashing when launched from the Start menu', 'zh-CN': '修复：Windows 上从开始菜单启动不再闪控制台窗口' } },
    ],
  },
  pre: {
    channel: 'pre',
    latest: '0.3.0-beta.1',
    publishedAt: new Date(2026, 9, 11).getTime(),
    notes: [
      { kind: 'feature', text: { en: 'Retry rules per status code and path', 'zh-CN': '按状态码与路径分别设置重试规则' } },
      { kind: 'feature', text: { en: 'Themes and mascots can be shared as a single string', 'zh-CN': '主题与吉祥物可以用一段字符串分享' } },
      { kind: 'fix', text: { en: 'Fixed the log view jumping while following new entries', 'zh-CN': '修复：实时跟随新日志时列表会跳动' } },
    ],
  },
};

/** 各阶段的演示时长（毫秒）；逐个重启按实例数线性增加 */
const STAGE_MS: Record<Exclude<UpdateStage, 'done'>, number> = {
  download: 3600,
  verify: 1100,
  notify: 1200,
  swap: 900,
  restart: 1500,
};
const STAGES: UpdateStage[] = ['download', 'verify', 'notify', 'swap', 'restart', 'done'];
const TOTAL_MB = 7.8;

function runInstall(version: string): Promise<void> {
  // 原型的「再演示一次」：每次安装都从当前正式版本重新走一遍
  installed = CURRENT;
  const targets = engine.instances.filter((i) => i.status !== 'stopped').map((i) => i.port);
  return new Promise((resolve) => {
    let idx = 0;
    let started = performance.now();
    const report = (stage: UpdateStage, progress: number) => {
      const restarted = stage === 'restart' ? targets.slice(0, Math.floor(progress * targets.length + 0.0001)) : stage === 'done' ? targets : [];
      const p: UpdateProgress = {
        version,
        stage,
        progress,
        restarted,
        targets,
        downloadedMb: stage === 'download' ? +(TOTAL_MB * progress).toFixed(1) : TOTAL_MB,
        totalMb: TOTAL_MB,
      };
      engine.emit({ type: 'update-progress', progress: p });
    };
    const timer = setInterval(() => {
      const stage = STAGES[idx] as Exclude<UpdateStage, 'done'>;
      const total = stage === 'restart' ? STAGE_MS.restart * Math.max(1, targets.length) : STAGE_MS[stage];
      const progress = Math.min(1, (performance.now() - started) / total);
      report(stage, progress);
      if (progress < 1) return;
      idx += 1;
      started = performance.now();
      if (STAGES[idx] === 'done') {
        clearInterval(timer);
        installed = version;
        report('done', 1);
        resolve();
      }
    }, 80);
  });
}

// ---------------------------------------------------------------------------
// 扩展：几个装好的示例，覆盖只读、已允许、尚未允许、已停用与自定义吉祥物
// ---------------------------------------------------------------------------

const DAY = 86_400_000;
const today4am = new Date(boot).setHours(4, 0, 0, 0);

const extensions: ExtensionInfo[] = [
  {
    id: 'nightly-restart',
    name: { en: 'Nightly restart', 'zh-CN': '定时重启' },
    description: {
      en: 'Restarts instances one by one at 4 a.m. to clear out connections that pile up over long runs.',
      'zh-CN': '每天凌晨 4 点逐个重启实例，清掉长时间运行积累下来的连接。',
    },
    version: '1.2.0',
    author: 'lin',
    origin: '~/.aproxy/extensions/nightly-restart',
    kind: 'extension',
    enabled: true,
    declares: ['control'],
    granted: [{ capability: 'control', at: boot - 9 * DAY }],
    recent: [
      { action: { kind: 'restart', port: 12346 }, at: today4am + 95_000 },
      { action: { kind: 'restart', port: 12345 }, at: today4am },
    ],
  },
  {
    id: 'retry-notifier',
    name: { en: 'Retry alerts', 'zh-CN': '重试提醒' },
    description: {
      en: 'Sends a system notification when a request has retried more than 10 times.',
      'zh-CN': '一个请求重试超过 10 次时发一条系统通知。',
    },
    version: '0.3.1',
    author: 'aProxy',
    origin: '~/.aproxy/extensions/retry-notifier',
    kind: 'extension',
    enabled: true,
    declares: [],
    granted: [],
    recent: [],
  },
  {
    id: 'config-sync',
    name: { en: 'Config sync', 'zh-CN': '配置同步' },
    description: {
      en: 'Keeps instance configs in a Git repo, writes back changes pulled from it and restarts instances when needed.',
      'zh-CN': '把各实例的配置放进一个 Git 仓库，拉到改动后写回配置，需要时重启实例。',
    },
    version: '2.0.0',
    author: 'mika',
    origin: '~/.aproxy/extensions/config-sync',
    kind: 'extension',
    enabled: true,
    declares: ['config', 'control'],
    granted: [],
    recent: [],
  },
  {
    id: 'auto-update',
    name: { en: 'Auto update', 'zh-CN': '自动更新' },
    description: {
      en: 'Installs new stable releases overnight.',
      'zh-CN': '有新的正式版时，在夜里自动安装。',
    },
    version: '0.4.2',
    author: 'aProxy',
    origin: '~/.aproxy/extensions/auto-update',
    kind: 'extension',
    enabled: false,
    declares: ['install'],
    granted: [],
    recent: [],
  },
  {
    id: 'yun.nimbus',
    name: { en: 'Nimbus', 'zh-CN': '小云' },
    description: {
      en: 'A little cloud that rains while requests retry and lets the sun out when they are rescued.',
      'zh-CN': '一朵小云：请求重试时下雨，救回时出太阳。',
    },
    version: '1.0.0',
    author: 'yun',
    origin: '~/.aproxy/mascots/nimbus',
    kind: 'mascot',
    enabled: true,
    declares: [],
    granted: [],
    recent: [],
  },
];

function emitExtensions() {
  engine.emit({ type: 'extensions', extensions: structuredClone(extensions) });
}

function extensionById(id: string): ExtensionInfo {
  const ext = extensions.find((e) => e.id === id);
  if (!ext) throw new Error(`no such extension: ${id}`);
  return ext;
}

/** 等用户回答的写操作：扩展的那次调用挂在这里 */
const consents = new Map<string, ConsentRequest>();
let consentSeq = 0;

async function perform(ext: ExtensionInfo, action: ExtensionAction) {
  if (action.kind === 'restart') await mockBackend.restartInstance(action.port);
  else if (action.kind === 'write-config') await mockBackend.writeConfig(action.port, { max_retry_backoff_secs: 60 });
  ext.recent.unshift({ action, at: Date.now() });
  emitExtensions();
}

/**
 * 原型审核用：模拟「配置同步」发起一次写操作——先改配置，再重启实例。
 * 每次挑第一类还没允许过的操作发出询问；两类都允许过了就什么也不做，返回 all-granted。
 */
function extensionWrite(): 'asked' | 'all-granted' {
  const ext = extensionById('config-sync');
  const plan: { capability: WriteCapability; action: ExtensionAction }[] = [
    { capability: 'config', action: { kind: 'write-config', port: 12346, keys: ['max_retry_backoff_secs'] } },
    { capability: 'control', action: { kind: 'restart', port: 12346 } },
  ];
  const next = plan.find((p) => !ext.granted.some((g) => g.capability === p.capability));
  if (!next) return 'all-granted';
  const request: ConsentRequest = { id: `consent-${++consentSeq}`, extension: ext.id, capability: next.capability, action: next.action, at: Date.now() };
  consents.set(request.id, request);
  engine.emit({ type: 'consent-request', request });
  return 'asked';
}

// ---------------------------------------------------------------------------

export const mockBackend: Backend = {
  snapshot: () => later(engine.snapshot()),
  subscribe: (listener) => engine.subscribe(listener),

  async queryLogs({ before, limit, port }) {
    let list = engine.visibleLogs.filter((e) => port === undefined || e.port === port);
    if (before) {
      const i = list.findIndex((e) => e.key === before);
      list = i >= 0 ? list.slice(0, i) : list;
    }
    const page = list.slice(-limit);
    return later({ entries: structuredClone(page), more: list.length > page.length });
  },

  async requestDetail(key) {
    const e = engine.entry(key);
    if (!e) throw new Error(`no such request: ${key}`);
    return later(buildDetail(e));
  },

  async setPinned(key, pinned) {
    engine.setPinned(key, pinned);
    return later(undefined);
  },

  async startInstance(port) {
    // 空状态里「启动第一个实例」：按默认配置启动，在模拟里就是回到有实例的正常场景
    if (port === undefined) {
      engine.setScenario('normal');
      return later(undefined, 600);
    }
    await engine.start(port);
  },

  async stopInstance(port) {
    engine.stop(port);
    return later(undefined);
  },

  async restartInstance(port) {
    await engine.restart(port);
    const inst = engine.instance(port);
    const p = pending.get(port);
    if (inst && p) {
      applyToInstance(inst, p);
      pending.delete(port);
    }
  },

  async forceStopInstance(port) {
    engine.stop(port);
    return later(undefined, 300);
  },

  async reconnectInstance() {
    // 模拟里卡住的实例不会自己好：重连只是再问一次，仍然没有应答
    return later(undefined, 900);
  },

  async readConfig(port) {
    const inst = engine.instance(port);
    if (!inst) throw new Error(`no such instance: ${port}`);
    const c = configOf(inst);
    const out: InstanceConfig = { port, path: inst.configPath, values: structuredClone(c.values), apiKeyMasked: c.apiKeyMasked, live: [...LIVE] };
    return later(out);
  },

  async validateConfig(port, changes) {
    const issues: ConfigIssue[] = [];
    const portOf = (addr: string) => Number(addr.split(':').pop());
    if (changes.listen_addr !== undefined) {
      const p = portOf(changes.listen_addr);
      const other = engine.instances.find((i) => i.port !== port && portOf(i.listen) === p);
      if (other) {
        const name = other.alias ?? String(other.port);
        issues.push({
          key: 'listen_addr',
          message: { en: `Port ${p} is already used by instance ${name}.`, 'zh-CN': `端口 ${p} 已经被实例 ${name} 占用。` },
        });
      }
    }
    if (changes.base_url !== undefined) {
      // 上游指回本机某个 aProxy 实例（包括它自己）：请求会在实例之间绕圈，重试还会把圈越绕越多
      try {
        const u = new URL(changes.base_url);
        const p = Number(u.port || (u.protocol === 'https:' ? 443 : 80));
        const loop = ['127.0.0.1', 'localhost', '[::1]'].includes(u.hostname) ? engine.instances.find((i) => portOf(i.listen) === p) : undefined;
        if (loop) {
          const name = loop.alias ?? String(loop.port);
          issues.push({
            key: 'base_url',
            message: {
              en: `The upstream points back at aProxy instance ${name}; requests would loop.`,
              'zh-CN': `上游指回了 aProxy 自己的实例 ${name}，请求会在实例之间绕圈。`,
            },
          });
        }
      } catch {
        // 格式错误界面那一侧已经拦下了，这里只管格式之外的问题
      }
    }
    return later(issues, 220);
  },

  async writeConfig(port, changes) {
    const inst = engine.instance(port);
    if (!inst) throw new Error(`no such instance: ${port}`);
    const c = configOf(inst);
    Object.assign(c.values, structuredClone(changes));
    const keys = Object.keys(changes) as ConfigKey[];
    const restartRequired = keys.filter((k) => !LIVE.includes(k));
    const live = Object.fromEntries(keys.filter((k) => LIVE.includes(k)).map((k) => [k, changes[k]])) as Partial<ConfigValues>;
    applyToInstance(inst, live);
    if (restartRequired.length) {
      pending.set(port, { ...pending.get(port), ...Object.fromEntries(restartRequired.map((k) => [k, changes[k]])) });
    }
    return later({ restartRequired });
  },

  readSettings: () => later(structuredClone(settings)),

  async writeSettings(changes) {
    settings = { ...settings, ...structuredClone(changes), ui: { ...settings.ui, ...changes.ui } };
    return later(undefined);
  },

  async checkUpdate(channel) {
    const r = RELEASES[channel];
    return later({ ...structuredClone(r), current: installed, variant: 'desktop', checkedAt: boot - 3 * 60_000 }, 120);
  },

  installUpdate(version) {
    installing ??= runInstall(version).finally(() => (installing = null));
    return installing;
  },

  listExtensions: () => later(structuredClone(extensions)),

  async setExtensionEnabled(id, enabled) {
    extensionById(id).enabled = enabled;
    emitExtensions();
    return later(undefined);
  },

  async revokeGrant(id, capability) {
    const ext = extensionById(id);
    ext.granted = ext.granted.filter((g) => g.capability !== capability);
    emitExtensions();
    return later(undefined);
  },

  async readMascot(id) {
    const ext = extensions.find((e) => e.id === id && e.kind === 'mascot');
    if (!ext) throw new Error(`no such mascot: ${id}`);
    // 模拟「从扩展文件夹读出 mascot.js」：原型把示例放在 public/mascots/ 下，取的是原样的源码文本
    const folder = ext.origin.split('/').pop();
    const res = await fetch(`${import.meta.env.BASE_URL}mascots/${folder}/mascot.js`);
    if (!res.ok) throw new Error(`mascot ${id}: ${res.status}`);
    return later(await res.text());
  },

  async answerConsent(requestId, allow) {
    const req = consents.get(requestId);
    if (!req) return later(undefined);
    consents.delete(requestId);
    if (allow) {
      const ext = extensionById(req.extension);
      ext.granted.push({ capability: req.capability, at: Date.now() });
      emitExtensions();
      // 挂起的那次调用继续执行；它的结果（实例重启、配置变化）照常经事件推给界面
      void perform(ext, req.action);
    }
    return later(undefined);
  },
};

/** 只给原型的审核工具条用：切换场景、立刻触发一次庆祝、模拟扩展的写操作。不属于 Backend 接口，正式实现里没有 */
export const mockControls = {
  setScenario: (s: Parameters<Engine['setScenario']>[0]) => engine.setScenario(s),
  celebrate: () => engine.celebrate(),
  extensionWrite,
};

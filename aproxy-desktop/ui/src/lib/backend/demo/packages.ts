/*
 * 演示数据里的主题包与扩展包：浏览器里开发、截图时用，背后没有文件，也不联网。
 *
 * 和真实实现一样的地方：check 立刻返回、verify 慢一拍；token 单次有效；升级保留启用状态、设置与数据；
 * 卸载可以连数据一起删；启用的扩展按 enabled 的顺序加载。
 * 不一样的地方：不解析 zip。选中的文件按文件名里的关键词挑一个演示场景（见 SCENARIOS），
 * 审核工具条上的「模拟安装」也是这样走的，方便把每一种安装提醒都看一遍。
 */
import type {
  DataFile,
  Fault,
  GithubFacts,
  PackageCheck,
  PackageEntry,
  PackageManifest,
  PackageSource,
  Token,
} from '../contract';

/** 模拟跨进程的往返；verify 另外再等一会儿，像真的去了一趟 GitHub */
function later<T>(value: T, ms = 25): Promise<T> {
  return new Promise((resolve) => setTimeout(() => resolve(value), ms));
}

function fault<C extends Fault['code']>(code: C, details: Extract<Fault, { code: C }>['details']): Fault {
  return { code, details } as Fault;
}

const DAY = 86_400_000;
const now = Date.now();

// ---------------------------------------------------------------------------
// 演示扩展的代码：真实的扩展是包里的 main.mjs，这里是同样写法的一段源码，经 blob: 地址加载
// ---------------------------------------------------------------------------

/** 「请求节拍」：在导航里加一页，把最近的请求按结果画成一排小柱子；总览顶上放一条今日小结 */
const PULSE_SOURCE = String.raw`
const COLORS = { ok: 'var(--mint)', 'in-flight': 'var(--sky)', failed: 'var(--coral)', cancelled: 'var(--line)' };

export function activate(api) {
  const t = (zh, en) => (api.ui.state().lang === 'zh-CN' ? zh : en);

  api.ui.addPage({
    id: 'pulse',
    title: { 'zh-CN': '请求节拍', en: 'Pulse' },
    icon: 'bolt',
    render(el) {
      const h = document.createElement('h1');
      h.style.cssText = 'margin:0 0 6px;font-size:var(--fs-2xl);font-weight:620';
      const p = document.createElement('p');
      p.style.cssText = 'margin:0 0 18px;color:var(--text-2)';
      const bars = document.createElement('div');
      bars.style.cssText = 'display:flex;align-items:flex-end;gap:4px;height:160px;padding:16px;border-radius:var(--r-lg);background:var(--card-2-bg)';
      el.append(h, p, bars);
      const draw = () => {
        const rows = api.requests.recent().slice().reverse();
        h.textContent = t('请求节拍', 'Pulse');
        p.textContent = t('最近 ' + rows.length + ' 条请求，越高的柱子重试得越多', 'The last ' + rows.length + ' requests; taller bars took more attempts');
        bars.replaceChildren(...rows.map((r) => {
          const b = document.createElement('span');
          b.title = r.method + ' ' + r.path;
          b.style.cssText = 'flex:1;min-width:3px;border-radius:4px 4px 2px 2px;background:' + (COLORS[r.outcome] || 'var(--line)') + ';height:' + Math.min(100, 18 + r.attempts * 16) + '%';
          return b;
        }));
      };
      draw();
      return api.requests.onChange(draw);
    },
  });

  api.ui.mount('overview.top', (el) => {
    el.style.cssText = 'padding:10px 14px;border-radius:var(--r-md);background:var(--sky-soft);font-size:var(--fs-sm)';
    const draw = () => {
      const rows = api.requests.recent();
      const retried = rows.filter((r) => r.attempts > 1).length;
      el.textContent = t('请求节拍：最近 ' + rows.length + ' 条请求里有 ' + retried + ' 条重试过', 'Pulse: ' + retried + ' of the last ' + rows.length + ' requests were retried');
    };
    draw();
    return api.requests.onChange(draw);
  });
}
`;

/** 新装的演示扩展：只打一声招呼，说明它确实加载了 */
function helloSource(name: string): string {
  const n = JSON.stringify(name);
  return `export function activate(api) { api.ui.toast(api.ui.state().lang === 'zh-CN' ? '已加载 ' + ${n} : ${n} + ' is running'); }`;
}

/**
 * 「小云」：只加一个吉祥物。定义就是 public/mascots/nimbus/mascot.js 默认导出的那个对象；
 * 演示的扩展源码经 blob: 地址加载，相对地址解析不了，所以按页面的绝对地址 import 它
 */
function nimbusSource(): string {
  const url = new URL(`${import.meta.env.BASE_URL}mascots/nimbus/mascot.js`, location.href).href;
  return `import nimbus from ${JSON.stringify(url)};
export function activate(api) { api.ui.registerMascot(nimbus); }
`;
}

// ---------------------------------------------------------------------------
// 已装的包
// ---------------------------------------------------------------------------

interface Installed {
  entry: PackageEntry;
  /** 包里的文件：路径 → 内容（文本）。演示只放得下文本 */
  files: Record<string, string>;
}

function manifest(m: Partial<PackageManifest> & Pick<PackageManifest, 'kind' | 'id' | 'name' | 'version' | 'author'>): PackageManifest {
  return { schema: 1, description: '', repository: null, license: null, formats: [], network: [], writes: [], ...m };
}

function checked(repo: string, stars: number, release: Extract<GithubFacts, { state: 'checked' }>['release'], extra: Partial<Extract<GithubFacts, { state: 'checked' }>['repo']> = {}): GithubFacts {
  return {
    state: 'checked',
    repo: {
      fullName: repo,
      url: `https://github.com/${repo}`,
      stars,
      forks: Math.round(stars / 9),
      createdAt: now - 400 * DAY,
      pushedAt: now - 6 * DAY,
      archived: false,
      isFork: false,
      license: 'MIT',
      renamed: false,
      ...extra,
    },
    release,
  };
}

const SAKURA_THEME = {
  name: '樱夜 Sakura Night',
  author: 'mika',
  radius: 1.2,
  light: { primary: '#f2a7c3', text: '#3a2632', surface: '#fbf3f6', ok: '#7fd6b4', retry: '#9cc3f0', error: '#f08a8a', warn: '#f2cf6b' },
  dark: { primary: '#e889b0', text: '#f6e9ef', surface: '#211820', ok: '#6ec9a6', retry: '#86b0e0', error: '#e27b7b', warn: '#e6c25c' },
};

const installed = new Map<string, Installed>();

function install(i: Installed) {
  installed.set(i.entry.id, i);
}

install({
  entry: {
    id: 'io.github.mika.sakura-night',
    kind: 'theme',
    manifest: manifest({
      kind: 'theme',
      id: 'io.github.mika.sakura-night',
      name: { 'zh-CN': '樱夜', en: 'Sakura Night' },
      version: '1.3.0',
      author: 'mika',
      description: { 'zh-CN': '樱花粉配夜色，圆角更大一点', en: 'Cherry-blossom pink on a night sky, with rounder corners' },
      repository: 'https://github.com/mika/aproxy-sakura-night',
      license: 'MIT',
    }),
    enabled: false,
    installedAt: now - 12 * DAY,
    source: { kind: 'file', name: 'sakura-night-1.3.0.zip', sha256: '5d41402abc4b2a76b9719d911017c592ae0f3c1d8e7b7c4e2f4f0f1b6a1e9c3d', bytes: 18_420 },
    github: checked('mika/aproxy-sakura-night', 318, { state: 'match', tag: 'v1.3.0', asset: 'sakura-night-1.3.0.zip', url: 'https://github.com/mika/aproxy-sakura-night/releases/tag/v1.3.0' }),
    issue: null,
    dataBytes: 0,
    theme: SAKURA_THEME,
  },
  files: { 'theme.json': JSON.stringify(SAKURA_THEME) },
});

install({
  entry: {
    id: 'io.github.lin.request-pulse',
    kind: 'extension',
    manifest: manifest({
      kind: 'extension',
      id: 'io.github.lin.request-pulse',
      name: { 'zh-CN': '请求节拍', en: 'Request pulse' },
      version: '0.4.1',
      author: 'lin',
      description: { 'zh-CN': '把最近的请求画成一排小柱子，一眼看出哪段时间重试得多', en: 'Draws recent requests as a row of bars, so bursts of retries stand out' },
      repository: 'https://github.com/lin/aproxy-request-pulse',
      license: 'Apache-2.0',
    }),
    enabled: true,
    installedAt: now - 3 * DAY,
    source: { kind: 'file', name: 'request-pulse-0.4.1.zip', sha256: '9b74c9897bac770ffc029102a200c5de3a1c8f0e4d2b6a7c5e3f1d9b8a7c6e5f', bytes: 6_912 },
    github: checked('lin/aproxy-request-pulse', 1_204, { state: 'match', tag: 'v0.4.1', asset: 'request-pulse-0.4.1.zip', url: 'https://github.com/lin/aproxy-request-pulse/releases/tag/v0.4.1' }),
    issue: null,
    dataBytes: 2_310,
    theme: null,
  },
  files: { 'main.mjs': PULSE_SOURCE },
});

install({
  entry: {
    id: 'io.github.yun.nimbus',
    kind: 'extension',
    manifest: manifest({
      kind: 'extension',
      id: 'io.github.yun.nimbus',
      name: { 'zh-CN': '小云', en: 'Nimbus' },
      version: '1.1.0',
      author: 'yun',
      description: { 'zh-CN': '一朵云当吉祥物：重试时下雨，救回时出太阳', en: 'A cloud for a mascot: it rains while retrying and the sun comes out on a rescue' },
      repository: 'https://github.com/yun/aproxy-nimbus',
      license: 'MIT',
    }),
    enabled: true,
    installedAt: now - 20 * DAY,
    source: { kind: 'file', name: 'nimbus-1.1.0.zip', sha256: '2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae', bytes: 5_120 },
    github: checked('yun/aproxy-nimbus', 96, { state: 'match', tag: 'v1.1.0', asset: 'nimbus-1.1.0.zip', url: 'https://github.com/yun/aproxy-nimbus/releases/tag/v1.1.0' }),
    issue: null,
    dataBytes: 0,
    theme: null,
  },
  files: { 'main.mjs': nimbusSource() },
});

// 手工拷进 extensions/ 的包：没有安装记录，没经过安装前的检查
install({
  entry: {
    id: 'local.quota-badge',
    kind: 'extension',
    manifest: manifest({
      kind: 'extension',
      id: 'local.quota-badge',
      name: 'Quota badge',
      version: '0.0.3',
      author: 'unknown',
      description: 'Shows the remaining quota of an upstream in the title bar',
      network: ['quota.example.invalid'],
    }),
    enabled: false,
    installedAt: null,
    source: null,
    github: null,
    issue: null,
    dataBytes: 0,
    theme: null,
  },
  files: { 'main.mjs': helloSource('Quota badge') },
});

/** 启用的扩展，按加载的先后：desktop.toml 里 [extensions] enabled 的样子 */
let enabled: string[] = ['io.github.lin.request-pulse', 'io.github.yun.nimbus'];

function entryOf(i: Installed): PackageEntry {
  return structuredClone({ ...i.entry, enabled: i.entry.kind === 'extension' && enabled.includes(i.entry.id) });
}

function list(): PackageEntry[] {
  return [...installed.values()].map(entryOf).sort((a, b) => a.id.localeCompare(b.id));
}

function extension(id: string): Installed {
  const i = installed.get(id);
  if (!i || i.entry.kind !== 'extension') throw fault('package_unknown', { id });
  return i;
}

// ---------------------------------------------------------------------------
// 安装：按文件名挑场景
// ---------------------------------------------------------------------------

interface Scenario {
  manifest: PackageManifest;
  files: Record<string, string>;
  source: (name: string) => PackageSource;
  facts: GithubFacts;
  theme?: Record<string, unknown>;
}

const fileSource = (bytes: number) => (name: string): PackageSource => ({
  kind: 'file',
  name,
  sha256: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
  bytes,
});

const MOSS_THEME = {
  name: '苔 Moss',
  author: 'rui',
  radius: 0.8,
  font: 'system',
  light: { primary: '#8fbf7f', text: '#28331f', surface: '#f2f4ec', ok: '#4fc59b', retry: '#86b2e8', error: '#ee8f86', warn: '#e8c766' },
  dark: { primary: '#86b878', text: '#e9eee2', surface: '#171c15', ok: '#4ab48e', retry: '#7aa4d6', error: '#d9817a', warn: '#d6b75c' },
};

/** 关键词 → 场景。文件名里含哪个词就走哪个；都不含就是一个干净的扩展 */
const SCENARIOS: Record<string, Scenario> = {
  clean: {
    manifest: manifest({
      kind: 'extension',
      id: 'io.github.nori.token-meter',
      name: { 'zh-CN': '用量表', en: 'Token meter' },
      version: '1.0.2',
      author: 'nori',
      description: { 'zh-CN': '按天统计每个实例用掉的 token', en: 'Counts the tokens each instance used per day' },
      repository: 'https://github.com/nori/aproxy-token-meter',
      license: 'MIT',
      network: [],
      writes: [],
    }),
    files: { 'main.mjs': helloSource('Token meter'), 'README.md': '' },
    source: fileSource(9_318),
    facts: checked('nori/aproxy-token-meter', 2_480, { state: 'match', tag: 'v1.0.2', asset: 'token-meter-1.0.2.zip', url: 'https://github.com/nori/aproxy-token-meter/releases/tag/v1.0.2' }),
  },
  theme: {
    manifest: manifest({
      kind: 'theme',
      id: 'io.github.rui.moss',
      name: { 'zh-CN': '苔', en: 'Moss' },
      version: '2.0.0',
      author: 'rui',
      description: { 'zh-CN': '安静的苔绿，系统字体', en: 'A quiet moss green with the system font' },
      repository: 'https://github.com/rui/aproxy-theme-moss',
      license: 'CC-BY-4.0',
    }),
    files: { 'theme.json': JSON.stringify(MOSS_THEME), 'theme.css': ':root { --demo-moss: 1; }', 'assets/leaf.svg': '<svg/>' },
    source: fileSource(4_102),
    facts: checked('rui/aproxy-theme-moss', 86, { state: 'match', tag: 'v2.0.0', asset: 'moss-2.0.0.zip', url: 'https://github.com/rui/aproxy-theme-moss/releases/tag/v2.0.0' }),
    theme: MOSS_THEME,
  },
  closed: {
    manifest: manifest({
      kind: 'extension',
      id: 'com.fastkey.booster',
      name: 'FastKey Booster',
      version: '3.1.0',
      author: 'FastKey Team',
      description: 'Rotates your keys automatically for maximum speed',
      network: ['api.fastkey.example.invalid'],
      writes: ['config', 'instances'],
    }),
    files: { 'main.mjs': helloSource('FastKey Booster') },
    source: fileSource(88_201),
    facts: { state: 'no-repository' },
  },
  mismatch: {
    manifest: manifest({
      kind: 'extension',
      id: 'io.github.nori.token-meter',
      name: { 'zh-CN': '用量表', en: 'Token meter' },
      version: '1.0.2',
      author: 'nori',
      description: { 'zh-CN': '按天统计每个实例用掉的 token', en: 'Counts the tokens each instance used per day' },
      repository: 'https://github.com/nori/aproxy-token-meter',
      license: 'MIT',
    }),
    files: { 'main.mjs': helloSource('Token meter'), 'assets/loader.mjs': '' },
    source: fileSource(11_870),
    facts: checked('nori/aproxy-token-meter', 2_480, { state: 'mismatch', tag: 'v1.0.2', asset: 'token-meter-1.0.2.zip' }),
  },
  lowstar: {
    manifest: manifest({
      kind: 'extension',
      id: 'io.github.k0de.retry-sound',
      name: 'Retry sound',
      version: '0.1.0',
      author: 'k0de',
      description: 'Plays a short sound when a long retry finally succeeds',
      repository: 'https://github.com/k0de/retry-sound',
    }),
    files: { 'main.mjs': helloSource('Retry sound'), 'assets/ding.ogg': '' },
    source: fileSource(23_004),
    facts: checked('k0de/retry-sound', 3, { state: 'match', tag: 'v0.1.0', asset: 'retry-sound-0.1.0.zip', url: 'https://github.com/k0de/retry-sound/releases/tag/v0.1.0' }, { createdAt: now - 9 * DAY, forks: 0, license: null }),
  },
  offline: {
    manifest: manifest({
      kind: 'extension',
      id: 'io.github.nori.token-meter',
      name: { 'zh-CN': '用量表', en: 'Token meter' },
      version: '1.0.2',
      author: 'nori',
      description: { 'zh-CN': '按天统计每个实例用掉的 token', en: 'Counts the tokens each instance used per day' },
      repository: 'https://github.com/nori/aproxy-token-meter',
      license: 'MIT',
    }),
    files: { 'main.mjs': helloSource('Token meter') },
    source: fileSource(9_318),
    facts: { state: 'unreachable', reason: 'network', status: null },
  },
  notgithub: {
    manifest: manifest({
      kind: 'extension',
      id: 'org.codeberg.yan.latency-ring',
      name: 'Latency ring',
      version: '0.9.0',
      author: 'yan',
      description: 'A ring around the mascot that shows the latest latency',
      repository: 'https://codeberg.org/yan/latency-ring',
      license: 'GPL-3.0-only',
    }),
    files: { 'main.mjs': helloSource('Latency ring') },
    source: fileSource(5_550),
    facts: { state: 'not-github', url: 'https://codeberg.org/yan/latency-ring' },
  },
  upgrade: {
    manifest: manifest({
      kind: 'extension',
      id: 'io.github.lin.request-pulse',
      name: { 'zh-CN': '请求节拍', en: 'Request pulse' },
      version: '0.5.0',
      author: 'lin',
      description: { 'zh-CN': '把最近的请求画成一排小柱子，一眼看出哪段时间重试得多', en: 'Draws recent requests as a row of bars, so bursts of retries stand out' },
      repository: 'https://github.com/lin/aproxy-request-pulse',
      license: 'Apache-2.0',
    }),
    files: { 'main.mjs': PULSE_SOURCE },
    source: fileSource(7_204),
    facts: checked('lin/aproxy-request-pulse', 1_211, { state: 'match', tag: 'v0.5.0', asset: 'request-pulse-0.5.0.zip', url: 'https://github.com/lin/aproxy-request-pulse/releases/tag/v0.5.0' }),
  },
};

/** 这些关键词直接以错误结束：演示读包失败时的提示 */
const BROKEN: Record<string, Fault> = {
  broken: fault('package_layout', { reason: 'not-allowed', path: 'bin/helper.exe' }),
  notzip: fault('package_read', { reason: 'not-zip' }),
};

function scenarioOf(name: string): Scenario | Fault {
  const lower = name.toLowerCase();
  for (const [key, f] of Object.entries(BROKEN)) if (lower.includes(key)) return f;
  for (const key of Object.keys(SCENARIOS)) if (key !== 'clean' && lower.includes(key)) return SCENARIOS[key];
  return SCENARIOS.clean;
}

interface Pending {
  check: PackageCheck;
  scenario: Scenario;
  facts: GithubFacts;
  expires: number;
}

/** 一个窗口同时只有一个待装的包：新的检查顶掉旧的 */
let pending: { token: Token; p: Pending } | null = null;
let seq = 0;

function take(token: Token): Pending {
  if (!pending || pending.token !== token || pending.p.expires < Date.now()) throw fault('package_token', {});
  return pending.p;
}

function check(scenario: Scenario | Fault, source: PackageSource): Promise<PackageCheck> {
  if ('code' in scenario) return Promise.reject(scenario);
  const m = scenario.manifest;
  const old = installed.get(m.id);
  if (old && old.entry.kind !== m.kind) return Promise.reject(fault('package_kind_changed', { installed: old.entry.kind }));
  const files = [{ path: 'manifest.json', bytes: 412 }, ...Object.entries(scenario.files).map(([path, text]) => ({ path, bytes: Math.max(text.length, 1_200) }))].sort((a, b) =>
    a.path.localeCompare(b.path),
  );
  const token = `pkg:${++seq}`;
  const result: PackageCheck = {
    token,
    manifest: structuredClone(m),
    files,
    totalBytes: files.reduce((n, f) => n + f.bytes, 0),
    source,
    installed: old ? { version: old.entry.manifest?.version ?? '?', enabled: enabled.includes(m.id) } : null,
    theme: scenario.theme ? structuredClone(scenario.theme) : null,
  };
  // 从目录装的没有文件可比：GitHub 只查仓库，release 一项是 not-a-file
  const facts: GithubFacts =
    source.kind === 'directory' && scenario.facts.state === 'checked' ? { ...scenario.facts, release: { state: 'not-a-file' } } : scenario.facts;
  pending = { token, p: { check: result, scenario, facts, expires: Date.now() + 30 * 60_000 } };
  return later(structuredClone(result), 320);
}

export const packageHandlers = {
  package_list: () => later(list()),

  package_check_file: ({ name }: { name: string; bytes: Uint8Array }) => check(scenarioOf(name), scenarioSource(name)),

  package_check_path: ({ path }: { path: string }) => {
    const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
    const s = scenarioOf(name);
    const source: PackageSource = /\.zip$/i.test(name) ? scenarioSource(name) : { kind: 'directory', name };
    return check(s, source);
  },

  async package_verify({ token }: { token: Token }): Promise<GithubFacts> {
    const p = take(token);
    // 连不上的那种要等到超时才知道
    await later(null, p.facts.state === 'unreachable' ? 2_600 : 1_100);
    return structuredClone(p.facts);
  },

  async package_install({ token, enable }: { token: Token; enable: boolean }): Promise<PackageEntry> {
    const p = take(token);
    pending = null;
    const m = p.check.manifest;
    const old = installed.get(m.id);
    const entry: PackageEntry = {
      id: m.id,
      kind: m.kind,
      manifest: structuredClone(m),
      enabled: false,
      installedAt: Date.now(),
      source: structuredClone(p.check.source),
      github: structuredClone(p.facts),
      issue: null,
      dataBytes: old?.entry.dataBytes ?? 0,
      theme: p.check.theme,
    };
    install({ entry, files: { ...p.scenario.files } });
    // 第一次装的扩展照用户的意思启用；升级保留原来的启用状态
    if (!old && m.kind === 'extension' && enable && !enabled.includes(m.id)) enabled = [...enabled, m.id];
    return later(entryOf(installed.get(m.id)!), 420);
  },

  package_discard: ({ token }: { token: Token }) => {
    if (pending?.token === token) pending = null;
    return later(null);
  },

  package_set_enabled: ({ id, enabled: on }: { id: string; enabled: boolean }) => {
    const i = installed.get(id);
    if (!i) return Promise.reject(fault('package_unknown', { id }));
    if (i.entry.kind !== 'extension') return Promise.reject(fault('desktop_bad_input', {}));
    enabled = on ? (enabled.includes(id) ? enabled : [...enabled, id]) : enabled.filter((x) => x !== id);
    return later(entryOf(i));
  },

  package_uninstall: ({ id, removeData }: { id: string; removeData: boolean }) => {
    if (!installed.delete(id)) return Promise.reject(fault('package_unknown', { id }));
    enabled = enabled.filter((x) => x !== id);
    if (removeData) {
      settings.delete(id);
      storage.delete(id);
      files.delete(id);
    }
    return later(null, 260);
  },

  extension_settings_read: ({ id }: { id: string }) => {
    extension(id);
    return later(structuredClone(settings.get(id) ?? {}));
  },

  extension_settings_write: ({ id, changes }: { id: string; changes: Record<string, unknown> }) => {
    extension(id);
    const next = merge(settings.get(id) ?? {}, changes);
    settings.set(id, next);
    return later(structuredClone(next));
  },

  extension_storage_read: ({ id }: { id: string }) => {
    extension(id);
    return later(structuredClone(storage.get(id) ?? {}));
  },

  extension_storage_write: ({ id, changes, reset }: { id: string; changes: Record<string, unknown>; reset: boolean }) => {
    extension(id);
    const next = merge(reset ? {} : (storage.get(id) ?? {}), changes);
    storage.set(id, next);
    return later(structuredClone(next));
  },

  extension_files_list: ({ id }: { id: string }) => {
    extension(id);
    const dir = files.get(id) ?? new Map();
    return later([...dir.entries()].map(([name, f]) => infoOf(name, f)).sort((a, b) => a.name.localeCompare(b.name)));
  },

  extension_file_read: ({ id, name }: { id: string; name: string }) => {
    extension(id);
    const f = files.get(id)?.get(name);
    if (!f) return Promise.reject(fault('extension_data', { reason: 'name', max: null }));
    return later(f.bytes.slice());
  },

  extension_file_write: ({ id, name, bytes }: { id: string; name: string; bytes: Uint8Array }) => {
    extension(id);
    if (!/^[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}$/.test(name) || name.endsWith('.')) return Promise.reject(fault('extension_data', { reason: 'name', max: null }));
    let dir = files.get(id);
    if (!dir) files.set(id, (dir = new Map()));
    const f = { bytes: bytes.slice(), modifiedAt: Date.now() };
    dir.set(name, f);
    return later(infoOf(name, f));
  },

  extension_file_delete: ({ id, name }: { id: string; name: string }) => {
    extension(id);
    files.get(id)?.delete(name);
    return later(null);
  },
};

function scenarioSource(name: string): PackageSource {
  const s = scenarioOf(name);
  return 'code' in s ? fileSource(0)(name) : s.source(name);
}

function merge(base: Record<string, unknown>, changes: Record<string, unknown>): Record<string, unknown> {
  const next = { ...base };
  for (const [k, v] of Object.entries(changes)) {
    if (v === null) delete next[k];
    else next[k] = structuredClone(v);
  }
  return next;
}

const settings = new Map<string, Record<string, unknown>>([['io.github.lin.request-pulse', { bars: 50, colorful: true }]]);
const storage = new Map<string, Record<string, unknown>>();
const files = new Map<string, Map<string, { bytes: Uint8Array; modifiedAt: number }>>();

function infoOf(name: string, f: { bytes: Uint8Array; modifiedAt: number }): DataFile {
  return { name, bytes: f.bytes.length, modifiedAt: f.modifiedAt };
}

/** desktop.toml 里 [extensions] enabled 这一项，给 read_desktop_state 用 */
export function enabledExtensions(): string[] {
  return [...enabled];
}

/**
 * 演示里包文件的地址：真实窗口里是 aproxy-pkg 协议的地址，这里没有文件，只好把内容做成 blob: 地址。
 * 所以演示的扩展不能相对 import，演示的主题样式表也不能引用相对路径的图片——真实窗口里都可以
 */
export function demoPackageUrl(id: string, path: string): string | null {
  const text = installed.get(id)?.files[path];
  if (text === undefined) return null;
  const type = path.endsWith('.css') ? 'text/css' : path.endsWith('.json') ? 'application/json' : 'text/javascript';
  return URL.createObjectURL(new Blob([text], { type }));
}

/** 审核工具条用：列出可以模拟的安装场景 */
export const INSTALL_SCENARIOS = ['clean', 'theme', 'closed', 'mismatch', 'lowstar', 'offline', 'notgithub', 'upgrade', 'broken'] as const;

/*
 * 界面这一侧的数据。Backend 推来的实例、日志、统计在这里落成 Svelte 的响应式状态；
 * 组件只读这里、只经这里发命令，从不直接碰数据源（原型里的 mock、将来的 IPC 都在 Backend 接口后面）。
 *
 * 「整体心情」（吉祥物的状态）、「在途」「重试中」这类派生也在这里算：
 * 它们是界面怎样概括数据，不是数据本身，换一个 Backend 实现也不该跟着变。
 */
import { backend } from './backend';
import type { BackendEvent, Snapshot } from './backend/types';
import type { MascotMood } from './mascot/types';
import { ui } from './state.svelte';
import type {
  AppSettings,
  ConfigValues,
  ConsentRequest,
  ExtensionInfo,
  Instance,
  LogEntry,
  UiPrefs,
  UpdateInfo,
  UpdateProgress,
  WatchdogInfo,
  WriteCapability,
} from './types';

/** 救回后庆祝多久：够吉祥物跳一下、撒完星星，又不至于盖住接下来的真实状态 */
const RESCUE_CELEBRATION_MS = 3_200;
/** 这么久没有新请求，吉祥物就睡着了 */
const SLEEP_AFTER_MS = 10 * 60_000;
/** 新到的日志高亮多久 */
const FRESH_MS = 1_600;

class Store {
  /** 界面自己的时钟：倒计时、「几秒前」都按它算，250ms 一拍 */
  now = $state(Date.now());
  connected = $state(false);
  instances = $state<Instance[]>([]);
  logs = $state<LogEntry[]>([]);
  watchdog = $state<WatchdogInfo | null>(null);
  rescuedTotal = $state(0);
  rescuedToday = $state(0);
  /** 单调递增的到达计数：日志页靠它算「N 条新日志」 */
  arrivals = $state(0);
  /** 最近一次「长时间重试后救回」的时刻与那一条 */
  rescuedAt = $state(0);
  rescuedKey = $state<string | null>(null);
  settings = $state<AppSettings | null>(null);
  /** 正在进行（或刚完成）的安装 */
  updateProgress = $state<UpdateProgress | null>(null);
  extensions = $state<ExtensionInfo[]>([]);
  /** 等用户回答的写权限请求：一次只问一个，先来的先问 */
  consents = $state<ConsentRequest[]>([]);

  get retrying(): LogEntry[] {
    return this.logs.filter((e) => e.outcome === 'retrying');
  }

  get inflight(): LogEntry[] {
    return this.logs.filter((e) => e.outcome === 'retrying' || e.outcome === 'streaming');
  }

  /**
   * 整体状态，也就是吉祥物的心情。优先级：出错 > 救回 > 重试 > 忙碌 > 空闲/睡着。
   * 救回只是几秒的庆祝，所以排在重试前面——否则只要还有别的请求在重试，这一刻就永远轮不到。
   */
  get mood(): MascotMood {
    if (this.instances.length === 0) return 'sleeping';
    if (this.instances.some((i) => i.status === 'unresponsive')) return 'worried';
    if (this.rescuedAt && this.now - this.rescuedAt < RESCUE_CELEBRATION_MS) return 'rescued';
    if (this.retrying.length > 0) return 'retrying';
    if (this.inflight.length > 0) return 'working';
    const last = Math.max(0, ...this.instances.map((i) => i.lastRequestAt ?? 0));
    if (this.now - last > SLEEP_AFTER_MS) return 'sleeping';
    return 'idle';
  }

  /** 正在重试的请求里失败次数最多的那个失败了几次：吉祥物的角标 */
  get maxRetries(): number {
    return this.retrying.reduce((m, e) => Math.max(m, e.attempts.length), 0);
  }

  get watchdogOn(): boolean {
    return this.settings?.watchdog ?? false;
  }

  instance(port: number): Instance | undefined {
    return this.instances.find((i) => i.port === port);
  }

  extension(id: string): ExtensionInfo | undefined {
    return this.extensions.find((e) => e.id === id);
  }

  // -------------------------------------------------------------------------
  // 连接：先订阅再取快照，订阅与快照之间推来的增量不会丢（快照到了会整体覆盖一次）
  // -------------------------------------------------------------------------

  private started = false;

  connect() {
    if (this.started) return;
    this.started = true;
    backend.subscribe((ev) => this.apply(ev));
    backend.snapshot().then((s) => {
      this.load(s);
      this.connected = true;
    });
    backend.readSettings().then((s) => {
      this.settings = s;
      ui.langPref = s.ui.lang;
      ui.modePref = s.ui.mode;
      ui.palette = s.ui.palette;
      ui.micaEnabled = s.ui.mica;
      ui.mascotId = s.ui.mascot;
      ui.themeSpec = s.ui.theme;
      this.persistUiPrefs();
    });
    backend.listExtensions().then((x) => (this.extensions = x));
    setInterval(() => this.tick(), 250);
  }

  /** 偏好改了就写回后端（settings.json 的 ui 段）。读到设置之后才开始，免得用默认值覆盖用户的设置 */
  private persistUiPrefs() {
    $effect.root(() => {
      $effect(() => {
        const prefs: UiPrefs = {
          lang: ui.langPref,
          mode: ui.modePref,
          palette: ui.palette,
          mica: ui.micaEnabled,
          mascot: ui.mascotId,
          theme: ui.themeSpec,
        };
        const saved = this.settings?.ui;
        // 主题是对象，逐项按 JSON 比较；预览中的主题（ui.themePreview）不在这里，预览不落盘
        const same = (k: keyof UiPrefs) => JSON.stringify(prefs[k]) === JSON.stringify(saved?.[k]);
        if (!saved || (Object.keys(prefs) as (keyof UiPrefs)[]).every(same)) return;
        this.settings = { ...this.settings!, ui: prefs };
        backend.writeSettings({ ui: prefs });
      });
    });
  }

  private tick() {
    this.now = Date.now();
    // 新条目都在末尾，往回看到第一条不新的就停
    for (let i = this.logs.length - 1; i >= 0 && i >= this.logs.length - 50; i--) {
      const e = this.logs[i];
      if (e.fresh && this.now - e.time > FRESH_MS) e.fresh = false;
    }
  }

  private load(s: Snapshot) {
    this.instances = s.instances;
    this.logs = s.logs.map((e) => ({ ...e, fresh: false }));
    this.watchdog = s.watchdog;
    this.rescuedTotal = s.rescuedTotal;
    this.rescuedToday = s.rescuedToday;
  }

  private apply(ev: BackendEvent) {
    switch (ev.type) {
      case 'snapshot':
        this.load(ev.snapshot);
        break;
      case 'instances':
        this.instances = ev.instances;
        break;
      case 'logs-appended':
        for (const e of ev.entries) this.logs.push({ ...e, fresh: true });
        this.arrivals += ev.entries.length;
        break;
      case 'logs-updated':
        for (const e of ev.entries) {
          const i = this.logs.findIndex((x) => x.key === e.key);
          if (i >= 0) this.logs[i] = { ...e, fresh: this.logs[i].fresh };
        }
        break;
      case 'logs-removed': {
        const gone = new Set(ev.keys);
        this.logs = this.logs.filter((e) => !gone.has(e.key));
        break;
      }
      case 'stats':
        this.watchdog = ev.watchdog;
        this.rescuedTotal = ev.rescuedTotal;
        this.rescuedToday = ev.rescuedToday;
        break;
      case 'rescued':
        this.rescuedAt = ev.at;
        this.rescuedKey = ev.key;
        break;
      case 'update-progress':
        this.updateProgress = ev.progress;
        break;
      case 'extensions':
        this.extensions = ev.extensions;
        break;
      case 'consent-request':
        this.consents.push(ev.request);
        break;
    }
  }

  // -------------------------------------------------------------------------
  // 命令：全部转给 Backend。实例状态的变化随后经事件推回来，这里不自己改本地数据
  // -------------------------------------------------------------------------

  start(port?: number) {
    return backend.startInstance(port);
  }

  stop(port: number) {
    return backend.stopInstance(port);
  }

  restart(port: number) {
    return backend.restartInstance(port);
  }

  forceStop(port: number) {
    return backend.forceStopInstance(port);
  }

  reconnect(port: number) {
    return backend.reconnectInstance(port);
  }

  setPinned(e: LogEntry, pinned: boolean) {
    return backend.setPinned(e.key, pinned);
  }

  requestDetail(key: string) {
    return backend.requestDetail(key);
  }

  readConfig(port: number) {
    return backend.readConfig(port);
  }

  writeConfig(port: number, changes: Partial<ConfigValues>) {
    return backend.writeConfig(port, changes);
  }

  async updateSettings(changes: Partial<Omit<AppSettings, 'ui'>>) {
    if (this.settings) this.settings = { ...this.settings, ...changes };
    await backend.writeSettings(changes);
  }

  checkUpdate(channel: UpdateInfo['channel']) {
    return backend.checkUpdate(channel);
  }

  installUpdate(version: string) {
    this.updateProgress = null;
    return backend.installUpdate(version);
  }

  setExtensionEnabled(id: string, enabled: boolean) {
    return backend.setExtensionEnabled(id, enabled);
  }

  revokeGrant(id: string, capability: WriteCapability) {
    return backend.revokeGrant(id, capability);
  }

  answerConsent(id: string, allow: boolean) {
    this.consents = this.consents.filter((c) => c.id !== id);
    return backend.answerConsent(id, allow);
  }

  validateConfig(port: number, changes: Partial<ConfigValues>) {
    return backend.validateConfig(port, changes);
  }

  // 同一个自定义吉祥物会同时出现在好几处（标题栏、总览、设置），代码只取一次。
  // 按 id@版本 缓存：吉祥物升级后版本变了，自然会取新代码
  private mascotCode = new Map<string, Promise<string>>();

  readMascot(id: string): Promise<string> {
    const key = `${id}@${this.extension(id)?.version ?? ''}`;
    let p = this.mascotCode.get(key);
    if (!p) {
      p = backend.readMascot(id);
      // 失败的不留在缓存里，下次挂载时再试
      p.catch(() => this.mascotCode.delete(key));
      this.mascotCode.set(key, p);
    }
    return p;
  }
}

export const store = new Store();

/*
 * 界面这一侧的数据：契约（backend/contract.ts）交来的实例卡片、最近的请求、看护者、安装进度、设置与置顶，
 * 在这里落成 Svelte 的响应式状态。组件只读这里、只经这里发命令，从不直接碰传输层。
 *
 * 数据怎么保持新鲜（契约文档 .agents/plan/w5b-desktop-api.md 第 4 节的规矩）：
 *   - 事件只说「哪块数据旧了」，不带数据。收到后在这里重读对应的那块，读完（成功或失败）这个处理函数才返回，
 *     传输层随后才确认这一帧、Rust 才发下一帧。所以重读天然是一个接一个的，不会两份结果互相覆盖。
 *   - resync（每次接上通道的第一帧）把一切都重读一遍，并让 epoch 加一：分页的列表、打开着的详情
 *     看到 epoch 变了就把游标、选择全部作废，从头再读。
 *   - requests-changed 只说「这次运行的请求变了」：这里重读快照里的最近 50 条，并给这次运行的 revision 加一，
 *     页面上按运行分页的列表看到它变了就从第一页重读。快照里的最近请求只是整个 home 的开头，
 *     不能当成某次运行的完整列表，也不能拿「列表里没有了」去推断请求不存在。
 *
 * 「整体心情」（吉祥物的状态）、「在途」「重试中」这类概括也在这里算：它们是界面怎样概括数据，
 * 不是数据本身，所以不在契约里。
 */
import { link } from './backend';
import type {
  About,
  BodyPart,
  Bookmark,
  Card,
  Chunk,
  ConfigTarget,
  DesktopEvent,
  DesktopState,
  Detail,
  Edit,
  Job,
  LogPage,
  Preferences,
  Ref,
  Registration,
  Row,
  Run,
  Selection,
  SettingsView,
  StartTarget,
  Token,
  TomlIssue,
  UpdateInfo,
  Watchdog,
} from './backend/contract';
import { toFailure, type Failure } from './backend/link';
import type { MascotMood } from './mascot/types';
import { packages } from './packages.svelte';
import { ui, type LangPref } from './state.svelte';

/** 救回后庆祝多久：够吉祥物跳一下、撒完星星，又不至于盖住接下来的真实状态 */
const RESCUE_CELEBRATION_MS = 3_200;
/** 这么久没有新请求，吉祥物就睡着了 */
const SLEEP_AFTER_MS = 10 * 60_000;

/** 一次运行的键：同一个端口重启之后是另一次运行，所以端口不够用 */
export function runKey(run: Run): string {
  return `${run.port}/${run.instanceId}`;
}

export function sameRun(a: Run | null, b: Run | null): boolean {
  return !!a && !!b && a.port === b.port && a.instanceId === b.instanceId;
}

/** 一条请求的键：只在界面里当键用，不拆开、不送回 Rust（送回去的永远是 Ref 本身） */
export function refKey(ref: Ref): string {
  return `${runKey(ref.run)}/${ref.requestId}`;
}

export function sameRef(a: Ref | null, b: Ref | null): boolean {
  return !!a && !!b && a.requestId === b.requestId && sameRun(a.run, b.run);
}

/** 界面偏好里的语言：认不出的一律按「跟随系统」 */
function langOf(lang: string): LangPref {
  return lang === 'zh-CN' || lang === 'en' ? lang : 'system';
}

class Store {
  /** 数据是不是演示用的假数据：标题栏据此常驻标明 */
  readonly demo = link.demo;
  /** 界面自己的时钟：倒计时、「几秒前」都按它算，250ms 一拍 */
  now = $state(Date.now());
  /** 第一次 resync 读完了：之前页面显示「正在连接」，而不是「没有实例」 */
  connected = $state(false);
  /** 最近一次重读失败的原因；下一次重读成功就清掉 */
  readFault = $state.raw<Failure | null>(null);

  /** 按端口升序。raw：每次重读整份替换，不需要深层代理 */
  cards = $state.raw<Card[]>([]);
  /** 整个 home 最近的 50 条，新的在前 */
  recent = $state.raw<Row[]>([]);
  watchdog = $state.raw<Watchdog | null>(null);
  /** 这个窗口里正在进行或刚结束的安装 */
  job = $state.raw<Job | null>(null);
  settings = $state.raw<SettingsView | null>(null);
  autostart = $state.raw<Registration | null>(null);
  /** 新置顶的在前 */
  bookmarks = $state.raw<Bookmark[]>([]);
  /** 这个窗口的版本与 home；读到之前为 null */
  about = $state.raw<About | null>(null);
  /** 最近一次检查更新的结果：导航上的「新版本」角标据它显示。不自动检查（检查要联网），用户在更新页点了才有 */
  offer = $state.raw<UpdateInfo | null>(null);
  /** 最近一次读界面状态时因为损坏被重置成默认的文件：设置页要告诉用户 */
  reset = $state.raw<DesktopState['reset']>([]);
  /** desktop.toml 里的问题（语法错时页面沿用手上的偏好，写入被拒绝）：设置页要告诉用户错在哪一行 */
  tomlIssues = $state.raw<TomlIssue[]>([]);
  /** 读成一次界面状态加一：扩展的 settings.onChange 据此去比一比自己那张表变没变 */
  desktopReads = $state(0);

  /** resync 一次加一：分页的游标、详情的选择看到它变了就作废 */
  epoch = $state(0);
  /** 任何一次运行的请求变了就加一：整个 home 的请求列表据此从第一页重读 */
  changes = $state(0);
  /** 每次运行的请求变过几次（键是 runKey）：按运行分页的列表据此从第一页重读 */
  revisions = $state.raw<ReadonlyMap<string, number>>(new Map());

  /** 最近一次「长时间重试后救回」的时刻（界面时钟）与那一条 */
  rescuedAt = $state(0);
  rescuedRef = $state.raw<Ref | null>(null);

  /** 正在重试的请求数：各次运行的计数之和 */
  get retrying(): number {
    return this.cards.reduce((n, c) => n + (c.activity?.retrying ?? 0), 0);
  }

  /** 在途的请求数（含正在重试的） */
  get inflight(): number {
    return this.cards.reduce((n, c) => n + (c.activity?.inFlight ?? 0), 0);
  }

  /**
   * 整体状态，也就是吉祥物的心情。优先级：出错 > 救回 > 重试 > 忙碌 > 空闲/睡着。
   * 救回只是几秒的庆祝，所以排在重试前面——否则只要还有别的请求在重试，这一刻就永远轮不到。
   */
  get mood(): MascotMood {
    if (this.cards.length === 0) return 'sleeping';
    if (this.cards.some((c) => c.liveness === 'unresponsive')) return 'worried';
    if (this.rescuedAt && this.now - this.rescuedAt < RESCUE_CELEBRATION_MS) return 'rescued';
    if (this.retrying > 0) return 'retrying';
    if (this.inflight > 0) return 'working';
    const last = Math.max(0, ...this.cards.map((c) => c.activity?.lastRequestAt ?? 0));
    if (this.now - last > SLEEP_AFTER_MS) return 'sleeping';
    return 'idle';
  }

  /**
   * 在途请求里失败次数最多的那条失败了几次：吉祥物的角标。只看得到最近 50 条里的，
   * 这只是个装饰，看不全也无妨。正在退避等待的，已经发出的每一次都失败了；还在尝试的，最后一次还没有结果
   */
  get maxRetries(): number {
    return this.recent.reduce((m, r) => {
      if (r.outcome !== 'in-flight') return m;
      return Math.max(m, r.phase === 'backoff' ? r.attempts : r.attempts - 1);
    }, 0);
  }

  get watchdogOn(): boolean {
    return this.settings?.watchdogEnabled ?? false;
  }

  card(port: number): Card | undefined {
    return this.cards.find((c) => c.port === port);
  }

  /** 这次运行的请求变过几次；从没变过是 0 */
  revision(run: Run | null): number {
    return run ? (this.revisions.get(runKey(run)) ?? 0) : 0;
  }

  pinned(ref: Ref): boolean {
    return this.bookmarks.some((b) => sameRef(b.ref, ref));
  }

  // -------------------------------------------------------------------------
  // 连接与事件
  // -------------------------------------------------------------------------

  private started = false;

  connect() {
    if (this.started) return;
    this.started = true;
    link.attach((ev) => this.onEvent(ev));
    setInterval(() => (this.now = Date.now()), 250);
    this.persistPreferences();
  }

  private async onEvent(ev: DesktopEvent): Promise<void> {
    switch (ev.type) {
      case 'resync':
        this.epoch++;
        this.rescuedAt = 0;
        this.rescuedRef = null;
        await Promise.all([
          this.readSnapshot(),
          this.readSettings(),
          this.readDesktopState(),
          this.readAutostart(),
          this.read(
            () => link.call('about', {}),
            (a) => (this.about = a),
          ),
          packages.refresh(),
        ]);
        this.connected = true;
        break;
      case 'instances-changed':
        await this.readSnapshot();
        break;
      case 'requests-changed': {
        const next = new Map(this.revisions);
        next.set(runKey(ev.run), (next.get(runKey(ev.run)) ?? 0) + 1);
        this.revisions = next;
        this.changes++;
        await this.readSnapshot();
        break;
      }
      case 'rescued':
        // 用界面时钟而不是 ev.at（请求结束的时刻）起算：通知晚到几秒，庆祝也要完整地放一遍
        this.rescuedAt = Date.now();
        this.rescuedRef = ev.ref;
        break;
      case 'update-progress':
        this.job = ev.job;
        break;
      case 'desktop-state-changed':
        await this.readDesktopState();
        break;
      case 'settings-changed':
        await this.readSettings();
        break;
    }
  }

  /** 重读一块数据。失败时记下原因、保留上一次读到的内容（页面据 readFault 提示），不往外抛：这一帧照样确认 */
  private async read<T>(load: () => Promise<T>, apply: (value: T) => void): Promise<void> {
    try {
      apply(await load());
      this.readFault = null;
    } catch (raw) {
      this.readFault = toFailure(raw);
    }
  }

  private readSnapshot() {
    return this.read(
      () => link.call('snapshot', {}),
      (s) => {
        this.cards = s.instances;
        this.recent = s.recent;
        this.watchdog = s.watchdog;
        this.job = s.update;
        // 已经不在的运行，它的计数也不必留着
        const live = new Set(s.instances.flatMap((c) => (c.run ? [runKey(c.run)] : [])));
        if ([...this.revisions.keys()].some((k) => !live.has(k))) {
          this.revisions = new Map([...this.revisions].filter(([k]) => live.has(k)));
        }
      },
    );
  }

  private readSettings() {
    return this.read(
      () => link.call('read_settings', {}),
      (s) => (this.settings = s),
    );
  }

  private readAutostart() {
    return this.read(
      () => link.call('read_autostart', {}),
      (r) => (this.autostart = r),
    );
  }

  private readDesktopState() {
    return this.read(
      () => link.call('read_desktop_state', {}),
      (s) => {
        this.bookmarks = s.bookmarks;
        this.reset = s.reset;
        this.tomlIssues = s.desktopToml;
        this.desktopReads++;
        packages.setEnabledOrder(s.enabledExtensions);
        // 自己的偏好写到一半时读到的是写之前的样子，套上去会把用户刚改的值闪回去；等写完再说。
        // desktop.toml 这次没读成（null）时保留手上的偏好：用户改到一半的文件不该把窗口打回默认
        if (this.writingPrefs === 0 && s.preferences) this.applyPreferences(s.preferences);
      },
    );
  }

  /**
   * 窗口重新获得焦点：desktop.toml 可能刚被手改过，extensions/ 下可能多了手工拷进来的包。
   * 偏好与包的列表都重读一遍（不读实例与请求，那些有事件通知）
   */
  async refreshOnFocus(): Promise<void> {
    if (!this.connected) return;
    await Promise.all([this.readDesktopState(), packages.refresh()]);
  }

  // -------------------------------------------------------------------------
  // 界面偏好：存在 Rust 那边（<home>/desktop/desktop.toml），界面里的副本在 ui（state.svelte.ts）上
  // -------------------------------------------------------------------------

  /** 最近一次从 Rust 读到或写成的偏好。读到之前为 null，这时不往回写 */
  private prefs: Preferences | null = null;
  private writingPrefs = 0;

  private applyPreferences(p: Preferences) {
    this.prefs = { ...p };
    ui.langPref = langOf(p.lang);
    ui.modePref = p.mode;
    ui.palette = p.palette;
    ui.micaEnabled = p.mica;
    // 认不出的吉祥物由吉祥物注册表回落到默认的那个；不在了的主题包由 packages 回落到内置主题
    ui.mascotId = p.mascot;
    ui.themeId = p.theme;
  }

  /** 用户在界面上改了偏好就写回去，只写改了的那几项 */
  private persistPreferences() {
    $effect.root(() => {
      $effect(() => {
        const now: Preferences = {
          lang: ui.langPref,
          mode: ui.modePref,
          palette: ui.palette,
          mica: ui.micaEnabled,
          mascot: ui.mascotId,
          theme: ui.themeId,
        };
        const saved = this.prefs;
        if (!saved) return;
        const changes: Partial<Preferences> = {};
        for (const key of Object.keys(now) as (keyof Preferences)[]) {
          if (now[key] !== saved[key]) Object.assign(changes, { [key]: now[key] });
        }
        if (Object.keys(changes).length === 0) return;
        this.prefs = { ...saved, ...changes };
        this.writingPrefs++;
        link.call('set_preferences', { changes }).then(
          (result) => {
            if (--this.writingPrefs === 0) this.applyPreferences(result);
          },
          (raw) => {
            // 没写进去：界面退回上一次确认过的样子，免得显示的与存着的不一致
            this.readFault = toFailure(raw);
            if (--this.writingPrefs === 0) this.applyPreferences(saved);
          },
        );
      });
    });
  }

  // -------------------------------------------------------------------------
  // 命令。失败时以 Failure 拒绝，文字由调用的页面按码来写。
  // 实例状态的变化随后经事件通知过来，这里不自己改卡片
  // -------------------------------------------------------------------------

  listConfigs() {
    return link.call('list_configs', {});
  }

  start(target: StartTarget) {
    return link.call('start_instance', { target });
  }

  stop(run: Run) {
    return link.call('stop_instance', { run });
  }

  restart(run: Run) {
    return link.call('restart_instance', { run });
  }

  forceStop(run: Run) {
    return link.call('force_stop_instance', { run });
  }

  restore(port: number, expectedRestore: Token) {
    return link.call('restore_instance', { port, expectedRestore });
  }

  forgetRestore(port: number, expectedRestore: Token) {
    return link.call('forget_restore', { port, expectedRestore });
  }

  /** 让 Rust 马上重新找一遍实例，不等下一次定时扫描 */
  rescan() {
    return link.call('rescan', {});
  }

  /** 这次运行的 format 条目。每次都现问实例，不进快照：只有实例页与扩展要看 */
  formatList(run: Run) {
    return link.call('format_list', { run });
  }

  /** 只重启选中的 format 条目（null 是全部），实例不换、config.toml 不重读 */
  formatRestart(run: Run, names: string[] | null) {
    return link.call('format_restart', { run, names });
  }

  /** 调用一个条目的 control 方法；方法与参数的含义由 format 的作者定 */
  formatCall(run: Run, name: string, method: string, params: unknown) {
    return link.call('format_call', { run, name, method, params });
  }

  queryLogs(run: Run | null, cursor: Token | null, limit: number): Promise<LogPage> {
    return link.call('query_logs', { run, cursor, limit });
  }

  // 详情的选择：每选一次（哪怕重新打开同一条）generation 加一，Rust 只给最新那次选择交数据，
  // 旧选择上还没回来的块自然作废。session 是这次 attach 的，重新接上之后旧的选择一律作废
  private generation = 0;

  /** 打开一条请求的详情。没接上通道时以 transport 拒绝 */
  async openDetail(ref: Ref): Promise<{ detail: Detail; selection: Selection }> {
    const session = link.session();
    if (!session) throw toFailure(null);
    const selection: Selection = { session, generation: ++this.generation };
    const detail = await link.call('request_detail', { ...selection, ref });
    return { detail, selection };
  }

  bodyChunk(selection: Selection, ref: Ref, part: BodyPart, offset: number, limit: number): Promise<Chunk> {
    return link.call('body_chunk', { ...selection, ref, part, offset, limit });
  }

  /** 关掉详情：Rust 停掉这次选择上还在读的块 */
  cancelDetail(selection: Selection) {
    return link.call('cancel_detail', selection);
  }

  async setPinned(ref: Ref, pinned: boolean) {
    this.bookmarks = await link.call('set_pinned', { ref, pinned });
  }

  readConfig(target: ConfigTarget) {
    return link.call('read_config', { target });
  }

  validateConfig(target: ConfigTarget, changes: Edit[]) {
    return link.call('validate_config', { target, changes });
  }

  writeConfig(target: ConfigTarget, changes: Edit[], acknowledgeExternalAcl: boolean) {
    return link.call('write_config', { target, changes, acknowledgeExternalAcl });
  }

  async setWatchdog(enabled: boolean) {
    const now = await link.call('set_watchdog', { enabled });
    if (this.settings) this.settings = { ...this.settings, watchdogEnabled: now };
  }

  async setAutostart(enabled: boolean) {
    this.autostart = await link.call('set_autostart', { enabled });
  }

  async checkUpdate(channel: 'auto' | 'pre') {
    const info = await link.call('check_update', { channel });
    this.offer = info;
    return info;
  }

  installUpdate(check: Token) {
    return link.call('install_update', { check });
  }

}

export const store = new Store();

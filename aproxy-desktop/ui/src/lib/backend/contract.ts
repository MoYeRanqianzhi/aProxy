/*
 * 桌面窗口与 Rust 之间的契约：每个命令的参数与结果、事件、数据形状、错误。
 *
 * 这里只有类型，逐项对应 Rust 一侧 `aproxy_ipc::desktop` 的定义；字段名、枚举值的拼写都以那边序列化出来的
 * 为准（字段 camelCase，枚举值 kebab-case）。界面自己的概括（心情、显示用的状态词）不放在这里。
 *
 * 读这些类型时要记住的几件事：
 *   - 交到页面的只有脱敏后的投影。密钥在这里永远只有「设没设」，没有任何字段能拿到明文；
 *     打码后的显示值（URL 里的 ***、<secret>）不能当成新值写回去。
 *   - 标着「来自客户端或上游」的字符串（方法、路径、模型名、头、报告摘要）是不可信的显示文本，
 *     只能当文本渲染，不能当 HTML，也不能拿来当程序里的键。
 *   - Token、Session 是 Rust 发的不透明句柄，只能原样带回去比较，不是路径、端口或命令。
 *   - 所有计数、字节数、时长都是 JSON 数字（Rust 在 2^53 − 1 处饱和）；唯一的例外是配置文件里的整数
 *     （`Cell` 的 integer），它以十进制字符串给出，因为文件里可以写任意大的整数。
 *   - null 都是显式给出的，数组没有内容时是 []。
 */
import type { Channel } from '@tauri-apps/api/core';

/** 没有参数的命令传一个空对象 */
export type None = Record<string, never>;
/** Unix 毫秒时间戳 */
export type Time = number;
/** 一次 attach 的标识：事件与详情的选择都挂在它下面，重新 attach 之后旧的作废 */
export type Session = string;
/** Rust 发的不透明句柄，只用来原样带回去比较 */
export type Token = string;

/** 一次运行：端口加上实例这次启动的标识。同一个端口上重启之后是另一次运行 */
export interface Run {
  port: number;
  instanceId: string;
}

/** 一条请求：在哪次运行里、请求的标识 */
export interface Ref {
  run: Run;
  requestId: string;
}

/** 详情的一次选择。generation 由页面发，每选一次（哪怕重新打开同一条）加一 */
export interface Selection {
  session: Session;
  generation: number;
}

// ---------------------------------------------------------------------------
// 实例
// ---------------------------------------------------------------------------

/**
 * 实例还在不在：
 *   live 答复了并且核验过；unresponsive 进程在但不答复（只有它可以强制停止）；
 *   incompatible、unknown 说明不了它死没死，不能强制停止；down 进程不在了。
 */
export type Liveness = 'live' | 'unresponsive' | 'incompatible' | 'down' | 'unknown';

/** 这个实例的请求数据是怎么来的。polling 只说明没有可用的事件流，不说明实例卡住了 */
export type Feed = 'offline' | 'joining' | 'streaming' | 'polling' | 'disconnected';

/** 请求历史为什么看不了：旧版本的实例不在源头脱敏（要重启它），或者它不提供历史 */
export type HistoryUnavailable = 'source-redaction-missing' | 'unsupported';

export type Trigger = 'any' | 'accept' | 'body-stream' | 'unknown';

export interface Card {
  port: number;
  /** 没有可信的运行记录时为 null：这张卡片上不能做任何针对运行的操作 */
  run: Run | null;
  /** 只用来显示，任何操作都不认它 */
  pid: number | null;
  version: string | null;
  startedAt: Time | null;
  liveness: Liveness;
  /** 实例不在了、但留着「应当在运行」的记录：等待恢复 */
  restorePending: boolean;
  restoreReason: 'config-missing' | 'unknown' | null;
  /** 恢复或放弃恢复时带回去的句柄；没有就不提供这两个操作 */
  expectedRestore: Token | null;
  feed: Feed;
  historyUnavailable: HistoryUnavailable | null;
  /** 现在能做哪些读：请求流、详情、体 */
  reads: { historyFeed: boolean; detail: boolean; body: boolean };
  state: 'starting' | 'serving' | 'swap-prepared' | 'stopping' | 'unknown' | null;
  activity: Activity | null;
  metadata: CardMeta | null;
}

export interface CardMeta {
  listen: string;
  /** 已打码 */
  baseUrl: string;
  configPath: string | null;
  aliases: string[];
  delivery: 'forward' | 'retry' | 'unknown';
  diskCache: boolean;
  /** 秒；0 是关闭 */
  keepaliveIntervalSecs: number;
  keepaliveTrigger: Trigger;
  retryRuleCount: number;
}

/** 这次运行的计数：实例重启后从头算 */
export interface Activity {
  /** 还没有请求时是启动时刻 */
  lastRequestAt: Time;
  requestsTotal: number;
  retriesTotal: number;
  /** 重试之后成功的 */
  rescuedTotal: number;
  /** 从仅转发改成重试的 */
  upgradedTotal: number;
  inFlight: number;
  retrying: number;
  byCode: Partial<Record<HistoryCode, number>>;
  lastError: FailureAt | null;
  buckets: Buckets;
}

/** 按时间分桶的计数，旧的在前；三个数组为空表示实例不提供这组数据 */
export interface Buckets {
  widthSecs: number;
  requests: number[];
  retries: number[];
  /** 失败的尝试次数，不是救回数 */
  failures: number[];
}

export interface FailureAt {
  code: HistoryCode;
  at: Time;
  status: number | null;
}

export interface Watchdog {
  pid: number;
  state: 'running' | 'stale' | 'unknown';
  lastScanAt: Time;
  restarts: { port: number; at: Time; kind: 'crash' | 'hang' | 'unknown'; ok: boolean }[];
}

export interface Snapshot {
  /** 按端口升序；不在里面的就是已经没有了 */
  instances: Card[];
  /** 整个 home 最近的 50 条，新的在前 */
  recent: Row[];
  /** 没有活着的看护者时为 null */
  watchdog: Watchdog | null;
  /** 这个窗口里正在进行或刚结束的安装 */
  update: Job | null;
}

// ---------------------------------------------------------------------------
// 请求
// ---------------------------------------------------------------------------

export type Phase =
  | 'reading-body'
  | 'transforming'
  | 'attempting'
  | 'backoff'
  | 'replaying'
  | 'forwarding'
  | 'done'
  | 'unknown';

/** 「救回」不是结局：它是 ok 而且尝试过不止一次。「流式」也不是结局 */
export type Outcome =
  | 'in-flight'
  | 'ok'
  | 'upstream-error'
  | 'aproxy-error'
  | 'format-response'
  | 'client-gone'
  | 'unknown';

export interface Row {
  ref: Ref;
  /** 运行内的序号；revision 是这条记录被改过几次，同一条的两份里取终态的、其次取 revision 大的 */
  seq: number;
  revision: number;
  startedAt: Time;
  endedAt: Time | null;
  /** 来自客户端：任意 HTTP 方法；路径里的查询值已打码 */
  method: string;
  path: string;
  model: string | null;
  userAgent: string | null;
  stream: boolean;
  delivery: 'forward' | 'retry' | 'unknown';
  upgraded: boolean;
  outcome: Outcome;
  code: HistoryCode | null;
  /** 交给客户端的状态码；还没发为 null */
  clientStatus: number | null;
  phase: Phase;
  /** 只在退避等待时有 */
  nextAttemptAt: Time | null;
  /** 总尝试次数。tries 可能不全（triesOmitted 是省略的条数），不要拿 tries.length 当总数 */
  attempts: number;
  triesOmitted: number;
  tries: Try[];
  keepalive: boolean;
  heartbeats: number;
  requestBytes: number;
  responseBytes: number;
  reports: Report[];
  truncated: boolean;
}

export interface Try {
  /** 从 1 起 */
  attempt: number;
  startedAt: Time;
  durationMs: number | null;
  upstreamStatus: number | null;
  code: HistoryCode | null;
  responseBytes: number;
  decision: Decision | null;
  reports: Report[];
}

export interface Decision {
  /** 做出决定的规则或 format 的名字 */
  by: string;
  decision: 'retry' | 'stop' | 'unknown';
  delayMs: number | null;
  reroute: boolean | null;
}

export type Stage = 'request' | 'retry' | 'decide' | 'response' | 'stream' | 'heartbeat' | 'notify' | 'unknown';

/** format 程序报告自己做了什么 */
export interface Report {
  format: string;
  stage: Stage;
  summary: string;
  actions: Action[];
}

export type Action = (
  | { type: 'protocol-convert'; from: string; to: string }
  | { type: 'key-select'; keyId: string }
  | { type: 'route'; channel: string; modelFrom: string | null; modelTo: string | null }
  | { type: 'header-edit'; name: string; op: 'set' | 'remove' }
  | { type: 'body-edit'; path: string; op: 'set' | 'remove' }
  | { type: 'other' }
) & { label: string | null };

export interface Header {
  name: string;
  /** 密钥只给一个标记，没有任何提示 */
  value: string | { secret: true };
}

export interface BodyInfo {
  /** 留存下来的解码后字节数 */
  bytes: number;
  state: 'retained' | 'same-as-previous' | 'evicted' | 'not-retained' | 'unknown';
  cause: 'budget-disabled' | 'exceeds-budget' | 'decode-failed' | 'unknown' | null;
  encoding: string | null;
}

export interface Message {
  /** 保持顺序，同名的头各占一项 */
  headers: Header[];
  /** 没有记录时为 null */
  body: BodyInfo | null;
}

export interface TryDetail {
  /** 按这个编号与 Row.tries 对上，不要按数组下标 */
  attempt: number;
  sent: (Message & { method: string; url: string }) | null;
  response: (Message & { upstreamStatus: number }) | null;
  error: { code: HistoryCode; message: string } | null;
}

/** 一条请求的详情：只有元数据，体要另外按块取 */
export interface Detail {
  record: Row;
  request: Message;
  tries: TryDetail[];
}

/** 要取哪一段体：客户端的请求体，或某次尝试发出去的、收到的 */
export type BodyPart = { kind: 'request' } | { kind: 'sent' | 'response'; attempt: number };

export interface Chunk {
  totalBytes: number;
  offset: number;
  /** base64 只是传输方式；里面已知的密钥已被等长的 * 盖住 */
  dataB64: string;
  /** 现在还有后续；more 与 complete 都为假表示「目前没有更多」，要用户再点才重读 */
  more: boolean;
  complete: boolean;
}

export interface LogPage {
  /** 新的在前 */
  entries: Row[];
  /** 往前翻用的游标，只能用一次；没有更早的为 null */
  next: Token | null;
  /** 这次翻页里没能读全的运行 */
  missing: { run: Run; reason: 'changed' | 'unavailable' | 'disconnected' }[];
}

// ---------------------------------------------------------------------------
// 目标、配置、设置
// ---------------------------------------------------------------------------

/** 启动的目标：list_configs 给出的选择原样带回，页面不传路径 */
export interface StartTarget {
  kind: 'default' | 'config' | 'alias';
  id: Token;
}

/** 配置表单的目标：一项选择，或一次运行实际用的那个文件。校验与写回时带上读的时候拿到的 fileToken */
export type ConfigTarget = StartTarget | { kind: 'run'; run: Run; expectedFile?: Token };

export interface Choice {
  target: StartTarget;
  /** 别名、文件名或 default */
  label: string;
  /** 只用来显示 */
  path: string;
  exists: boolean;
  isDefault: boolean;
}

export type Source = 'override' | 'toml' | 'settings' | 'env' | 'default';

export type TextKey =
  | 'base_url'
  | 'listen_addr'
  | 'api_key'
  | 'proxy'
  | 'proxy_username'
  | 'proxy_password'
  | 'keepalive.heartbeat';

export type SecondsKey =
  | 'timeouts.connect_secs'
  | 'timeouts.first_byte_secs'
  | 'timeouts.read_idle_secs'
  | 'keepalive.interval_secs';

export type HeaderTable = 'extra_headers' | 'override_headers';

/** 对配置文件的一处修改。只传改了的；没改的密钥不要传 */
export type Edit =
  | { op: 'set'; key: TextKey; value: string }
  | { op: 'set'; key: SecondsKey; value: number }
  | { op: 'set'; key: 'keepalive.trigger'; value: Exclude<Trigger, 'unknown'> }
  | { op: 'unset'; key: TextKey | SecondsKey | 'keepalive.trigger' }
  | { op: 'set-header'; key: HeaderTable; name: string; value: string }
  | { op: 'unset-header'; key: HeaderTable; name: string };

/** 只读的显示树。map 里的 name 是数据（文件里的键、头名），不是对象属性 */
export type Cell =
  | { kind: 'none' }
  | { kind: 'secret'; set: boolean }
  | { kind: 'text'; text: string }
  | { kind: 'bool'; value: boolean }
  | { kind: 'integer'; decimal: string }
  | { kind: 'list'; items: Cell[] }
  | { kind: 'map'; entries: { name: string; value: Cell }[] };

export interface ConfigField {
  /** 与命令行里的写法一致的键 */
  key: string;
  value: Cell;
  source: Source | null;
  /** 头表里的一项才有；编辑它时用这里的 table 与 name */
  header: { table: HeaderTable; name: string } | null;
  editable: boolean;
}

export interface Issue {
  code: DiagCode;
  severity: 'error' | 'warning' | 'info';
  key: string | null;
  file: string;
  line: number | null;
  column: number | null;
}

export interface ConfigView {
  fileToken: Token | null;
  /** 解析出的文件路径；第一次使用时文件可能还不存在 */
  file: string;
  fields: ConfigField[];
  /** 全局默认里因为目标不同而没有继承下来的凭据 */
  withheld: {
    key: string;
    from: 'settings';
    reason: 'base-url-differs' | 'proxy-differs';
    defaultTarget: string;
    target: string;
  }[];
  diagnostics: Issue[];
  /** Windows：文件在 home 之外，沿用所在目录的访问权限，写之前要用户确认 */
  externalAcl: boolean;
}

export interface Validation {
  canSave: boolean;
  diagnostics: Issue[];
  /** 新的监听端口被别的实例占着 */
  occupiedPort: number | null;
  externalAcl: boolean;
}

export interface Saved {
  changed: boolean;
  /** 正在用这个文件的实例：要重启才生效，界面不替用户重启 */
  instancesToRestart: number[];
}

export interface SettingsView {
  watchdogEnabled: boolean;
  readOnly: { key: string; value: Cell }[];
  diagnostics: Issue[];
}

/**
 * 界面偏好，存在 <home>/desktop/desktop.toml，用户也可以手改。lang 与 mascot 由页面解释，不认识的回落到默认
 */
export interface Preferences {
  lang: string;
  mode: 'system' | 'light' | 'dark';
  palette: 'soft' | 'standard' | 'vivid';
  mica: boolean;
  mascot: string;
  /** builtin，或已装主题包的 id；那个主题包不在了，页面就用内置主题 */
  theme: string;
}

/**
 * desktop.toml 读不了或某个键写错了的地方。syntax 带行与列；value 带出错的键（那个键用默认值）；
 * too-large、io 说明整个文件这次没读成
 */
export interface TomlIssue {
  kind: 'syntax' | 'too-large' | 'io' | 'value';
  line: number | null;
  column: number | null;
  key: string | null;
}

/** 置顶时抓下来的摘要：原记录被淘汰以后它还在，但不再更新 */
export interface Bookmark {
  ref: Ref;
  startedAt: Time;
  method: string;
  path: string;
  model: string | null;
  outcome: Outcome;
  attempts: number;
}

export interface DesktopState {
  /**
   * desktop.toml 这次没读成（语法错、太大、读不了）时为 null：页面保留手上已有的值，
   * 免得用户改到一半的文件把窗口打回默认
   */
  preferences: Preferences | null;
  /** 启用的扩展，按加载的先后；与 preferences 同时为 null */
  enabledExtensions: string[] | null;
  /** 新置顶的在前 */
  bookmarks: Bookmark[];
  /** 这次读的时候因为损坏被重置成默认的文件：要告诉用户（desktop.toml 从不被重置，见 desktopToml） */
  reset: 'bookmarks'[];
  /** desktop.toml 里的问题；没有问题是 [] */
  desktopToml: TomlIssue[];
}

/** 开机自启：每次都是现查系统里的登记 */
export interface Registration {
  enabled: boolean;
  issue: 'binary-missing' | 'entry-mismatch' | null;
}

export interface About {
  /** 正在运行的这个窗口的版本；安装更新之后、重新打开窗口之前，它与已安装的版本不同 */
  version: string;
  variant: 'desktop' | 'headless';
  home: string;
}

// ---------------------------------------------------------------------------
// 更新
// ---------------------------------------------------------------------------

export interface UpdateInfo {
  channel: 'stable' | 'pre';
  current: string;
  latest: string | null;
  updateAvailable: boolean;
  variant: 'desktop' | 'headless';
  publishedAt: Time | null;
  checkedAt: Time;
  /** 安装时带回去；没有可装的版本时为 null */
  check: Token | null;
}

export type InstallPhase =
  | 'marking'
  | 'downloading'
  | 'downloaded'
  | 'broadcasting'
  | 'acked'
  | 'swapping'
  | 'swapped'
  | 'restarting'
  | 'verifying'
  | 'cleaning'
  | 'done'
  | 'failed'
  | 'aborted'
  | 'unknown';

/** 一次安装的当前状态。每次事件给的都是整份，直接替换 */
export interface Job {
  id: Token;
  requestedVersion: string;
  /** 涉及的实例端口；计划出来之前为 null（与「没有实例」不同） */
  targets: number[] | null;
  phase: InstallPhase | null;
  phaseAt: Time | null;
  /** 总量未知时 total 为 null：这时不显示百分比 */
  download: { bytes: number; total: number | null } | null;
  instances: { port: number; outcome: 'restarted' | 'rolled-back' | 'failed' | 'unknown' }[];
  /** 结束之前两个都是 null；结束之后恰好有一个 */
  end: InstallEnd | null;
  fault: Fault | null;
}

export interface InstallEnd {
  outcome: 'installed' | 'up-to-date';
  version: string | null;
}

// ---------------------------------------------------------------------------
// 主题包与扩展包
//
// 只有两种包：主题（颜色、样式表、图片、字体，没有代码）与扩展（在窗口里运行的代码，也可以改外观）。
// 不做沙箱、不做逐项授权：装上之后扩展能做页面能做的一切。保护只在安装之前——Rust 交来这里的事实，
// 页面据此逐条弹窗、每条都要点确认。安装分两步：check 读包、校验、算 sha256，立刻返回；
// verify 另去 GitHub 查仓库与 release，慢、可能连不上，页面先弹第一个提醒，同时等它。
// manifest 里的文字都是包作者写的，Rust 已去掉控制字符与改变书写方向的字符，仍只能当文本显示。
// ---------------------------------------------------------------------------

export type PackageKind = 'theme' | 'extension';

/** 一段给人看的文字：一个字符串，或按语言标签（en、zh-CN…）给的几份 */
export type Text = string | Record<string, string>;

/** 扩展自己声明会改的东西；只是说明，不做限制 */
export type WriteKind = 'config' | 'instances' | 'updates' | 'formats';

export interface PackageManifest {
  schema: 1;
  kind: PackageKind;
  /** 小写字母、数字、`.`、`-`，3–100 个字符；也是安装目录名 */
  id: string;
  name: Text;
  version: string;
  author: string;
  description: Text;
  /** 开源仓库地址；null 即作者声明闭源 */
  repository: string | null;
  license: string | null;
  /** 配对的 format 程序名（hello 回复里的 name） */
  formats: string[];
  /** 自己声明会连接的主机；只是说明 */
  network: string[];
  writes: WriteKind[];
}

/** 包从哪来：一个文件（有 sha256，能与 release 比对），或一个解开的目录（没有可比对的文件） */
export type PackageSource =
  | { kind: 'file'; name: string; sha256: string; bytes: number }
  | { kind: 'directory'; name: string };

/** 读包、校验之后的结果；token 交给 verify、install 或 discard，单次有效，30 分钟后过期 */
export interface PackageCheck {
  token: Token;
  manifest: PackageManifest;
  /** 按路径排序 */
  files: { path: string; bytes: number }[];
  totalBytes: number;
  source: PackageSource;
  /** 同一个 id 已经装着（同一种类）：这次是升级、重装或降级 */
  installed: { version: string; enabled: boolean } | null;
  /** 主题包的 theme.json，由页面按 ThemeSpec 校验 */
  theme: Record<string, unknown> | null;
}

export type Unreachable = 'network' | 'timeout' | 'rate-limited' | 'http';

/**
 * GitHub 上查到的事实，只陈述、不下结论；阈值（例如 star 少）由页面定。
 * 只有 release 的 digest 对上了，仓库的这些数字才说明这个包：没对上时，谁都可以在 manifest 里写一个热门仓库
 */
export type GithubFacts =
  | { state: 'no-repository' }
  | { state: 'not-github'; url: string }
  | { state: 'repo-not-found' }
  | { state: 'unreachable'; reason: Unreachable; status: number | null }
  | { state: 'checked'; repo: RepoFacts; release: ReleaseFacts };

export interface RepoFacts {
  fullName: string;
  url: string;
  stars: number;
  forks: number;
  createdAt: Time;
  pushedAt: Time;
  archived: boolean;
  isFork: boolean;
  license: string | null;
  /** 仓库改过名或换了主人：GitHub 把 manifest 里的地址转到了别处 */
  renamed: boolean;
}

/**
 * 安装文件与 tag v<version>（或 <version>）的 release 比对的结果：
 *   match 有一个 asset 的 digest 等于文件的 sha256；mismatch 有同名的 asset 但 digest 不同；
 *   no-asset 哪个 asset 都对不上；no-digest 这个 release 的 asset 没有 digest（早期上传的）；
 *   no-release 两个 tag 都没有 release；not-a-file 从目录安装，没有可比对的文件
 */
export type ReleaseFacts =
  | { state: 'match'; tag: string; asset: string; url: string }
  | { state: 'mismatch'; tag: string; asset: string }
  | { state: 'no-asset'; tag: string }
  | { state: 'no-digest'; tag: string }
  | { state: 'no-release' }
  | { state: 'not-a-file' }
  | { state: 'unreachable'; reason: Unreachable; status: number | null };

/**
 * 已装的包为什么用不了：manifest 读不了或不合格、入口文件不见了或坏了（多半是被手改过）、读文件出错。
 * 页面按它给出修复的说法；有它的包不能启用、不能选作主题，只能卸载或重装
 */
export interface EntryIssue {
  reason: 'manifest' | 'entry' | 'io';
}

export interface PackageEntry {
  id: string;
  kind: PackageKind;
  /** 装着的 manifest 不再合格时为 null（看 issue） */
  manifest: PackageManifest | null;
  /** 扩展是否启用；主题总是 false（主题由偏好的 theme 选中） */
  enabled: boolean;
  /** 以下三项来自安装记录；手工拷进来的包没有记录，三项都是 null：它没有经过安装前的检查 */
  installedAt: Time | null;
  source: PackageSource | null;
  github: GithubFacts | null;
  issue: EntryIssue | null;
  /** 扩展的数据目录有多大（卸载时问要不要一起删） */
  dataBytes: number;
  /** 主题包的 theme.json */
  theme: Record<string, unknown> | null;
}

/** 扩展数据目录里的一个文件 */
export interface DataFile {
  name: string;
  bytes: number;
  modifiedAt: Time;
}

// ---------------------------------------------------------------------------
// 窗口外壳
// ---------------------------------------------------------------------------

export interface ShellInfo {
  customTitlebar: boolean;
  /** macOS 的 Overlay 标题栏：页面画整条栏，左上角让给系统的交通灯 */
  trafficLights: boolean;
  mica: 'available' | 'unsupported' | 'transparency-off';
  windowsBuild: number | null;
}

export interface Insets {
  top: number;
  height: number;
  right: number;
}

// ---------------------------------------------------------------------------
// 事件：每个窗口一条通道
// ---------------------------------------------------------------------------

/**
 * 事件都是「某块数据旧了」的通知，不带数据本身；页面收到后重新读，读完再 ack。
 * rescued 与 update-progress 例外：前者是一次性的庆祝，后者带着安装的整份状态。
 */
export type DesktopEvent =
  | { type: 'resync' }
  | { type: 'instances-changed' }
  | { type: 'requests-changed'; run: Run }
  | { type: 'rescued'; ref: Ref; at: Time }
  | { type: 'update-progress'; job: Job }
  | { type: 'desktop-state-changed' }
  | { type: 'settings-changed' };

export interface Frame {
  session: Session;
  /** 从 1 起连续编号；出现缺口就重新 attach */
  seq: number;
  event: DesktopEvent;
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

type F<C extends string, D = None> = C extends unknown ? { code: C; details: D } : never;

/**
 * 包与扩展数据的操作为什么被拒（desktop_package 的 details）。reason 是稳定的类别，界面按它选句子；其余几项只在相关时有值：
 * field 是 manifest 里出错的字段，path 是包里的相对路径或扩展数据的文件名（清理过、至多 200 个字符，从不是本机的绝对路径），
 * limit 与 max 是超出的哪一项上限与它的值（files、extension-files 是个数，其余是字节数）。
 * 比本版本新的 Rust 可能给出这里没列出的 reason 或 limit，界面落到通用的说法上
 */
export interface PackageIssue {
  reason: PackageReason;
  field: string | null;
  path: string | null;
  limit: PackageLimit | null;
  max: number | null;
}

export type PackageReason =
  // zip 本身：不是合格的 zip、加了密码、压缩方式不支持、成员读不出来、实际大小与声明不符
  | 'archive'
  | 'encrypted'
  | 'compression'
  | 'content'
  | 'size-mismatch'
  // 包里的内容：不安全的路径、重复或只差大小写、链接、类型不对、这种包不允许的文件、缺少必需的文件、不是文本、不是 JSON 对象
  | 'path'
  | 'collision'
  | 'link'
  | 'file-type'
  | 'unexpected-file'
  | 'missing'
  | 'text'
  | 'json'
  // manifest：某个字段缺失或不合格（看 field）、包格式比本版本新
  | 'field'
  | 'unsupported-schema'
  // 安装、启用与扩展数据：换种类、不在了、不是扩展、坏包不能启用、安装句柄失效、检查后变了、存储坏了、读写出错、出错后没能恢复
  | 'kind-change'
  | 'not-installed'
  | 'not-extension'
  | 'manifest'
  | 'entry'
  | 'token'
  | 'changed'
  | 'corrupt-storage'
  | 'io'
  | 'rollback'
  // 超出上限（看 limit 与 max）
  | 'limit';

export type PackageLimit =
  | 'files'
  | 'extension-files'
  | 'zip-bytes'
  | 'file-bytes'
  | 'total-bytes'
  | 'json-bytes'
  | 'settings-bytes'
  | 'css-bytes'
  | 'storage-bytes'
  | 'extension-bytes'
  | 'records-bytes';

/** 窗口自己的私有文件：读写出错（desktop_state_io）或损坏后被重置（desktop_state_reset）时说是哪一个 */
export type StateFile = 'preferences' | 'packages' | 'bookmarks';

type InstallSimple =
  | 'install_in_progress'
  | 'install_version_not_found'
  | 'install_below_floor'
  | 'install_download_failed'
  | 'install_checksum_mismatch'
  | 'install_probe_failed'
  | 'install_swap_failed'
  | 'install_rolled_back'
  | 'install_failed'
  | 'install_unmanaged_instances';

/** 命令失败时 Promise 以这个对象拒绝：一个稳定的码加上固定的几个细节，文字由界面按码来写 */
export type Fault =
  | F<
      | 'desktop_bad_input'
      | 'desktop_unavailable'
      | 'desktop_window'
      | 'desktop_cursor'
      | 'desktop_changed'
      | 'desktop_history'
      | 'desktop_gone'
      | 'desktop_cancelled'
      | 'desktop_busy'
      | 'desktop_target'
      | 'desktop_external_acl'
      | 'desktop_conflict'
      | 'desktop_binary'
      | 'desktop_install_protocol'
      | InstallSimple
    >
  | F<
      'desktop_unresponsive' | 'desktop_force_refused' | 'desktop_no_spec' | 'desktop_port' | 'install_restart_failed',
      { port: number }
    >
  | F<'desktop_config', { diagnostics: Issue[] }>
  | F<'desktop_start', { reason: 'spawn' | 'not-ready'; oldStopped: boolean }>
  | F<'desktop_stop', { port: number; reason: 'still-running' | 'unverified' | 'permission' | 'failed' }>
  | F<'desktop_state_io' | 'desktop_state_reset', { file: StateFile }>
  | F<'desktop_limit', { resource: 'bookmark-count' | 'bookmark-bytes'; max: number }>
  /** desktop.toml 这时写不了（语法错、太大、读不了）：文件原样不动，等用户改好。details 就是那条问题本身 */
  | F<'desktop_toml', TomlIssue>
  /** 包与扩展数据：装不了、用不了、超出上限 */
  | F<'desktop_package', PackageIssue>
  /** format 条目的列出、重启与 control 调用在宿主这边没办成（不是 format 程序自己拒绝） */
  | F<'desktop_format', { reason: FormatFaultReason }>
  /**
   * format 程序收到了 control 调用并明确拒绝。code 与 reason 都由 format 的作者定（已打码），不是宿主的词表：
   * 照原样显示给人看，不要拿 code 去查翻译
   */
  | F<'desktop_format_fail', { code: string; reason: string }>
  | F<
      'desktop_registration',
      {
        reason:
          | 'unsupported-path'
          | 'permission'
          | 'binary-missing'
          | 'entry-mismatch'
          | 'system-unavailable'
          | 'verification-failed';
      }
    >
  | F<'install_foreground_instances', { ports: number[] }>
  | F<'install_precheck_failed', { port: number; diagnostics: Issue[] }>;

export type FaultCode = Fault['code'];

/**
 * desktop_format 的原因：no-control（条目没提供 control，或它的 control 进程已退役）、busy（排队的调用满了）、
 * timeout（五秒内没办完，含排队、启动与握手）、protocol（control 进程答非所问或中途退出）、
 * too-large（参数或答复超过上限）、spawn（control 进程起不来）、unknown-format（实例里没有这个条目）、
 * unsupported（实例的版本太旧，不认这个操作）
 */
export type FormatFaultReason =
  | 'no-control'
  | 'busy'
  | 'timeout'
  | 'protocol'
  | 'too-large'
  | 'spawn'
  | 'unknown-format'
  | 'unsupported';

/**
 * 一个运行中实例里的一个 [[format]] 条目。name 是配置里的条目名，用它选条目；program 是握手时程序报的名字，
 * 两个条目可以跑同一个程序，所以不能拿 program 当唯一的键
 */
export interface FormatView {
  name: string;
  /** 握手之前、或旧版本实例不报时为 null */
  program: string | null;
  version: string | null;
  process: 'spawn' | 'persistent';
  stages: string[];
  /** 必需的条目握手失败时，请求会失败；可选的会被跳过 */
  required: boolean;
  /** 请求这一路的熔断器；control 进程不影响它 */
  breaker: 'closed' | 'open' | 'probing';
  /** 程序在握手时是否提供了 control */
  control: boolean;
  /** 最近一次成功的单独重启（握手失败的不算，那次什么都没替换）；实例启动不算，从没单独重启过为 null */
  restartedAt: Time | null;
}

/** 单独重启一个条目的结果：每个选中的条目各一条，按配置里的顺序 */
export type RestartOutcome =
  | { name: string; outcome: 'restarted'; program: string | null; version: string | null }
  /**
   * 新进程没通过握手，什么都没替换：这个条目仍按原样运行。reason 是 format 故障的类别
   * （spawn、io、died、timeout、protocol、invalid_output 等），不是给人看的句子
   */
  | { name: string; outcome: 'handshake-failed'; reason: string };

/** 请求失败或重试的原因。不认识的码一律是 unknown */
export type HistoryCode =
  | 'unknown'
  | 'inbound_host_rejected'
  | 'inbound_origin_rejected'
  | 'request_too_large'
  | 'client_body_aborted'
  | 'upgrade_unsupported'
  | 'local_storage_failed'
  | 'response_too_large'
  | 'retry_exhausted'
  | 'retry_stopped'
  | 'format_rejected'
  | 'format_unavailable'
  | 'format_failed'
  | 'upstream_failed'
  | 'decode_failed'
  | 'shutting_down'
  | 'internal'
  | 'upstream_connect_failed'
  | 'upstream_connect_timeout'
  | 'upstream_first_byte_timeout'
  | 'upstream_read_idle_timeout'
  | 'upstream_body_interrupted'
  | 'upstream_status'
  | 'upstream_error_body'
  | 'format_error'
  | 'format_fail'
  | 'format_decision';

/** 配置与设置的诊断码 */
export type DiagCode =
  | 'unknown'
  | 'config_syntax'
  | 'config_type'
  | 'config_invalid_value'
  | 'config_missing_key'
  | 'config_unknown_key'
  | 'config_not_supported'
  | 'config_conflict'
  | 'secret_placeholder_unmatched'
  | 'default_credential_withheld'
  | 'default_header_shared'
  | 'forward_default_unguarded'
  | 'forward_response_buffered'
  | 'settings_schema_too_new'
  | 'format_handshake_failed'
  | 'format_stage_unsupported'
  | 'file_permissions_too_open'
  | 'listen_not_loopback'
  | 'port_shared'
  | 'config_dir_missing'
  | 'config_file_unreadable'
  | 'watchdog_slow_hang_detection'
  | 'watchdog_restarts_disabled'
  | 'install_restore_unreadable'
  | 'install_config_unreadable';

// ---------------------------------------------------------------------------
// 命令表：每项是 [参数, 结果]
// ---------------------------------------------------------------------------

export interface Commands {
  desktop_info: [None, ShellInfo];
  titlebar_insets: [None, Insets];
  set_caption_color: [{ color: string | null }, null];
  attach: [{ channel: Channel<Frame> }, Session];
  detach: [{ session: Session }, null];
  ack: [{ session: Session; seq: number }, null];
  snapshot: [None, Snapshot];
  query_logs: [{ run: Run | null; cursor: Token | null; limit: number }, LogPage];
  request_detail: [Selection & { ref: Ref }, Detail];
  body_chunk: [Selection & { ref: Ref; part: BodyPart; offset: number; limit: number }, Chunk];
  cancel_detail: [Selection, null];
  rescan: [None, null];
  list_configs: [None, Choice[]];
  start_instance: [{ target: StartTarget }, Run];
  stop_instance: [{ run: Run }, null];
  restart_instance: [{ run: Run }, Run];
  force_stop_instance: [{ run: Run }, null];
  restore_instance: [{ port: number; expectedRestore: Token }, Run];
  forget_restore: [{ port: number; expectedRestore: Token }, null];
  read_config: [{ target: ConfigTarget }, ConfigView];
  validate_config: [{ target: ConfigTarget; changes: Edit[] }, Validation];
  write_config: [{ target: ConfigTarget; changes: Edit[]; acknowledgeExternalAcl: boolean }, Saved];
  read_settings: [None, SettingsView];
  set_watchdog: [{ enabled: boolean }, boolean];
  read_desktop_state: [None, DesktopState];
  set_preferences: [{ changes: Partial<Preferences> }, Preferences];
  set_pinned: [{ ref: Ref; pinned: boolean }, Bookmark[]];
  read_autostart: [None, Registration];
  set_autostart: [{ enabled: boolean }, Registration];
  check_update: [{ channel: 'auto' | 'pre' }, UpdateInfo];
  install_update: [{ check: Token }, InstallEnd];
  about: [None, About];

  package_list: [None, PackageEntry[]];
  /** 用户在文件选择框里选的包：bytes 是文件内容（传输层把它当请求体发，name 放进请求头） */
  package_check_file: [{ name: string; bytes: Uint8Array }, PackageCheck];
  /** 拖进窗口的文件或目录 */
  package_check_path: [{ path: string }, PackageCheck];
  package_verify: [{ token: Token }, GithubFacts];
  /** enable 只对第一次装的扩展有用；升级保留原来的启用状态、设置与数据 */
  package_install: [{ token: Token; enable: boolean }, PackageEntry];
  /** 用户取消安装：删掉暂存的包。token 不认识或过期了也不算错 */
  package_discard: [{ token: Token }, null];
  /** 只对扩展；主题用 set_preferences 的 theme 选 */
  package_set_enabled: [{ id: string; enabled: boolean }, PackageEntry];
  package_uninstall: [{ id: string; removeData: boolean }, null];

  /** 扩展自己的设置：desktop.toml 里 [extensions.settings."<id>"] 这张表 */
  extension_settings_read: [{ id: string }, Record<string, unknown>];
  /** changes 里值为 null 的键被删掉；返回改完的整张表 */
  extension_settings_write: [{ id: string; changes: Record<string, unknown> }, Record<string, unknown>];
  /** 扩展的键值数据：<home>/desktop/extension-data/<id>/storage.json */
  extension_storage_read: [{ id: string }, Record<string, unknown>];
  /** reset 为 true 时从空对象开始改（存储坏了也能这样恢复） */
  extension_storage_write: [{ id: string; changes: Record<string, unknown>; reset: boolean }, Record<string, unknown>];
  /** 扩展的文件：<home>/desktop/extension-data/<id>/files/，名字不带路径 */
  extension_files_list: [{ id: string }, DataFile[]];
  extension_file_read: [{ id: string; name: string }, Uint8Array];
  extension_file_write: [{ id: string; name: string; bytes: Uint8Array }, DataFile];
  /** 文件本来就不在也不算错 */
  extension_file_delete: [{ id: string; name: string }, null];

  /** 这次运行的 format 条目，按配置里的顺序；没有条目时是空数组 */
  format_list: [{ run: Run }, FormatView[]];
  /**
   * 不换实例、只重启选中的 format 条目：在途的请求与流式响应在旧进程上照常收尾，config.toml 不重读。
   * names 为 null 是全部条目；列出的名字区分大小写，有一个不认识就整个拒绝、什么都不改
   */
  format_restart: [{ run: Run; names: string[] | null }, RestartOutcome[]];
  /**
   * 调用条目的 control 方法（方法与参数的含义由 format 的作者定）。兑现的是程序给的结果本身（可能是 null）；
   * 程序明确拒绝时以 desktop_format_fail 拒绝。method 只能是 [A-Za-z0-9._-]，1–64 个字符；params 序列化后不超过 64 KiB
   */
  format_call: [{ run: Run; name: string; method: string; params: unknown }, unknown];
}

export type CommandName = keyof Commands;

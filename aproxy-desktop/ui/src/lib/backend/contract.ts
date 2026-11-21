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

/** 界面偏好，存在 <home>/desktop/ 下。lang 与 mascot 由页面解释，不认识的回落到默认 */
export interface Preferences {
  lang: string;
  mode: 'system' | 'light' | 'dark';
  palette: 'soft' | 'standard' | 'vivid';
  mica: boolean;
  mascot: string;
  /** 主题字符串（aproxy-theme-v1:…）；null 是内置主题 */
  theme: string | null;
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
  preferences: Preferences;
  /** 新置顶的在前 */
  bookmarks: Bookmark[];
  /** 这次读的时候因为损坏被重置成默认的文件：要告诉用户 */
  reset: ('preferences' | 'bookmarks')[];
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
// 窗口外壳
// ---------------------------------------------------------------------------

export interface ShellInfo {
  customTitlebar: boolean;
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
  | F<'desktop_state_io' | 'desktop_state_reset', { file: 'preferences' | 'bookmarks' }>
  | F<
      'desktop_limit',
      { resource: 'bookmark-count' | 'bookmark-bytes' | 'preference-bytes' | 'theme-bytes'; max: number }
    >
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
}

export type CommandName = keyof Commands;

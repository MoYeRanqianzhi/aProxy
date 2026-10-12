/*
 * 界面用到的数据形状：Backend 接口（lib/backend）交给界面的就是这些。
 *
 * 原型里由 mock 后端生成假数据；正式接入后由 Tauri IPC 实现从实例的状态与日志流映射过来。
 * 字段按「界面要显示什么」来定，不要求与线上格式一一对应——映射是各个 Backend 实现自己的事。
 * 密钥永远是打码后的样子：IPC 不返回明文，界面也没有地方能拿到明文。
 */

export type InstanceStatus = 'active' | 'idle' | 'unresponsive' | 'stopped' | 'restarting';

/** 重试或失败的原因。界面文案按 key 查字典，日志原文另存在尝试的 detail 里 */
export type Reason =
  | 'overloaded' // 上游 529
  | 'unavailable' // 上游 503
  | 'badGateway' // 上游 502
  | 'rateLimited' // 上游 429
  | 'reset' // 连接被重置
  | 'readTimeout' // 读超时
  | 'errorBody' // 200 但响应体是错误 JSON
  | 'notFound' // 404，命中受限重试路径后透传
  | 'clientGone' // 客户端断开，aProxy 中止上游请求
  | 'forwardedError' // 仅转发模式：上游错误原样透传，不重试
  | 'stopped'; // 实例被停止或重启，在途请求随之中断

export interface TransformInfo {
  program: string;
  mode: 'persistent' | 'spawn';
  /** 多 key 轮换时的 key 数量 */
  keys: number;
  /** 多渠道路由：模型名前缀 → 渠道 */
  channels: { match: string; channel: string }[];
}

export interface Instance {
  port: number;
  alias: string | null;
  pid: number | null;
  version: string;
  status: InstanceStatus;
  forwardOnly: boolean;
  /** 已按 aProxy 的规则打码（userinfo 与查询串的值替换为 ***） */
  baseUrl: string;
  listen: string;
  configPath: string;
  proxy: string | null;
  startedAt: number;
  lastRequestAt: number | null;
  req1h: number;
  retry1h: number;
  lastError: { reason: Reason; status: number | null; at: number } | null;
  /** 近一小时每 2 分钟一桶，共 30 桶 */
  spark: { req: number[]; retry: number[] };
  transform: TransformInfo | null;
  keepalive: { interval: number; trigger: 'any' | 'accept' | 'body_stream' };
  backoffCap: number;
  boundedPaths: string[];
  diskCache: boolean;
  /** 无响应时：控制通道从什么时候开始沉默。界面上的数据都停在这一刻的快照 */
  silentSince: number | null;
}

export interface Attempt {
  n: number;
  startedAt: number;
  durationMs: number;
  /** null 表示没拿到响应（网络错误、超时） */
  status: number | null;
  /** null 表示这次成功 */
  reason: Reason | null;
  /** 这次失败后等多久再试；最后一次为 0 */
  backoffMs: number;
  respBytes: number;
}

export type Outcome =
  | 'ok' // 一次成功
  | 'rescued' // 失败过、最终成功（「救回」）
  | 'retrying' // 还在重试
  | 'streaming' // 正在接收上游响应
  | 'failed'; // 以失败结束（透传、客户端断开等）

export interface LogEntry {
  key: string;
  /** 实例内第 N 个请求：aProxy 的 request_id 就是这个序号 */
  seq: number;
  port: number;
  time: number;
  method: 'POST' | 'GET';
  path: string;
  model: string | null;
  stream: boolean;
  client: string;
  outcome: Outcome;
  attempts: Attempt[];
  /** 重试中：当前处于退避等待还是正在尝试 */
  phase: 'waiting' | 'attempting' | null;
  phaseStartedAt: number | null;
  phaseEndsAt: number | null;
  durationMs: number;
  reqBytes: number;
  respBytes: number;
  heartbeats: number;
  transformed: boolean;
  keyIndex: number | null;
  pinned: boolean;
  /** 刚到达的条目，用来播放滑入与高亮。界面自己打的标记，后端不填 */
  fresh: boolean;
}

export interface WatchdogInfo {
  pid: number;
  lastScanAt: number;
  respawns: { port: number; at: number; kind: 'crash' | 'hang' }[];
}

// ---------------------------------------------------------------------------
// 单条请求的详情：只在点开某条日志时按需取（请求体可能很大，也是最敏感的内容）
// ---------------------------------------------------------------------------

export type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

export interface HeaderLine {
  name: string;
  value: string;
  /** 已打码的密钥 */
  secret?: boolean;
}

export interface SseEvent {
  event: string;
  data: string;
}

/** 请求转换（format 程序）改了什么：改写前后各一行 */
export interface TransformChange {
  field: string;
  before: string;
  after: string;
  note?: 'key' | 'channel' | 'model';
}

/** 一次尝试拿到的东西。四种情况互斥：没拿到响应 / 上游错误体 / 成功的 SSE 事件 / 成功的普通响应体 */
export interface AttemptDetail {
  /** 响应头；没拿到响应时为空 */
  headers: HeaderLine[];
  /** 没拿到响应时的传输层错误原文 */
  transportError: string | null;
  /** 上游返回的错误体（JSON 或纯文本） */
  errorBody: Json | string | null;
  /** 成功的流式响应：逐条 SSE 事件 */
  sse: SseEvent[] | null;
  /** 成功的非流式响应体 */
  body: Json | null;
  /** 这次尝试的原始往来（请求头 + 截断的请求体 + 响应），给「原始」页签 */
  raw: string;
}

export interface RequestDetail {
  key: string;
  request: { headers: HeaderLine[]; body: Json | null; curl: string; raw: string };
  /** 与条目的 attempts 一一对应 */
  attempts: AttemptDetail[];
  transform: TransformChange[];
}

// ---------------------------------------------------------------------------
// 配置、设置、更新
// ---------------------------------------------------------------------------

export interface RequestTransformConfig {
  command: string;
  args: string;
  mode: 'persistent' | 'spawn';
  extra: string;
  every_attempt: boolean;
}

/** 实例配置里界面能改的那些键，名字与 config.toml 一致 */
export interface ConfigValues {
  base_url: string;
  listen_addr: string;
  proxy: string;
  connect_timeout_secs: number;
  read_timeout_secs: number;
  max_retry_backoff_secs: number;
  bounded_retry_paths: string[];
  forward_only: boolean;
  spool_limit_mb: number;
  max_body_mb: number;
  disk_cache: boolean;
  keepalive_interval_secs: number;
  keepalive_trigger: 'any' | 'accept' | 'body_stream';
  keepalive_heartbeat: string;
  request_transform: RequestTransformConfig | null;
  allowed_hosts: string[];
  allowed_origins: string[];
}

export type ConfigKey = keyof ConfigValues;

/** 实例保存前检查出的问题：格式之外界面判断不了的那些（端口被占、上游指回 aProxy 自己…） */
export interface ConfigIssue {
  key: ConfigKey;
  /** 实例给出的说明，至少有 en */
  message: LocalizedText;
}

export interface InstanceConfig {
  port: number;
  path: string;
  values: ConfigValues;
  /** 密钥只写不读：已设置时只给打码后的样子，未设置为 null */
  apiKeyMasked: string | null;
  /** 改了立即生效的键；其余要重启实例。以实例的实现为准，所以由后端告诉界面 */
  live: ConfigKey[];
}

// ---------------------------------------------------------------------------
// 设置与主题
// ---------------------------------------------------------------------------

/**
 * 主题里的一组颜色，对应浅色或深色变体。每项都可以省略，省略的沿用内置主题。
 * 颜色写作 #rgb、#rrggbb 或 oklch(L C H)。作者给的颜色只决定色调（色相与鲜艳程度），
 * 深浅由界面按角色的固定台阶取，保证任何主题下文字对比度都和内置主题一样（见 lib/theme.ts）。
 */
export interface ThemeColors {
  /** 主色：品牌、选中、主要按钮 */
  primary?: string;
  /** 文字：正文、次要文字、彩色块上的字 */
  text?: string;
  /** 表面：窗口底、纸面、卡片、细边 */
  surface?: string;
  /** 状态语义色：成功 / 重试中 / 失败 / 提醒（仅转发、需重启） */
  ok?: string;
  retry?: string;
  error?: string;
  warn?: string;
}

/** 主题字符串 aproxy-theme-v1:{...} 里的 JSON。除 name 外都可省略 */
export interface ThemeSpec {
  name: string;
  author?: string;
  /** more：次要文字更深、细边更明显 */
  contrast?: 'standard' | 'more';
  /** 圆角倍数：0 是直角，1 是内置，最大 1.5。胶囊形的按钮与标签始终是胶囊 */
  radius?: number;
  /** 只能从界面自带的字体里选：运行时不联网，主题也不该去加载远程字体 */
  font?: 'rounded' | 'system' | 'mono';
  /** 动效强度：位移与回弹幅度的倍数。0 等于减少动效（只留淡入淡出），最大 1.5 */
  motion?: number;
  /** 浅色与深色两个变体；只给一个时，另一种模式沿用内置配色 */
  light?: ThemeColors;
  dark?: ThemeColors;
}

/** 用户偏好：持久化在 settings.json 的 ui 段（随 W2 定），由后端读写 */
export interface UiPrefs {
  lang: 'system' | 'zh-CN' | 'en';
  /** 浅色 / 深色 / 跟随系统 */
  mode: 'system' | 'light' | 'dark';
  palette: 'soft' | 'standard' | 'vivid';
  mica: boolean;
  mascot: string;
  /** 导入的主题；null 是内置主题。存解析后的对象而不是字符串，settings.json 里可读可改 */
  theme: ThemeSpec | null;
}

export interface AppSettings {
  watchdog: boolean;
  autostart: boolean;
  ui: UiPrefs;
}

/** 多语言的一句话（发布说明这类后端给的文字）：至少有 en */
export type LocalizedText = { en: string; [lang: string]: string };

export interface UpdateInfo {
  channel: 'stable' | 'pre';
  current: string;
  latest: string;
  /** 带界面 / 无界面：升级保持原变体 */
  variant: 'desktop' | 'headless';
  publishedAt: number;
  checkedAt: number;
  notes: { kind: 'feature' | 'fix'; text: LocalizedText }[];
}

export type UpdateStage = 'download' | 'verify' | 'notify' | 'swap' | 'restart' | 'done';

export interface UpdateProgress {
  version: string;
  stage: UpdateStage;
  /** 当前阶段的进度 0–1 */
  progress: number;
  /** 逐个重启阶段：已经换到新版本的实例端口 */
  restarted: number[];
  /** 这次安装涉及的实例（开始时在运行的那些） */
  targets: number[];
  downloadedMb: number;
  totalMb: number;
}

// ---------------------------------------------------------------------------
// 扩展
//
// 扩展能读到的就是界面能看到的（界面本身不持有密钥、不经手流量，IPC 也不给明文密钥），
// 所以读不分级。只有「写」——改变实例或配置的操作——要用户同意：扩展第一次调用某类写操作时，
// 后端发出 consent-request，界面弹框问用户；同意后记住，用户可以在扩展详情里撤销。
// 授权由后端保存并在执行写操作时检查，界面只负责展示和转达用户的回答。
// ---------------------------------------------------------------------------

/** 写操作的种类，也是授权的粒度 */
export type WriteCapability = 'control' | 'config' | 'install';

/** 一次写操作具体做什么：确认框与「最近的写操作」都按它出文案 */
export type ExtensionAction =
  | { kind: 'start' | 'stop' | 'restart'; port: number }
  | { kind: 'write-config'; port: number; keys: string[] }
  | { kind: 'install'; version: string };

export interface ExtensionInfo {
  id: string;
  /** 扩展自己在清单里提供的名字与说明，至少有 en */
  name: LocalizedText;
  description: LocalizedText;
  version: string;
  author: string;
  /** 安装位置：给用户看，方便找到文件 */
  origin: string;
  /**
   * extension：在界面里运行，用和界面一样的数据接口。
   * mascot：自定义吉祥物，只拿到状态数据、隔离渲染（隔离是为了稳定，见 lib/mascot/types.ts），
   * 没有写操作，也不需要启用开关——在外观里选中它就是在用它。
   */
  kind: 'extension' | 'mascot';
  enabled: boolean;
  /** 清单里声明会用到的写操作；空数组就是只读 */
  declares: WriteCapability[];
  /** 用户已经同意过的写操作 */
  granted: { capability: WriteCapability; at: number }[];
  /** 最近执行过的写操作，新的在前 */
  recent: { action: ExtensionAction; at: number }[];
}

/** 扩展第一次调用某类写操作时，后端请界面问用户 */
export interface ConsentRequest {
  id: string;
  extension: string;
  capability: WriteCapability;
  action: ExtensionAction;
  at: number;
}

/*
 * 原型时期的数据形状：模拟引擎（lib/backend/mock）按它们生成演示用的实例、请求与详情，演示后端（lib/backend/demo）
 * 再换成契约（lib/backend/contract.ts）交给界面。界面本身只认契约；这里另外定义的只有主题（ThemeSpec）与正文查看器的 Json。
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
  /** 这一次启动的来由（start、restart、restore、login、watchdog、upgrade、plugin、unknown）与备注 */
  launch: { reason: string; note: string | null };
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
// 主题
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

/** 主题包里 theme.json 的内容（校验见 theme.ts）。除 name 外都可省略 */
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
  /**
   * 标题栏至少多高，CSS 像素，36 – 64，内置 44（app.css 的 --titlebar-min）。Windows 原生的窗口按钮不跟着变，
   * 保持系统高度贴在顶部
   */
  titlebarMin?: number;
  /** 浅色与深色两个变体；只给一个时，另一种模式沿用内置配色 */
  light?: ThemeColors;
  dark?: ThemeColors;
}

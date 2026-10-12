/*
 * mock 后端的假数据：实例、请求日志，以及点开一条日志后看到的请求/响应/转换细节。
 * 只有 mock 后端（engine.ts、index.ts）用它；界面组件不认识这个文件。
 *
 * 追求「像真的」：路径、请求头、SSE 事件、错误体都照 Claude Code 与 Codex 的真实流量写，
 * 退避间隔照 aProxy 的规则（前三次重试立即发出，第四次起 5 秒翻倍直到封顶），
 * request_id 就是实例内的请求序号（aProxy 的真实做法），密钥按 aProxy 的打码规则只留前 6 个字符。
 * 用固定种子的随机数生成，每次打开看到的初始数据一致，方便对比审核。
 */
import type {
  Attempt,
  AttemptDetail,
  HeaderLine,
  Instance,
  Json,
  LogEntry,
  Outcome,
  Reason,
  RequestDetail,
  SseEvent,
  TransformChange,
  WatchdogInfo,
} from '../../types';
import { backoffAfter } from '../../retry';

/** 模拟内部用的条目：比交给界面的 LogEntry 多两个只有模拟才需要的字段，发出去之前去掉 */
export interface SimEntry extends LogEntry {
  /** 这条重试会坚持到第几次才成功 */
  succeedAt: number;
  /** 请求体用哪一句提示词 */
  promptIdx: number;
}

/** mulberry32：足够小的可复现随机数 */
export function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

type Rand = () => number;
const pick = <T>(r: Rand, items: readonly T[]): T => items[Math.floor(r() * items.length)];
function weighted<T>(r: Rand, table: readonly (readonly [T, number])[]): T {
  const total = table.reduce((s, [, w]) => s + w, 0);
  let x = r() * total;
  for (const [v, w] of table) {
    x -= w;
    if (x <= 0) return v;
  }
  return table[table.length - 1][0];
}
const between = (r: Rand, lo: number, hi: number) => lo + r() * (hi - lo);

export const MIN = 60_000;

export const CLAUDE_CODE_UA = 'claude-cli/2.1.42 (external, cli)';
export const CODEX_UA = 'codex_cli_rs/0.58.0 (Windows 10.0.26200; x86_64) WindowsTerminal';

export const PROMPTS = [
  '给购物车加上优惠券校验，过期的券要提示用户',
  '把登录表单的校验改成 zod，错误提示放在输入框下面',
  'Why does this regex backtrack so badly on long inputs?',
  '跑一下测试，把失败的几个修掉',
  'Refactor the retry loop to use exponential backoff with jitter',
  '解释一下保活心跳是在什么时候提交响应头的',
  'Write a migration that adds an index on orders.created_at',
  '把 OrderTable 拆成三个更小的组件，保持行为不变',
] as const;

// ---------------------------------------------------------------------------
// 实例
// ---------------------------------------------------------------------------

function spark(r: Rand, base: number, retryRate: number, quietTail = 0) {
  const req: number[] = [];
  const retry: number[] = [];
  for (let i = 0; i < 30; i++) {
    // 中间有一段上游拥堵（重试变多），让走势图有起伏
    const surge = i > 17 && i < 23 ? 1.8 : 1;
    const v = i >= 30 - quietTail ? 0 : Math.max(0, Math.round(base * (0.55 + r() * 0.9)));
    req.push(v);
    retry.push(Math.round(v * retryRate * surge * (0.4 + r() * 1.2)));
  }
  return { req, retry };
}

export function makeInstances(now: number): Instance[] {
  const r = rng(7);
  return [
    {
      port: 12345,
      alias: 'default',
      pid: 14732,
      version: '0.2.0',
      status: 'active',
      forwardOnly: false,
      baseUrl: 'https://api.anthropic.com',
      listen: '127.0.0.1:12345',
      configPath: '~/.aproxy/config.toml',
      proxy: 'http://127.0.0.1:7890',
      startedAt: now - (3 * 24 * 60 + 4 * 60 + 12) * MIN,
      lastRequestAt: now - 4_000,
      req1h: 186,
      retry1h: 9,
      lastError: { reason: 'overloaded', status: 529, at: now - 14 * MIN },
      spark: spark(r, 6, 0.06),
      transform: null,
      keepalive: { interval: 15, trigger: 'any' },
      backoffCap: 320,
      boundedPaths: [],
      diskCache: true,
      silentSince: null,
    },
    {
      port: 12346,
      alias: 'openrouter',
      pid: 18244,
      version: '0.2.0',
      status: 'active',
      forwardOnly: false,
      baseUrl: 'https://openrouter.ai/api',
      listen: '127.0.0.1:12346',
      configPath: '~/.aproxy/openrouter.toml',
      proxy: null,
      startedAt: now - (26 * 60 + 40) * MIN,
      lastRequestAt: now - 2_000,
      req1h: 212,
      retry1h: 31,
      lastError: { reason: 'overloaded', status: 529, at: now - 9_000 },
      spark: spark(r, 7, 0.16),
      transform: {
        program: 'aproxy-format',
        mode: 'persistent',
        keys: 3,
        channels: [
          { match: 'claude-*', channel: 'anthropic' },
          { match: 'gpt-*', channel: 'openai' },
        ],
      },
      keepalive: { interval: 15, trigger: 'any' },
      backoffCap: 120,
      boundedPaths: ['/v1/messages/count_tokens.*'],
      diskCache: true,
      silentSince: null,
    },
    {
      port: 12350,
      alias: 'codex',
      pid: 9120,
      version: '0.2.0',
      status: 'idle',
      forwardOnly: true,
      baseUrl: 'https://relay.example.net/v1?token=***',
      listen: '127.0.0.1:12350',
      configPath: '~/.aproxy/codex.toml',
      proxy: null,
      startedAt: now - (5 * 60 + 2) * MIN,
      lastRequestAt: now - 38 * MIN,
      req1h: 14,
      retry1h: 0,
      lastError: { reason: 'forwardedError', status: 502, at: now - 41 * MIN },
      spark: { req: spark(r, 1.2, 0, 19).req, retry: Array(30).fill(0) },
      transform: null,
      keepalive: { interval: 0, trigger: 'any' },
      backoffCap: 320,
      boundedPaths: [],
      diskCache: false,
      silentSince: null,
    },
    {
      port: 12360,
      alias: 'work',
      pid: null,
      version: '0.2.0',
      status: 'stopped',
      forwardOnly: false,
      baseUrl: 'https://***@gateway.corp.example/anthropic',
      listen: '127.0.0.1:12360',
      configPath: '~/.aproxy/work.toml',
      proxy: null,
      startedAt: now,
      lastRequestAt: now - 2 * 24 * 60 * MIN,
      req1h: 0,
      retry1h: 0,
      lastError: null,
      spark: { req: Array(30).fill(0), retry: Array(30).fill(0) },
      transform: null,
      keepalive: { interval: 15, trigger: 'any' },
      backoffCap: 320,
      boundedPaths: [],
      diskCache: true,
      silentSince: null,
    },
  ];
}

export function makeWatchdog(now: number): WatchdogInfo {
  return {
    pid: 6408,
    lastScanAt: now - 12_000,
    respawns: [{ port: 12346, at: now - (9 * 60 + 47) * MIN, kind: 'hang' }],
  };
}

// ---------------------------------------------------------------------------
// 请求日志
// ---------------------------------------------------------------------------

const STATUS_OF: Record<Reason, number | null> = {
  overloaded: 529,
  unavailable: 503,
  badGateway: 502,
  rateLimited: 429,
  reset: null,
  readTimeout: null,
  errorBody: 200,
  notFound: 404,
  clientGone: null,
  forwardedError: 502,
  stopped: null,
};

function attemptMs(r: Rand, reason: Reason | null, model: string | null, stream: boolean): number {
  switch (reason) {
    case 'readTimeout':
      return 300_000 + r() * 400;
    case 'reset':
      return between(r, 2_000, 9_000);
    case 'overloaded':
    case 'unavailable':
    case 'badGateway':
    case 'rateLimited':
    case 'notFound':
    case 'forwardedError':
      return between(r, 180, 1_400);
    case 'clientGone':
      return between(r, 3_000, 20_000);
    case 'errorBody':
      return between(r, 1_500, 6_000);
    default:
      if (!model) return between(r, 80, 300);
      if (model.includes('haiku')) return between(r, 600, 2_200);
      if (!stream) return between(r, 900, 3_000);
      if (model.includes('opus')) return between(r, 9_000, 52_000);
      return between(r, 4_000, 31_000);
  }
}

interface Shape {
  method: 'POST' | 'GET';
  path: string;
  model: string | null;
  stream: boolean;
  client: string;
}

function shapeFor(r: Rand, port: number): Shape {
  if (port === 12350) {
    if (r() < 0.06) return { method: 'GET', path: '/v1/models', model: null, stream: false, client: CODEX_UA };
    return { method: 'POST', path: '/v1/responses', model: 'gpt-5.5', stream: true, client: CODEX_UA };
  }
  if (port === 12346) {
    if (r() < 0.07)
      return { method: 'POST', path: '/v1/messages/count_tokens?beta=true', model: 'claude-sonnet-5-5', stream: false, client: CLAUDE_CODE_UA };
    if (r() < 0.42) return { method: 'POST', path: '/v1/responses', model: 'gpt-5.5', stream: true, client: CODEX_UA };
    return { method: 'POST', path: '/v1/messages?beta=true', model: 'claude-sonnet-5-5', stream: true, client: CLAUDE_CODE_UA };
  }
  const model = weighted(r, [
    ['claude-opus-5-5', 55],
    ['claude-sonnet-5-5', 30],
    ['claude-haiku-5-5', 15],
  ] as const);
  if (r() < 0.05)
    return { method: 'POST', path: '/v1/messages/count_tokens?beta=true', model, stream: false, client: CLAUDE_CODE_UA };
  return { method: 'POST', path: '/v1/messages?beta=true', model, stream: !model.includes('haiku'), client: CLAUDE_CODE_UA };
}

function rescueReason(r: Rand, port: number): Reason {
  if (port === 12346)
    return weighted(r, [
      ['overloaded', 35],
      ['rateLimited', 30],
      ['badGateway', 20],
      ['reset', 15],
    ] as const);
  return weighted(r, [
    ['overloaded', 60],
    ['unavailable', 15],
    ['reset', 15],
    ['readTimeout', 5],
    ['errorBody', 5],
  ] as const);
}

function reqBytesFor(r: Rand, s: Shape): number {
  if (s.method === 'GET') return 0;
  if (s.model?.includes('haiku')) return Math.round(between(r, 2_800, 9_000));
  if (s.path.includes('count_tokens')) return Math.round(between(r, 30_000, 160_000));
  return Math.round(between(r, 38_000, 420_000));
}

function respBytesFor(r: Rand, s: Shape): number {
  if (s.method === 'GET') return Math.round(between(r, 3_000, 6_000));
  if (s.path.includes('count_tokens')) return 34;
  if (s.model?.includes('haiku')) return Math.round(between(r, 600, 1_800));
  return Math.round(between(r, 2_400, 34_000));
}

interface Seed {
  r: Rand;
  port: number;
  seq: number;
  time: number;
  outcome: Outcome;
  shape?: Shape;
  reasons?: Reason[];
  capSecs: number;
  keyCount: number;
}

/** 按「结果」构造一条日志，尝试序列与耗时都由它推出来，保证时间线自洽 */
function buildEntry(seed: Seed): SimEntry {
  const { r, port, seq, time, outcome } = seed;
  const shape = seed.shape ?? shapeFor(r, port);
  const attempts: Attempt[] = [];
  let t = time + Math.round(between(r, 4, 30)); // 读完请求体后才发第一跳
  const push = (reason: Reason | null, n: number) => {
    const durationMs = Math.round(attemptMs(r, reason, shape.model, shape.stream));
    const status = reason ? STATUS_OF[reason] : 200;
    const backoffMs = reason ? backoffAfter(n, seed.capSecs) : 0;
    attempts.push({
      n,
      startedAt: t,
      durationMs,
      status,
      reason,
      backoffMs,
      respBytes: reason ? (status === null ? 0 : Math.round(between(r, 90, 1_600))) : respBytesFor(r, shape),
    });
    t += durationMs + backoffMs;
  };

  if (outcome === 'ok' || outcome === 'streaming') {
    push(null, 1);
  } else if (outcome === 'rescued') {
    const reasons = seed.reasons ?? [];
    reasons.forEach((reason, i) => push(reason, i + 1));
    push(null, reasons.length + 1);
  } else if (outcome === 'failed' || outcome === 'retrying') {
    (seed.reasons ?? []).forEach((reason, i) => push(reason, i + 1));
  }

  const last = attempts[attempts.length - 1];
  // 失败结束的那一次没有退避
  if (outcome === 'failed' && last) last.backoffMs = 0;
  const end = last ? last.startedAt + last.durationMs : time;
  const finished = outcome === 'ok' || outcome === 'rescued' || outcome === 'failed';
  // 救回的流式请求：从第一次失败到最后一次开始之间，客户端靠心跳续命
  const heartbeatSpan = outcome === 'rescued' ? end - time - last.durationMs : 0;
  return {
    key: `${port}:${seq}`,
    seq,
    port,
    time,
    method: shape.method,
    path: shape.path,
    model: shape.model,
    stream: shape.stream,
    client: shape.client,
    outcome,
    attempts,
    phase: null,
    phaseStartedAt: null,
    phaseEndsAt: null,
    durationMs: Math.max(1, end - time),
    reqBytes: reqBytesFor(r, shape),
    respBytes: finished && last ? last.respBytes : 0,
    // 保活心跳只发给流式请求，每 15 秒一拍；重试期间才有意义
    heartbeats: shape.stream && port !== 12350 ? Math.floor(heartbeatSpan / 15_000) : 0,
    transformed: port === 12346,
    keyIndex: port === 12346 ? (seq % seed.keyCount) + 1 : null,
    pinned: false,
    fresh: false,
    succeedAt: 0,
    promptIdx: seq % PROMPTS.length,
  };
}

function outcomeFor(r: Rand, port: number, shape: Shape): Outcome {
  if (port === 12350) return r() < 0.07 && shape.method === 'POST' ? 'failed' : 'ok';
  if (port === 12346) {
    if (shape.path.includes('count_tokens')) return 'failed';
    return weighted(r, [
      ['ok', 66],
      ['rescued', 30],
      ['failed', 4],
    ] as const);
  }
  return weighted(r, [
    ['ok', 84],
    ['rescued', 13],
    ['failed', 3],
  ] as const);
}

function reasonsFor(r: Rand, port: number, outcome: Outcome, shape: Shape): Reason[] {
  if (outcome === 'rescued') {
    const reason = rescueReason(r, port);
    const n = reason === 'readTimeout' ? 1 : weighted(r, [[1, 30], [2, 25], [3, 20], [4, 15], [5, 7], [6, 3]] as const);
    // 同一段故障里原因大体相同，偶尔夹一次别的
    return Array.from({ length: n }, () => (r() < 0.85 ? reason : rescueReason(r, port)));
  }
  if (outcome === 'failed') {
    if (port === 12350) return ['forwardedError'];
    if (shape.path.includes('count_tokens')) return ['notFound', 'notFound', 'notFound'];
    // 客户端在重试中途放弃（用户按了 Esc），aProxy 随即中止上游请求
    return ['overloaded', 'overloaded', 'clientGone'];
  }
  return [];
}

export function makeLogs(now: number, instances: Instance[]): SimEntry[] {
  const r = rng(42);
  const logs: SimEntry[] = [];
  const plan: { port: number; from: number; to: number; gap: number; seq: number }[] = [
    { port: 12345, from: now - 34 * MIN, to: now - 15_000, gap: 21_000, seq: 18_210 },
    { port: 12346, from: now - 34 * MIN, to: now - 20_000, gap: 19_000, seq: 9_377 },
    { port: 12350, from: now - 64 * MIN, to: now - 38 * MIN, gap: 110_000, seq: 640 },
  ];
  const nextSeq: Record<number, number> = {};
  for (const p of plan) {
    const inst = instances.find((i) => i.port === p.port)!;
    let time = p.from;
    let seq = p.seq;
    while (time < p.to) {
      const shape = shapeFor(r, p.port);
      const outcome = outcomeFor(r, p.port, shape);
      const reasons = reasonsFor(r, p.port, outcome, shape);
      logs.push(
        buildEntry({ r, port: p.port, seq, time, outcome, shape, reasons, capSecs: inst.backoffCap, keyCount: inst.transform?.keys ?? 1 }),
      );
      seq += 1;
      time += Math.round(p.gap * (0.35 + r() * 1.3));
    }
    nextSeq[p.port] = seq;
  }

  // 正在发生的事：两个 openrouter 请求在退避等待，两个 default 请求在接收流
  const or = instances.find((i) => i.port === 12346)!;
  const waiting = [
    {
      shape: { method: 'POST', path: '/v1/messages?beta=true', model: 'claude-sonnet-5-5', stream: true, client: CLAUDE_CODE_UA } as Shape,
      reasons: ['overloaded', 'reset', 'overloaded', 'overloaded', 'overloaded'] as Reason[],
      waitedMs: 2_600,
      // 坚持得久一点：审核时打开任何页面都还能看到「重试中」的样子
      succeedAt: 10,
    },
    {
      shape: { method: 'POST', path: '/v1/responses', model: 'gpt-5.5', stream: true, client: CODEX_UA } as Shape,
      reasons: ['rateLimited', 'rateLimited', 'rateLimited', 'rateLimited'] as Reason[],
      waitedMs: 1_900,
      succeedAt: 9,
    },
  ];
  for (const w of waiting) {
    const e = buildEntry({ r, port: 12346, seq: nextSeq[12346]++, time: 0, outcome: 'retrying', shape: w.shape, reasons: w.reasons, capSecs: or.backoffCap, keyCount: 3 });
    // 把整条时间线平移到「最后一次失败在 waitedMs 之前结束」，尝试、退避与总耗时才互相对得上
    const last = e.attempts[e.attempts.length - 1];
    const shift = now - w.waitedMs - (last.startedAt + last.durationMs);
    e.time += shift;
    for (const a of e.attempts) a.startedAt += shift;
    e.phase = 'waiting';
    e.phaseStartedAt = now - w.waitedMs;
    e.phaseEndsAt = e.phaseStartedAt + last.backoffMs;
    e.succeedAt = w.succeedAt;
    e.durationMs = now - e.time;
    e.heartbeats = Math.floor(e.durationMs / 15_000);
    logs.push(e);
  }

  for (const [ago, model] of [
    [9_000, 'claude-opus-5-5'],
    [2_500, 'claude-sonnet-5-5'],
  ] as const) {
    const e = buildEntry({
      r,
      port: 12345,
      seq: nextSeq[12345]++,
      time: now - ago,
      outcome: 'streaming',
      shape: { method: 'POST', path: '/v1/messages?beta=true', model, stream: true, client: CLAUDE_CODE_UA },
      capSecs: 320,
      keyCount: 1,
    });
    e.phase = 'attempting';
    e.phaseStartedAt = e.attempts[0].startedAt;
    e.phaseEndsAt = now + (model.includes('opus') ? 21_000 : 9_000);
    e.attempts[0].durationMs = 0;
    e.durationMs = ago;
    e.respBytes = 0;
    logs.push(e);
  }

  logs.sort((a, b) => a.time - b.time);
  // 几条预先固定的，展示「固定」的样子
  for (const e of logs) if (e.outcome === 'rescued' && e.attempts.length >= 5) e.pinned = true;
  return logs;
}

/** 模拟器用：生成一条刚到的请求 */
export function makeArrival(r: Rand, port: number, seq: number, now: number, inst: Instance): SimEntry {
  const shape = shapeFor(r, port);
  const willRetry = !shape.path.includes('count_tokens') && r() < (port === 12346 ? 0.3 : 0.12);
  const e = buildEntry({ r, port, seq, time: now, outcome: 'streaming', shape, capSecs: inst.backoffCap, keyCount: inst.transform?.keys ?? 1 });
  e.attempts[0].durationMs = 0;
  e.durationMs = 0;
  e.respBytes = 0;
  e.phase = 'attempting';
  e.phaseStartedAt = now;
  if (willRetry) {
    // 第一次尝试会失败，进入重试；几次之后救回
    e.outcome = 'retrying';
    e.attempts = [];
    e.phaseEndsAt = now + 600;
    e.succeedAt = 2 + Math.floor(r() * 3);
  } else {
    // 真实耗时按比例缩短，演示时不用干等几十秒
    e.phaseEndsAt = now + attemptMs(r, null, shape.model, shape.stream) / 6;
  }
  return e;
}

export { rescueReason, STATUS_OF, attemptMs, respBytesFor };

// ---------------------------------------------------------------------------
// 详情：请求、响应、转换、原始
// ---------------------------------------------------------------------------

const LONG_SYSTEM =
  'You are an interactive CLI tool that helps users with software engineering tasks. Use the instructions below and the tools available to you to assist the user. IMPORTANT: Assist with defensive security tasks only…';

export function requestHeaders(e: LogEntry): HeaderLine[] {
  const host = { name: 'host', value: `127.0.0.1:${e.port}` };
  if (e.client === CODEX_UA)
    return [
      host,
      { name: 'user-agent', value: e.client },
      { name: 'authorization', value: 'Bearer sk-pro***', secret: true },
      { name: 'content-type', value: 'application/json' },
      { name: 'accept', value: e.stream ? 'text/event-stream' : 'application/json' },
      { name: 'openai-beta', value: 'responses=experimental' },
      { name: 'originator', value: 'codex_cli_rs' },
      { name: 'session_id', value: '0199d7b4-5a1e-7c2a-9f3e-6b1d2e8c4a70' },
      { name: 'content-length', value: String(e.reqBytes) },
    ];
  return [
    host,
    { name: 'user-agent', value: e.client },
    { name: 'x-api-key', value: 'sk-ant***', secret: true },
    { name: 'anthropic-version', value: '2023-06-01' },
    { name: 'anthropic-beta', value: 'claude-code-20250219,interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14' },
    { name: 'content-type', value: 'application/json' },
    { name: 'accept', value: 'application/json' },
    { name: 'x-app', value: 'cli' },
    { name: 'x-stainless-lang', value: 'js' },
    { name: 'x-stainless-retry-count', value: '0' },
    { name: 'content-length', value: String(e.reqBytes) },
  ];
}

const TOOL_NAMES = ['Task', 'Bash', 'Glob', 'Grep', 'Read', 'Edit', 'Write', 'WebFetch', 'TodoWrite', 'WebSearch'];

export function requestBody(e: SimEntry): Json {
  if (e.method === 'GET') return null;
  const prompt = PROMPTS[e.promptIdx];
  if (e.client === CODEX_UA)
    return {
      model: e.model,
      instructions: 'You are Codex, based on GPT-5. You are running as a coding agent in the Codex CLI on a user\'s computer.',
      input: [
        { type: 'message', role: 'user', content: [{ type: 'input_text', text: prompt }] },
        { type: 'function_call', name: 'shell', arguments: '{"command":["rg","-n","retry","src"]}', call_id: 'call_Qm3vX8' },
        { type: 'function_call_output', call_id: 'call_Qm3vX8', output: 'src/net/retry.ts:14:export async function withRetry(…' },
      ],
      tools: [
        { type: 'function', name: 'shell', description: 'Runs a shell command and returns its output.', strict: false, parameters: { type: 'object', properties: { command: { type: 'array', items: { type: 'string' } } } } },
        { type: 'function', name: 'apply_patch', description: 'Use this tool to edit files.', strict: false, parameters: { type: 'object' } },
      ],
      tool_choice: 'auto',
      parallel_tool_calls: false,
      reasoning: { effort: 'medium', summary: 'auto' },
      store: false,
      stream: e.stream,
      prompt_cache_key: '0199d7b4-5a1e-7c2a-9f3e-6b1d2e8c4a70',
    };
  if (e.path.includes('count_tokens'))
    return {
      model: e.model,
      messages: [{ role: 'user', content: [{ type: 'text', text: prompt }] }],
      tools: TOOL_NAMES.slice(0, 4).map((name) => ({ name, input_schema: { type: 'object' } })),
    };
  return {
    model: e.model,
    max_tokens: e.model?.includes('haiku') ? 512 : 32000,
    stream: e.stream,
    ...(e.model?.includes('haiku')
      ? {}
      : { thinking: { type: 'enabled', budget_tokens: 31999 } }),
    system: [
      { type: 'text', text: "You are Claude Code, Anthropic's official CLI for Claude.", cache_control: { type: 'ephemeral' } },
      { type: 'text', text: LONG_SYSTEM, cache_control: { type: 'ephemeral' } },
    ],
    messages: [
      { role: 'user', content: [{ type: 'text', text: prompt }] },
      {
        role: 'assistant',
        content: [
          { type: 'text', text: '我先读一下相关的代码。' },
          { type: 'tool_use', id: 'toolu_01HXq8Vb2c', name: 'Read', input: { file_path: 'G:\\work\\shop\\src\\cart\\coupon.ts' } },
        ],
      },
      {
        role: 'user',
        content: [{ type: 'tool_result', tool_use_id: 'toolu_01HXq8Vb2c', content: '     1\texport function applyCoupon(cart: Cart, code: string) {\n     2\t  const coupon = coupons.get(code);\n     3\t  …' }],
      },
    ],
    tools: TOOL_NAMES.map((name) => ({
      name,
      description: `${name} tool`,
      input_schema: { type: 'object', properties: {}, additionalProperties: false },
    })),
    metadata: { user_id: 'user_3f9a2c***' },
  };
}

export function errorBody(reason: Reason, e: LogEntry): { contentType: string; body: Json | string } {
  switch (reason) {
    case 'overloaded':
      return { contentType: 'application/json', body: { type: 'error', error: { type: 'overloaded_error', message: 'Overloaded' }, request_id: `req_011CU${e.seq}xYvK2` } };
    case 'unavailable':
      return { contentType: 'text/plain', body: 'upstream connect error or disconnect/reset before headers. reset reason: connection termination' };
    case 'badGateway':
    case 'forwardedError':
      return {
        contentType: 'text/html; charset=UTF-8',
        body: '<!DOCTYPE html>\n<html>\n<head><title>502 Bad Gateway</title></head>\n<body>\n<center><h1>502 Bad Gateway</h1></center>\n<hr><center>cloudflare</center>\n</body>\n</html>',
      };
    case 'rateLimited':
      return { contentType: 'application/json', body: { error: { message: 'Rate limit exceeded: too many requests for this key, retry shortly', code: 429, metadata: { provider_name: e.model?.startsWith('gpt') ? 'OpenAI' : 'Anthropic' } } } };
    case 'errorBody':
      return { contentType: 'text/event-stream', body: 'event: error\ndata: {"type":"error","error":{"type":"api_error","message":"Internal server error"}}' };
    case 'notFound':
      return { contentType: 'application/json', body: { error: { message: 'Not Found', code: 404 } } };
    default:
      return { contentType: '', body: '' };
  }
}

/** 没有拿到响应的尝试：aProxy 日志里记录的错误原文 */
export function transportError(reason: Reason): string {
  switch (reason) {
    case 'reset':
      return 'error sending request for url (https://api.anthropic.com/v1/messages?beta=true): connection reset (os error 10054)';
    case 'readTimeout':
      return 'read timed out: no data for 300 s (read_timeout_secs)';
    case 'clientGone':
      return 'client disconnected; upstream request aborted';
    default:
      return '';
  }
}

export function responseHeaders(e: LogEntry, a: Attempt): HeaderLine[] {
  if (a.status === null) return [];
  if (a.reason) {
    const { contentType } = errorBody(a.reason, e);
    const lines: HeaderLine[] = [
      { name: 'content-type', value: contentType },
      { name: 'date', value: new Date(a.startedAt + a.durationMs).toUTCString() },
    ];
    if (a.reason === 'overloaded') lines.push({ name: 'x-should-retry', value: 'true' }, { name: 'request-id', value: `req_011CU${e.seq}xYvK2` });
    if (a.reason === 'rateLimited') lines.push({ name: 'retry-after', value: '8' });
    if (a.reason === 'badGateway' || a.reason === 'forwardedError') lines.push({ name: 'server', value: 'cloudflare' }, { name: 'cf-ray', value: '98f3b1c7de0a4e21-HKG' });
    return lines;
  }
  if (e.path.includes('count_tokens') || !e.stream)
    return [
      { name: 'content-type', value: 'application/json' },
      { name: 'request-id', value: `req_011CV${e.seq}Pq7` },
    ];
  return [
    { name: 'content-type', value: 'text/event-stream; charset=utf-8' },
    { name: 'cache-control', value: 'no-cache' },
    { name: 'request-id', value: `req_011CV${e.seq}Pq7` },
    { name: 'anthropic-ratelimit-output-tokens-remaining', value: '79000' },
    { name: 'x-envoy-upstream-service-time', value: String(Math.round(a.durationMs / 40)) },
  ];
}

/** 成功响应的 SSE 事件（Anthropic Messages 或 OpenAI Responses） */
export function sseEvents(e: LogEntry): SseEvent[] {
  const pieces = ['我先', '看了一下 ', 'coupon.ts', '：过期判断', '用的是本地时间，', '跨时区会', '提前失效。', '我来改成 UTC 比较，', '并补一个测试。'];
  if (e.client === CODEX_UA) {
    return [
      { event: 'response.created', data: `{"type":"response.created","response":{"id":"resp_68e9${e.seq}","status":"in_progress","model":"${e.model}"}}` },
      { event: 'response.output_item.added', data: '{"type":"response.output_item.added","output_index":0,"item":{"type":"message","role":"assistant"}}' },
      ...pieces.slice(0, 6).map((p) => ({ event: 'response.output_text.delta', data: `{"type":"response.output_text.delta","delta":${JSON.stringify(p)}}` })),
      { event: 'response.output_item.done', data: '{"type":"response.output_item.done","output_index":0}' },
      { event: 'response.completed', data: '{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":21877,"output_tokens":318}}}' },
    ];
  }
  return [
    {
      event: 'message_start',
      data: `{"type":"message_start","message":{"id":"msg_01K${e.seq}Qw","type":"message","role":"assistant","model":"${e.model}","content":[],"usage":{"input_tokens":3,"cache_read_input_tokens":41212,"output_tokens":1}}}`,
    },
    { event: 'content_block_start', data: '{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}' },
    { event: 'ping', data: '{"type": "ping"}' },
    ...pieces.map((p) => ({ event: 'content_block_delta', data: `{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":${JSON.stringify(p)}}}` })),
    { event: 'content_block_stop', data: '{"type":"content_block_stop","index":0}' },
    { event: 'message_delta', data: '{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":214}}' },
    { event: 'message_stop', data: '{"type":"message_stop"}' },
  ];
}

export function nonStreamBody(e: LogEntry): Json {
  if (e.method === 'GET') return { object: 'list', data: [{ id: 'gpt-5.5', object: 'model' }, { id: 'gpt-5.5-mini', object: 'model' }] };
  if (e.path.includes('count_tokens')) return { input_tokens: 41_388 };
  return {
    id: `msg_01K${e.seq}Qw`,
    type: 'message',
    role: 'assistant',
    model: e.model,
    content: [{ type: 'text', text: 'Fix coupon expiry check' }],
    stop_reason: 'end_turn',
    usage: { input_tokens: 412, output_tokens: 7 },
  };
}

/** openrouter 实例的 format 改写：按模型选渠道、换 key、映射模型名 */
export function transformChanges(e: LogEntry): TransformChange[] {
  const isGpt = e.model?.startsWith('gpt');
  const path = e.path.split('?')[0];
  return [
    { field: 'url', before: `http://127.0.0.1:${e.port}${e.path}`, after: `https://openrouter.ai/api${path}`, note: 'channel' },
    {
      // Anthropic 风格的 x-api-key 换成 OpenRouter 要的 Bearer 头
      field: isGpt ? 'authorization' : 'x-api-key → authorization',
      before: isGpt ? 'Bearer sk-pro***' : 'sk-ant***',
      after: 'Bearer sk-or-***',
      note: 'key',
    },
    { field: 'body.model', before: JSON.stringify(e.model), after: JSON.stringify(isGpt ? `openai/${e.model}` : `anthropic/${e.model}`), note: 'model' },
    { field: 'http-referer', before: '', after: 'https://github.com/MoYeRanQianZhi/aProxy' },
  ];
}

function curlFor(e: LogEntry): string {
  const lines = [`curl -X ${e.method} 'http://127.0.0.1:${e.port}${e.path}'`];
  for (const h of requestHeaders(e)) {
    if (h.name === 'host' || h.name === 'content-length') continue;
    lines.push(`  -H '${h.name}: ${h.value}'`);
  }
  if (e.method === 'POST') lines.push(`  --data-binary @request-${e.port}-${e.seq}.json`);
  return lines.join(' \\\n');
}

function rawText(e: SimEntry, attemptIdx: number): string {
  const head = [`${e.method} ${e.path} HTTP/1.1`, ...requestHeaders(e).map((h) => `${h.name}: ${h.value}`)].join('\n');
  const body = e.method === 'POST' ? `\n\n${JSON.stringify(requestBody(e)).slice(0, 600)}…  (${e.reqBytes} bytes)` : '';
  const a = e.attempts[attemptIdx];
  if (!a) return head + body;
  let resp: string;
  if (a.status === null) {
    resp = `!! ${transportError(a.reason ?? 'reset')}`;
  } else {
    const statusText: Record<number, string> = { 200: 'OK', 404: 'Not Found', 429: 'Too Many Requests', 502: 'Bad Gateway', 503: 'Service Unavailable', 529: '' };
    const lines = [`HTTP/1.1 ${a.status} ${statusText[a.status] ?? ''}`.trimEnd(), ...responseHeaders(e, a).map((h) => `${h.name}: ${h.value}`)];
    let payload: string;
    if (a.reason) {
      const b = errorBody(a.reason, e).body;
      payload = typeof b === 'string' ? b : JSON.stringify(b);
    } else if (e.stream && !e.path.includes('count_tokens')) {
      payload = sseEvents(e)
        .slice(0, 5)
        .map((ev) => `event: ${ev.event}\ndata: ${ev.data}`)
        .join('\n\n') + '\n\n…';
    } else {
      payload = JSON.stringify(nonStreamBody(e));
    }
    resp = lines.join('\n') + '\n\n' + payload;
  }
  return `${head}${body}\n\n---- attempt ${a.n} ----\n\n${resp}`;
}

export function pickRand<T>(r: Rand, items: readonly T[]): T {
  return pick(r, items);
}

/**
 * 点开一条日志时后端交给界面的全部细节。每次尝试各自成形：没拿到响应的只有传输错误，
 * 失败的带上游错误体，成功的带 SSE 事件或响应体——界面不需要再猜它属于哪一种。
 */
export function buildDetail(e: SimEntry): RequestDetail {
  const streamed = e.stream && !e.path.includes('count_tokens');
  const attempts: AttemptDetail[] = e.attempts.map((a, i) => {
    const raw = rawText(e, i);
    if (a.status === null) {
      return { headers: [], transportError: transportError(a.reason ?? 'reset'), errorBody: null, sse: null, body: null, raw };
    }
    if (a.reason) {
      return { headers: responseHeaders(e, a), transportError: null, errorBody: errorBody(a.reason, e).body, sse: null, body: null, raw };
    }
    // 成功的那次：还在接收中时内容还没齐，不给正文
    const done = e.outcome !== 'streaming';
    return {
      headers: responseHeaders(e, a),
      transportError: null,
      errorBody: null,
      sse: done && streamed ? sseEvents(e) : null,
      body: done && !streamed ? nonStreamBody(e) : null,
      raw,
    };
  });
  return {
    key: e.key,
    request: { headers: requestHeaders(e), body: e.method === 'POST' ? requestBody(e) : null, curl: curlFor(e), raw: rawText(e, -1) },
    attempts,
    transform: e.transformed ? transformChanges(e) : [],
  };
}

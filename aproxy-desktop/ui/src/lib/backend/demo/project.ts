/*
 * 把模拟引擎里的实例与请求投影成契约（../contract.ts）的形状。
 *
 * 引擎（../mock/engine.ts）按「演示起来像真的」来组织数据，契约按 Rust 一侧实际交出来的样子来定，
 * 两者之间的差别都在这里抹平：界面拿到的演示数据与真实数据是同一种形状，页面不必知道自己看的是哪一种。
 * 这里全是纯函数加一张「请求属于哪次运行」的表，不持有别的状态。
 */
import type {
  Action,
  Activity,
  BodyInfo,
  BodyPart,
  Card,
  Detail,
  Header,
  HistoryCode,
  Message,
  Outcome,
  Phase,
  Report,
  Row,
  Run,
  Try,
  TryDetail,
  Watchdog,
} from '../contract';
import type { Attempt, HeaderLine, Instance, Reason, WatchdogInfo } from '../../types';
import {
  errorBody,
  nonStreamBody,
  requestBody,
  requestHeaders,
  responseHeaders,
  sseEvents,
  transportError,
  type SimEntry,
} from '../mock/fake';

/** 演示里的失败原因对应契约里的哪个码。客户端断开不是「尝试失败」，没有码 */
const CODE_OF: Record<Reason, HistoryCode | null> = {
  overloaded: 'upstream_status',
  unavailable: 'upstream_status',
  badGateway: 'upstream_status',
  rateLimited: 'upstream_status',
  notFound: 'upstream_status',
  forwardedError: 'upstream_status',
  reset: 'upstream_connect_failed',
  readTimeout: 'upstream_read_idle_timeout',
  errorBody: 'upstream_error_body',
  clientGone: null,
  stopped: 'shutting_down',
};

/** 一次运行的标识：实例每次启动的时刻不同，标识也就不同 */
export function runOf(inst: Instance): Run | null {
  if (inst.status === 'stopped') return null;
  return { port: inst.port, instanceId: `demo-${inst.port}-${inst.startedAt.toString(36)}` };
}

/**
 * 请求属于哪次运行。引擎不记这件事，所以第一次见到一条请求时把它记在实例当时的那次运行名下；
 * 实例重启以后，之前的请求仍然挂在旧的运行上，和真实的历史一样。
 */
export class RunBook {
  private ids = new Map<string, string>();

  bind(e: SimEntry, inst: Instance | undefined): Run {
    let id = this.ids.get(e.key);
    if (!id) {
      id = (inst && runOf(inst)?.instanceId) ?? `demo-${e.port}-gone`;
      this.ids.set(e.key, id);
    }
    return { port: e.port, instanceId: id };
  }
}

// ---------------------------------------------------------------------------
// 实例
// ---------------------------------------------------------------------------

/** 各实例在演示开始之前「已经」重试过、救回过多少：让累计数字和请求序号的量级对得上 */
const HISTORY: Record<number, { retries: number; rescued: number }> = {
  12345: { retries: 1_106, rescued: 512 },
  12346: { retries: 1_874, rescued: 772 },
};

function activityOf(inst: Instance, logs: SimEntry[]): Activity {
  const mine = logs.filter((e) => e.port === inst.port);
  const byCode: Partial<Record<HistoryCode, number>> = {};
  let rescued = 0;
  for (const e of mine) {
    if (e.outcome === 'rescued') rescued += 1;
    for (const a of e.attempts) {
      const code = a.reason && CODE_OF[a.reason];
      if (code) byCode[code] = (byCode[code] ?? 0) + 1;
    }
  }
  const base = HISTORY[inst.port] ?? { retries: 0, rescued: 0 };
  const err = inst.lastError;
  const errCode = err && CODE_OF[err.reason];
  return {
    lastRequestAt: inst.lastRequestAt ?? inst.startedAt,
    // 请求的序号就是实例内的第几个请求，最大的那个加一就是总数
    requestsTotal: mine.reduce((m, e) => Math.max(m, e.seq + 1), 0),
    retriesTotal: base.retries + inst.retry1h,
    rescuedTotal: base.rescued + rescued,
    upgradedTotal: 0,
    inFlight: mine.filter((e) => e.outcome === 'retrying' || e.outcome === 'streaming').length,
    retrying: mine.filter((e) => e.outcome === 'retrying').length,
    byCode,
    lastError: err && errCode ? { code: errCode, at: err.at, status: err.status } : null,
    // 演示里每次失败的尝试都会重试，所以两条线相同
    buckets: { widthSecs: 120, requests: [...inst.spark.req], retries: [...inst.spark.retry], failures: [...inst.spark.retry] },
  };
}

/**
 * 一个实例的卡片。restorePending 由调用方给：引擎里「停着」的实例，只有留着「应当在运行」记录的那些
 * 才出现在界面上（等待恢复），用户自己停掉的不出现。
 */
export function cardOf(inst: Instance, logs: SimEntry[], restorePending: boolean): Card {
  const down = inst.status === 'stopped';
  const stuck = inst.status === 'unresponsive';
  const readable = !down && !stuck;
  return {
    port: inst.port,
    run: runOf(inst),
    pid: inst.pid,
    version: inst.version,
    startedAt: down ? null : inst.startedAt,
    liveness: down ? 'down' : stuck ? 'unresponsive' : 'live',
    restorePending: down && restorePending,
    restoreReason: null,
    expectedRestore: down && restorePending ? restoreToken(inst.port) : null,
    feed: down ? 'offline' : stuck ? 'disconnected' : 'streaming',
    historyUnavailable: null,
    reads: { historyFeed: readable, detail: readable, body: readable },
    state: down ? null : inst.status === 'restarting' ? 'starting' : 'serving',
    // 卡住的实例留着最后一次答复时的数字
    activity: down ? null : activityOf(inst, logs),
    metadata: {
      listen: inst.listen,
      baseUrl: inst.baseUrl,
      configPath: inst.configPath,
      aliases: inst.alias ? [inst.alias] : [],
      delivery: inst.forwardOnly ? 'forward' : 'retry',
      diskCache: inst.diskCache,
      keepaliveIntervalSecs: inst.keepalive.interval,
      keepaliveTrigger: inst.keepalive.trigger === 'body_stream' ? 'body-stream' : inst.keepalive.trigger,
      retryRuleCount: inst.boundedPaths.length,
    },
  };
}

export function restoreToken(port: number): string {
  return `restore:${port}`;
}

export function watchdogOf(w: WatchdogInfo | null): Watchdog | null {
  if (!w) return null;
  return {
    pid: w.pid,
    state: 'running',
    lastScanAt: w.lastScanAt,
    restarts: w.respawns.map((r) => ({ port: r.port, at: r.at, kind: r.kind, ok: true })),
  };
}

// ---------------------------------------------------------------------------
// 请求
// ---------------------------------------------------------------------------

/** format 程序报告自己对这条请求做了什么：只有带 format 的实例才有 */
function reportsOf(e: SimEntry): Report[] {
  if (!e.transformed) return [];
  const gpt = e.model?.startsWith('gpt') ?? false;
  const actions: Action[] = [
    { type: 'route', channel: gpt ? 'openai' : 'anthropic', modelFrom: e.model, modelTo: e.model && `${gpt ? 'openai' : 'anthropic'}/${e.model}`, label: null },
    { type: 'key-select', keyId: `key-${e.keyIndex ?? 1}`, label: null },
    { type: 'header-edit', name: 'http-referer', op: 'set', label: null },
  ];
  if (!gpt) actions.push({ type: 'header-edit', name: 'x-api-key', op: 'remove', label: null });
  return [{ format: 'aproxy-format', stage: 'request', summary: `route ${gpt ? 'openai' : 'anthropic'}, key ${e.keyIndex ?? 1}`, actions }];
}

function tryOf(e: SimEntry, a: Attempt, last: boolean): Try {
  // 重试的决定：客户端断开、实例停止都不是规则做的决定；仅转发的实例不重试
  const decided = a.reason !== null && a.reason !== 'clientGone' && a.reason !== 'stopped' && a.reason !== 'forwardedError';
  const stop = last && e.phase === null;
  return {
    attempt: a.n,
    startedAt: a.startedAt,
    // 还在接收响应的那次没有耗时
    durationMs: e.outcome === 'streaming' ? null : a.durationMs,
    upstreamStatus: a.status,
    code: a.reason ? CODE_OF[a.reason] : null,
    responseBytes: a.respBytes,
    decision: decided
      ? { by: a.reason === 'notFound' ? 'bounded-path' : 'default', decision: stop ? 'stop' : 'retry', delayMs: stop ? null : a.backoffMs, reroute: null }
      : null,
    reports: [],
  };
}

function triesOf(e: SimEntry): Try[] {
  const tries = e.attempts.map((a, i) => tryOf(e, a, i === e.attempts.length - 1));
  // 引擎在一次尝试结束时才记下它；正在进行的那次补在末尾
  if (e.outcome === 'retrying' && e.phase === 'attempting') {
    tries.push({
      attempt: e.attempts.length + 1,
      startedAt: e.phaseStartedAt ?? e.time,
      durationMs: null,
      upstreamStatus: null,
      code: null,
      responseBytes: 0,
      decision: null,
      reports: [],
    });
  }
  return tries;
}

function endOf(e: SimEntry): { outcome: Outcome; code: HistoryCode | null; clientStatus: number | null; phase: Phase } {
  const last = e.attempts[e.attempts.length - 1];
  switch (e.outcome) {
    case 'ok':
    case 'rescued':
      return { outcome: 'ok', code: null, clientStatus: 200, phase: 'done' };
    case 'streaming':
      return { outcome: 'in-flight', code: null, clientStatus: 200, phase: 'forwarding' };
    case 'retrying':
      return {
        outcome: 'in-flight',
        code: last?.reason ? CODE_OF[last.reason] : null,
        // 保活心跳发出去的时候，响应头（200）已经交给客户端了
        clientStatus: e.heartbeats > 0 ? 200 : null,
        phase: e.phase === 'waiting' ? 'backoff' : 'attempting',
      };
    case 'failed':
      if (last?.reason === 'clientGone') return { outcome: 'client-gone', code: null, clientStatus: null, phase: 'done' };
      if (last?.reason === 'stopped') return { outcome: 'aproxy-error', code: 'shutting_down', clientStatus: null, phase: 'done' };
      return {
        outcome: 'upstream-error',
        code: last?.reason === 'notFound' ? 'retry_exhausted' : 'upstream_status',
        clientStatus: last?.status ?? null,
        phase: 'done',
      };
  }
}

/** 记录被改过几次：每多一次尝试、每换一个阶段都变大，终态最大 */
function revisionOf(e: SimEntry): number {
  const step = e.phase === null ? 3 : e.phase === 'attempting' ? 2 : 1;
  return e.attempts.length * 4 + step + (e.outcome === 'streaming' ? 0 : 1);
}

export function rowOf(e: SimEntry, run: Run, inst: Instance | undefined): Row {
  const tries = triesOf(e);
  return {
    ref: { run, requestId: String(e.seq) },
    seq: e.seq,
    revision: revisionOf(e),
    startedAt: e.time,
    endedAt: e.phase === null ? e.time + e.durationMs : null,
    method: e.method,
    path: e.path,
    model: e.model,
    userAgent: e.client,
    stream: e.stream,
    delivery: inst?.forwardOnly ? 'forward' : 'retry',
    upgraded: false,
    ...endOf(e),
    nextAttemptAt: e.outcome === 'retrying' && e.phase === 'waiting' ? e.phaseEndsAt : null,
    attempts: tries.length,
    triesOmitted: 0,
    tries,
    keepalive: e.stream && (inst?.keepalive.interval ?? 0) > 0,
    heartbeats: e.heartbeats,
    requestBytes: e.reqBytes,
    responseBytes: e.respBytes,
    reports: reportsOf(e),
    truncated: false,
  };
}

// ---------------------------------------------------------------------------
// 详情与体
// ---------------------------------------------------------------------------

const encoder = new TextEncoder();

function headersOf(lines: HeaderLine[]): Header[] {
  return lines.map((h) => ({ name: h.name, value: h.secret ? { secret: true } : h.value }));
}

function retained(bytes: Uint8Array | null): BodyInfo | null {
  return bytes && { bytes: bytes.length, state: 'retained', cause: null, encoding: null };
}

/** 经 format 改写之后发给上游的请求头 */
function sentHeaders(e: SimEntry): Header[] {
  const kept = headersOf(requestHeaders(e)).filter((h) => !['host', 'x-api-key', 'authorization'].includes(h.name));
  if (!e.transformed) return headersOf(requestHeaders(e)).filter((h) => h.name !== 'host');
  return [
    { name: 'authorization', value: { secret: true } },
    ...kept,
    { name: 'http-referer', value: 'https://github.com/MoYeRanQianZhi/aProxy' },
  ];
}

function sentModel(e: SimEntry): string | null {
  if (!e.transformed || !e.model) return e.model;
  return `${e.model.startsWith('gpt') ? 'openai' : 'anthropic'}/${e.model}`;
}

/** 一段体的字节。没有这段体（GET 没有请求体、没拿到响应、响应还没收完）时为 null */
export function bodyBytes(e: SimEntry, part: BodyPart): Uint8Array | null {
  if (part.kind === 'request' || part.kind === 'sent') {
    const body = requestBody(e);
    if (body === null) return null;
    if (part.kind === 'sent' && e.transformed) (body as { model: string | null }).model = sentModel(e);
    return encoder.encode(JSON.stringify(body));
  }
  const a = e.attempts[part.attempt - 1];
  if (!a || a.status === null) return null;
  if (a.reason) {
    const b = errorBody(a.reason, e).body;
    return encoder.encode(typeof b === 'string' ? b : JSON.stringify(b));
  }
  if (e.outcome === 'streaming') return null;
  if (e.stream && !e.path.includes('count_tokens')) {
    return encoder.encode(sseEvents(e).map((ev) => `event: ${ev.event}\ndata: ${ev.data}\n\n`).join(''));
  }
  return encoder.encode(JSON.stringify(nonStreamBody(e)));
}

function tryDetailOf(e: SimEntry, t: Try, inst: Instance | undefined): TryDetail {
  const a: Attempt | undefined = e.attempts[t.attempt - 1];
  const sentBody = bodyBytes(e, { kind: 'sent', attempt: t.attempt });
  const sent: Message & { method: string; url: string } = {
    method: e.method,
    url: `${e.transformed ? 'https://openrouter.ai/api' : (inst?.baseUrl ?? '')}${e.path.split('?')[0]}`,
    headers: sentHeaders(e),
    // 改写过的请求体第一次发出时留一份，之后每次重试发的是同一份；没改写的与客户端发来的相同
    body: sentBody && (e.transformed && t.attempt === 1 ? retained(sentBody) : { bytes: sentBody.length, state: 'same-as-previous', cause: null, encoding: null }),
  };
  const response =
    a && a.status !== null
      ? { upstreamStatus: a.status, headers: headersOf(responseHeaders(e, a)), body: retained(bodyBytes(e, { kind: 'response', attempt: t.attempt })) }
      : null;
  const code = a?.reason ? CODE_OF[a.reason] : null;
  const error = a?.reason && code ? { code, message: transportError(a.reason) || `upstream returned ${a.status}` } : null;
  return { attempt: t.attempt, sent, response, error };
}

export function detailOf(e: SimEntry, run: Run, inst: Instance | undefined): Detail {
  const record = rowOf(e, run, inst);
  return {
    record,
    request: { headers: headersOf(requestHeaders(e)), body: retained(bodyBytes(e, { kind: 'request' })) },
    tries: record.tries.map((t) => tryDetailOf(e, t, inst)),
  };
}

/** 字节转 base64：分段拼，免得一次展开太多参数 */
export function toBase64(bytes: Uint8Array): string {
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

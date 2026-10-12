/*
 * mock 后端的模拟引擎：持有实例与日志，用一个计时器模拟真实流量——
 * 新请求不断到达、重试中的请求按 aProxy 的退避规则倒计时并重新尝试、救回时计数加一。
 *
 * 它是普通的 TypeScript（不用 Svelte 的响应式）：界面拿到的一切都经 Backend 接口的事件推过去，
 * 和将来的 IPC 一样是「一份份数据」，而不是共享同一个可变对象。所以引擎每拍只把变了的东西深拷贝后发出。
 */
import { ui, type Scenario } from '../../state.svelte';
import { backoffAfter } from '../../retry';
import type { Instance, LogEntry, Reason } from '../../types';
import type { BackendEvent, Snapshot } from '../types';
import { attemptMs, makeArrival, makeInstances, makeLogs, makeWatchdog, respBytesFor, rng, STATUS_OF, type SimEntry } from './fake';

const MAX_LOGS = 600;
/** 「长时间重试」的门槛：至少失败 3 次，或者从发出到成功超过 15 秒。立即重试一两次就成的不值得庆祝 */
const LONG_RESCUE_FAILS = 3;
const LONG_RESCUE_MS = 15_000;

/** 交给界面之前去掉只有模拟才需要的字段，并断开与引擎内部对象的引用 */
function publicEntry(e: SimEntry): LogEntry {
  const { succeedAt: _s, promptIdx: _p, ...rest } = structuredClone(e);
  return rest;
}

export class Engine {
  private now = Date.now();
  private instancesAll: Instance[] = makeInstances(this.now);
  logs: SimEntry[];
  private watchdog = makeWatchdog(this.now);
  private rescuedTotal = 1284;
  private rescuedToday = 37;
  scenario: Scenario = 'normal';

  private r = rng(2026);
  private nextArrivalAt = this.now + 2_000;
  private seq: Record<number, number> = {};
  private listeners = new Set<(e: BackendEvent) => void>();
  private timer: ReturnType<typeof setInterval> | undefined;
  /** 上一次推出去的实例列表：没变就不推，和真实后端只推变化一样 */
  private lastInstances = '';

  constructor() {
    this.logs = makeLogs(this.now, this.instancesAll);
    for (const e of this.logs) this.seq[e.port] = Math.max(this.seq[e.port] ?? 0, e.seq + 1);
  }

  /** 「没有实例」场景下，对外就像真的一个实例也没有 */
  get instances(): Instance[] {
    return this.scenario === 'empty' ? [] : this.instancesAll;
  }

  get visibleLogs(): SimEntry[] {
    return this.scenario === 'empty' ? [] : this.logs;
  }

  instance(port: number): Instance | undefined {
    return this.instancesAll.find((i) => i.port === port);
  }

  entry(key: string): SimEntry | undefined {
    return this.logs.find((e) => e.key === key);
  }

  snapshot(): Snapshot {
    return {
      instances: structuredClone(this.instances),
      logs: this.visibleLogs.map(publicEntry),
      watchdog: this.scenario === 'empty' ? null : structuredClone(this.watchdog),
      rescuedTotal: this.rescuedTotal,
      rescuedToday: this.rescuedToday,
    };
  }

  subscribe(listener: (e: BackendEvent) => void): () => void {
    this.listeners.add(listener);
    // 有人订阅才开始模拟：250ms 一拍，倒计时与进度条需要比秒更细的步长才显得连贯
    this.timer ??= setInterval(() => this.tick(Date.now()), 250);
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size === 0) {
        clearInterval(this.timer);
        this.timer = undefined;
      }
    };
  }

  emit(event: BackendEvent) {
    for (const l of this.listeners) l(event);
  }

  private emitInstances() {
    const json = JSON.stringify(this.instances);
    if (json === this.lastInstances) return;
    this.lastInstances = json;
    this.emit({ type: 'instances', instances: JSON.parse(json) });
  }

  private emitUpdated(entries: SimEntry[]) {
    if (entries.length && this.scenario !== 'empty') this.emit({ type: 'logs-updated', entries: entries.map(publicEntry) });
  }

  private emitStats() {
    this.emit({
      type: 'stats',
      watchdog: this.scenario === 'empty' ? null : structuredClone(this.watchdog),
      rescuedTotal: this.rescuedTotal,
      rescuedToday: this.rescuedToday,
    });
  }

  /** 原型审核用：切换场景时直接改实例状态，后续操作（如强制停止）可以在此基础上继续 */
  setScenario(s: Scenario) {
    this.scenario = s;
    // 审核工具条显示的当前场景；mock 本来就是原型的一部分，直接写界面状态最省事
    ui.scenario = s;
    const or = this.instancesAll.find((i) => i.port === 12346);
    if (or) {
      // 已经卡住时重复点「出错」什么也不做：再前移一次时间会让快照凭空又老 45 秒
      if (s === 'error' && or.status !== 'stopped' && or.status !== 'unresponsive') {
        or.status = 'unresponsive';
        // 卡住的实例最后一份快照停在 45 秒前。把它在途请求的时间整体前移同样的量，
        // 「快照时刻」和「沉默时长」才对得上——否则已等时长、进度会出现负数
        const silentFor = 45_000;
        or.silentSince = this.now - silentFor;
        for (const e of this.logs) {
          if (e.port !== or.port || e.phase === null || e.phaseStartedAt === null || e.phaseEndsAt === null) continue;
          e.time -= silentFor;
          e.phaseStartedAt -= silentFor;
          e.phaseEndsAt -= silentFor;
          for (const a of e.attempts) a.startedAt -= silentFor;
          e.durationMs = or.silentSince - e.time;
        }
        this.logs.sort((a, b) => a.time - b.time);
      } else if (s !== 'error' && or.status === 'unresponsive') {
        or.status = 'active';
        or.silentSince = null;
      }
    }
    // 场景一换，界面看到的世界整体变了：发一份完整快照，而不是拼凑增量
    this.lastInstances = JSON.stringify(this.instances);
    this.emit({ type: 'snapshot', snapshot: this.snapshot() });
  }

  /** 原型审核用：立刻触发一次救回庆祝，不用等模拟流量碰上 */
  celebrate() {
    const e = this.visibleLogs.filter((x) => x.outcome === 'rescued').at(-1);
    if (e) this.emit({ type: 'rescued', key: e.key, at: this.now });
  }

  private cutInflight(port: number) {
    const cut: SimEntry[] = [];
    for (const e of this.logs) {
      if (e.port !== port || (e.outcome !== 'retrying' && e.outcome !== 'streaming')) continue;
      e.outcome = 'failed';
      e.phase = null;
      e.durationMs = this.now - e.time;
      e.attempts.push({
        n: e.attempts.length + 1,
        startedAt: e.phaseStartedAt ?? this.now,
        durationMs: Math.max(0, this.now - (e.phaseStartedAt ?? this.now)),
        status: null,
        reason: 'stopped',
        backoffMs: 0,
        respBytes: 0,
      });
      cut.push(e);
    }
    this.emitUpdated(cut);
  }

  stop(port: number) {
    const inst = this.instance(port);
    if (!inst) return;
    this.cutInflight(port);
    inst.status = 'stopped';
    inst.pid = null;
    inst.silentSince = null;
    this.emitInstances();
  }

  /** 模拟一个需要时间的状态切换：先进入「正在重启」，过一会儿落到目标状态 */
  private cycle(port: number, after: Instance['status'], ms: number): Promise<void> {
    const inst = this.instance(port);
    if (!inst) return Promise.resolve();
    inst.status = 'restarting';
    inst.silentSince = null;
    this.emitInstances();
    return new Promise((resolve) =>
      setTimeout(() => {
        inst.status = after;
        inst.pid = 20000 + Math.floor(this.r() * 9000);
        inst.startedAt = Date.now();
        this.emitInstances();
        resolve();
      }, ms),
    );
  }

  restart(port: number): Promise<void> {
    this.cutInflight(port);
    return this.cycle(port, 'active', 1600);
  }

  start(port: number): Promise<void> {
    return this.cycle(port, 'idle', 1200);
  }

  setPinned(key: string, pinned: boolean) {
    const e = this.entry(key);
    if (!e) return;
    e.pinned = pinned;
    this.emitUpdated([e]);
  }

  private tick(now: number) {
    this.now = now;
    if (this.scenario === 'empty') return;

    const changed: SimEntry[] = [];
    for (const e of this.logs) {
      if (e.phase === null || e.phaseEndsAt === null) continue;
      if (this.scenario === 'error' && e.port === 12346) continue; // 卡住的实例：一切静止，连耗时也不再增长
      e.durationMs = now - e.time;
      if (e.outcome === 'retrying') {
        // 保活心跳：流式请求在等待期间每 15 秒一拍
        if (e.stream && e.port !== 12350) e.heartbeats = Math.floor(e.durationMs / 15_000);
      }
      changed.push(e);
      if (now < e.phaseEndsAt) continue;
      this.advance(e, now);
    }
    this.emitUpdated(changed);

    let statsChanged = false;
    // 看护者每 30 秒巡检一次
    if (now - this.watchdog.lastScanAt >= 30_000) {
      this.watchdog.lastScanAt = now;
      statsChanged = true;
    }

    if (now >= this.nextArrivalAt) {
      this.nextArrivalAt = now + 2_400 + this.r() * 3_200;
      this.arrive(now);
    }
    if (statsChanged || changed.some((e) => e.outcome === 'rescued' && e.phase === null)) this.emitStats();
    this.emitInstances();
  }

  private advance(e: SimEntry, now: number) {
    const inst = this.instance(e.port);
    if (!inst) return;

    if (e.outcome === 'streaming') {
      const a = e.attempts[0];
      a.durationMs = now - a.startedAt;
      if (!a.respBytes) a.respBytes = respBytesFor(this.r, e);
      e.respBytes = a.respBytes;
      e.outcome = 'ok';
      e.phase = null;
      return;
    }

    if (e.phase === 'waiting') {
      // 退避结束：发起下一次尝试。会成功的那次耗时按正常响应算（缩短后演示）
      const n = e.attempts.length + 1;
      const willSucceed = n >= e.succeedAt;
      const reason = this.reasonOf(e);
      e.phase = 'attempting';
      e.phaseStartedAt = now;
      e.phaseEndsAt = now + (willSucceed ? attemptMs(this.r, null, e.model, e.stream) / 5 : attemptMs(this.r, reason, e.model, e.stream));
      return;
    }

    // 一次尝试结束
    const n = e.attempts.length + 1;
    const startedAt = e.phaseStartedAt ?? now;
    if (n >= e.succeedAt && e.attempts.length > 0) {
      const respBytes = respBytesFor(this.r, e);
      e.attempts.push({ n, startedAt, durationMs: now - startedAt, status: 200, reason: null, backoffMs: 0, respBytes });
      e.outcome = 'rescued';
      e.respBytes = respBytes;
      e.phase = null;
      this.rescuedTotal += 1;
      this.rescuedToday += 1;
      if (e.attempts.length - 1 >= LONG_RESCUE_FAILS || now - e.time >= LONG_RESCUE_MS) {
        this.emit({ type: 'rescued', key: e.key, at: now });
      }
      return;
    }
    const reason = this.reasonOf(e);
    const backoffMs = backoffAfter(n, inst.backoffCap);
    e.attempts.push({
      n,
      startedAt,
      durationMs: now - startedAt,
      status: STATUS_OF[reason],
      reason,
      backoffMs,
      respBytes: STATUS_OF[reason] === null ? 0 : 120 + Math.floor(this.r() * 900),
    });
    e.phase = 'waiting';
    e.phaseStartedAt = now;
    // 立即重试也留 250ms，让界面看得见「失败 → 再试」这一下
    e.phaseEndsAt = now + Math.max(backoffMs, 250);
    inst.retry1h += 1;
    inst.spark.retry[29] += 1;
    inst.lastError = { reason, status: STATUS_OF[reason], at: now };
  }

  private reasonOf(e: SimEntry): Reason {
    const prev = e.attempts[e.attempts.length - 1]?.reason;
    if (prev && prev !== 'stopped') return prev;
    return e.port === 12346 ? (this.r() < 0.5 ? 'overloaded' : 'rateLimited') : 'overloaded';
  }

  private arrive(now: number) {
    const candidates = this.instancesAll.filter(
      (i) => (i.status === 'active' || i.status === 'idle') && !i.forwardOnly && !(this.scenario === 'error' && i.port === 12346),
    );
    if (candidates.length === 0) return;
    const inst = candidates[Math.floor(this.r() * candidates.length)];
    const seq = this.seq[inst.port] ?? 1;
    this.seq[inst.port] = seq + 1;
    const e = makeArrival(this.r, inst.port, seq, now, inst);
    this.logs.push(e);
    this.emit({ type: 'logs-appended', entries: [publicEntry(e)] });
    if (this.logs.length > MAX_LOGS) {
      const idx = this.logs.findIndex((x) => !x.pinned && x.phase === null);
      if (idx >= 0) {
        const [gone] = this.logs.splice(idx, 1);
        this.emit({ type: 'logs-removed', keys: [gone.key] });
      }
    }
    inst.status = 'active';
    inst.lastRequestAt = now;
    inst.req1h += 1;
    inst.spark.req[29] += 1;
  }
}

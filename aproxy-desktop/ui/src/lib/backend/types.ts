/*
 * 模拟引擎（mock/engine.ts）交给演示后端（demo/）的事件与全貌。
 *
 * 引擎按原型时期的数据形状（../types.ts 里的 Instance、LogEntry…）模拟实例与流量；演示后端把这些事件
 * 换成契约（contract.ts）的通知与数据，界面只认契约。这两个类型只在这两者之间流动。
 */
import type { Instance, LogEntry, WatchdogInfo } from '../types';

/** 演示后端启动或重连时拉取的全貌 */
export interface Snapshot {
  instances: Instance[];
  /** 最近的日志，旧的在前 */
  logs: LogEntry[];
  watchdog: WatchdogInfo | null;
  rescuedTotal: number;
  rescuedToday: number;
}

export type BackendEvent =
  /** 整体替换：重置场景之后 */
  | { type: 'snapshot'; snapshot: Snapshot }
  /** 实例只有几个，每次整份推 */
  | { type: 'instances'; instances: Instance[] }
  | { type: 'logs-appended'; entries: LogEntry[] }
  /** 在途条目的进展（新的一次尝试、进入等待、结束），按 key 整条替换 */
  | { type: 'logs-updated'; entries: LogEntry[] }
  /** 超出保留上限被淘汰的条目 */
  | { type: 'logs-removed'; keys: string[] }
  | { type: 'stats'; watchdog: WatchdogInfo | null; rescuedTotal: number; rescuedToday: number }
  /** 一条请求在长时间重试后成功了，界面据此庆祝一下 */
  | { type: 'rescued'; key: string; at: number };

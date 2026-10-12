/*
 * 界面与数据之间的那层薄接口。
 *
 * 组件不直接碰数据源：它们读客户端 store（lib/store.svelte.ts），store 只经这个接口取数据、发命令。
 * 原型里只有 mock 实现（lib/backend/mock）；正式版是 Tauri IPC 实现；将来要做 Web UI，再加一个
 * HTTP/WebSocket 实现即可，整套界面不用改。所以这里不能出现 Tauri API 或任何传输方式的细节：
 *   - 所有方法都是异步的——任何真实实现都要跨进程；
 *   - 推送统一走 subscribe，事件是可以序列化的纯数据（mock 也按「跨进程」的样子深拷贝后再发）；
 *   - 只有「写」类命令（启停、重启、改配置、安装）会改变实例。扩展调用它们时由后端检查授权：
 *     已允许的直接执行，第一次的先发 consent-request 等用户回答。界面只展示、转达，不做判断——
 *     扩展和界面跑在同一个页面里，界面这一侧的检查拦不住一个不守规矩的扩展。
 *
 * 安全边界在实例与 IPC：接口里没有任何能拿到明文密钥的方法，配置里的密钥只写不读。
 */
import type {
  AppSettings,
  ConfigIssue,
  ConfigKey,
  ConfigValues,
  ConsentRequest,
  ExtensionInfo,
  Instance,
  InstanceConfig,
  LogEntry,
  RequestDetail,
  UpdateInfo,
  UpdateProgress,
  WatchdogInfo,
  WriteCapability,
} from '../types';

/** 界面启动或重连时拉取的全貌 */
export interface Snapshot {
  instances: Instance[];
  /** 最近的日志，旧的在前；条数由实现决定，更早的用 queryLogs 往前翻 */
  logs: LogEntry[];
  watchdog: WatchdogInfo | null;
  rescuedTotal: number;
  rescuedToday: number;
}

export type BackendEvent =
  /** 整体替换：重连之后，或后端状态整体变了 */
  | { type: 'snapshot'; snapshot: Snapshot }
  /** 实例只有几个，每次整份推最简单，也不怕丢一条增量 */
  | { type: 'instances'; instances: Instance[] }
  | { type: 'logs-appended'; entries: LogEntry[] }
  /** 在途条目的进展（新的一次尝试、进入等待、结束），按 key 整条替换 */
  | { type: 'logs-updated'; entries: LogEntry[] }
  /** 超出保留上限被淘汰的条目 */
  | { type: 'logs-removed'; keys: string[] }
  | { type: 'stats'; watchdog: WatchdogInfo | null; rescuedTotal: number; rescuedToday: number }
  /** 一条请求在长时间重试后成功了。怎样算「长时间」由后端定，界面据此庆祝一下 */
  | { type: 'rescued'; key: string; at: number }
  | { type: 'update-progress'; progress: UpdateProgress }
  /** 扩展列表、启用状态或授权变了。扩展只有几个，整份推 */
  | { type: 'extensions'; extensions: ExtensionInfo[] }
  /** 扩展第一次调用某类写操作：那次调用挂起，等界面经 answerConsent 转达用户的回答 */
  | { type: 'consent-request'; request: ConsentRequest };

export interface LogQuery {
  /** 只取这条（key）之前更早的；不填就从最新的开始 */
  before?: string;
  limit: number;
  port?: number;
}

export interface LogPage {
  entries: LogEntry[];
  /** 还有更早的 */
  more: boolean;
}

export interface Backend {
  /**
   * 数据是不是演示用的假数据。界面据此在显眼处常驻标明「预览版 · 演示数据」，免得用户把假的实例
   * 当成自己电脑上正在跑的。接上真实数据的实现给 false，标签随之消失。
   */
  readonly demo: boolean;

  snapshot(): Promise<Snapshot>;
  /** 返回取消订阅的函数 */
  subscribe(listener: (event: BackendEvent) => void): () => void;

  queryLogs(query: LogQuery): Promise<LogPage>;
  /** 请求体与响应只在点开某条日志时按需取：它们可能很大，也是最敏感的内容 */
  requestDetail(key: string): Promise<RequestDetail>;
  setPinned(key: string, pinned: boolean): Promise<void>;

  /** 不给端口时按默认配置启动（空状态里的「启动第一个实例」） */
  startInstance(port?: number): Promise<void>;
  stopInstance(port: number): Promise<void>;
  restartInstance(port: number): Promise<void>;
  /** 实例无响应时直接结束进程，不走交接 */
  forceStopInstance(port: number): Promise<void>;
  /** 实例无响应时让界面再试着连一次控制通道 */
  reconnectInstance(port: number): Promise<void>;

  readConfig(port: number): Promise<InstanceConfig>;
  /**
   * 保存前让实例按它自己的规则检查一遍改动：端口有没有被别的实例占、上游是不是指回了 aProxy 自己。
   * 界面只能查格式，这类判断要看全局，以实例的实现为准。空数组表示可以保存
   */
  validateConfig(port: number, changes: Partial<ConfigValues>): Promise<ConfigIssue[]>;
  /** 只传改了的键；返回其中需要重启实例才生效的那些 */
  writeConfig(port: number, changes: Partial<ConfigValues>): Promise<{ restartRequired: ConfigKey[] }>;

  readSettings(): Promise<AppSettings>;
  writeSettings(changes: Partial<AppSettings>): Promise<void>;

  checkUpdate(channel: UpdateInfo['channel']): Promise<UpdateInfo>;
  /** 进度经 update-progress 事件推送，全部完成时兑现 */
  installUpdate(version: string): Promise<void>;

  listExtensions(): Promise<ExtensionInfo[]>;
  setExtensionEnabled(id: string, enabled: boolean): Promise<void>;
  /** 撤销一项写授权：扩展下次再做这类操作时重新询问 */
  revokeGrant(id: string, capability: WriteCapability): Promise<void>;
  /** 回答 consent-request：允许就记住授权并执行挂起的那次操作，拒绝则那次调用失败 */
  answerConsent(requestId: string, allow: boolean): Promise<void>;
  /**
   * 自定义吉祥物的代码（ES 模块源码，原样读自它的文件夹）。界面拿到后原样交给沙箱 iframe，自己从不执行；
   * 吉祥物的名字、位置等元数据在 listExtensions 里（kind 为 mascot 的条目）
   */
  readMascot(id: string): Promise<string>;
}

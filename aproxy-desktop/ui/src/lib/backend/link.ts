/*
 * 契约的传输层：怎样发一个命令、怎样接事件。界面的 store 只认这个接口，不碰 Tauri。
 *
 * 两个实现：桌面窗口里的 Tauri 实现（tauri.ts），浏览器里开发与截图用的演示实现（demo/）。
 * 两者对 store 完全一样：同一张命令表、同一种事件、同一种错误。
 */
import type { CommandName, Commands, DesktopEvent, Fault, None } from './contract';

/**
 * 命令失败的样子。Rust 给的是 Fault；命令根本没送到（窗口正在关、运行环境不在）时没有 Fault，
 * 统一成 transport，界面显示「与 aProxy 的连接断了」，不显示底层给的原文。
 */
export type Failure = Fault | { code: 'transport'; details: None };

/** 收到一个事件之后要做的事。返回的 Promise 兑现（或拒绝）之后才确认这一帧，下一帧随后才会来 */
export type EventHandler = (event: DesktopEvent) => Promise<void>;

export interface Link {
  /**
   * 数据是不是演示用的假数据。界面据此在显眼处常驻标明，免得用户把假的实例当成自己电脑上正在跑的
   */
  readonly demo: boolean;

  /** 发一个命令。失败时以 Failure 拒绝 */
  call<N extends CommandName>(name: N, args: Commands[N][0]): Promise<Commands[N][1]>;

  /**
   * 接上这个窗口的事件通道。第一个事件一定是 resync。整个页面只接一次；
   * 通道出了岔子（帧的编号有缺口、确认发不出去）由实现自己重新接上，重新接上之后的第一个事件又是 resync。
   */
  attach(handler: EventHandler): void;

  /**
   * 当前这次 attach 的标识：详情的选择（request_detail、body_chunk、cancel_detail）要带上它。
   * 还没接上时为 null
   */
  session(): string | null;

  /**
   * 已装的包里一个文件的地址（主题的 theme.css、扩展的 main.mjs 与 assets/ 下的文件），没有这个文件时为 null。
   * 桌面窗口里是 Rust 的 aproxy-pkg 协议：同一个包里的相对地址都能解析。
   *
   * generation 是这一次激活的代号（见 newGeneration），写进地址的第一段（`<id>~<代号>`）。页面按完整地址
   * 缓存模块与样式表，查询串只管得住入口文件本身，相对引用的子模块、字体、图片会解析回旧地址、拿到旧内容；
   * 代号在第一段，相对解析会把它带下去，换一次代号，整个包的每个地址都是新的。Rust 一侧只认 id，代号不影响取到的文件
   */
  packageUrl(id: string, path: string, generation?: string): string | null;
}

let generations = 0;

/**
 * 一个新的激活代号：每次加载扩展、每次换主题样式表都取一个。时间戳让页面重新加载之后也不会和上一次的代号撞上，
 * 计数保证同一毫秒里取的也不同。只用 [0-9a-z-]，长度远小于 Rust 一侧的 64 字节上限
 */
export function newGeneration(): string {
  return `${Date.now().toString(36)}-${(++generations).toString(36)}`;
}

/** 把任意的拒绝值整理成 Failure：形状不对的一律当作连接问题，原文不外传 */
export function toFailure(raw: unknown): Failure {
  if (raw && typeof raw === 'object') {
    const { code, details } = raw as { code?: unknown; details?: unknown };
    if (typeof code === 'string' && details && typeof details === 'object') {
      return raw as Fault;
    }
  }
  return { code: 'transport', details: {} };
}

/** 一个被拒绝的值是不是某个码的 Failure */
export function isFailure<C extends Failure['code']>(raw: unknown, code: C): raw is Extract<Failure, { code: C }> {
  return toFailure(raw).code === code;
}

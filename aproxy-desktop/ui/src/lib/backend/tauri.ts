/*
 * 桌面窗口里的传输层：命令经 Tauri 的 invoke 发给 Rust，事件经一条 Channel 推过来。
 *
 * 事件通道的规矩（Rust 一侧同样按这个做）：
 *   - 一个窗口同时只有一次 attach；新的 attach 顶掉旧的。
 *   - 帧从 1 起连续编号。页面处理完一帧（该重读的都读完了）才 ack，Rust 收到 ack 才发下一帧，
 *     所以页面慢的时候 Rust 那边把积压的通知合并，不会越堆越多。
 *   - 第一帧是 resync，可能比 attach 的返回值先到：先存着，拿到 session 之后再处理。
 *   - 编号对不上、ack 发不出去，都说明这条通道不可靠了：重新 attach，从头再来一次 resync。
 */
import { Channel, convertFileSrc, invoke } from '@tauri-apps/api/core';
import type { CommandName, Commands, Frame } from './contract';
import { toFailure, type EventHandler, type Link } from './link';

/** 重新接上之前等多久：接连失败时不要把 CPU 占满 */
const REATTACH_DELAY_MS = 500;

class TauriLink implements Link {
  readonly demo = false;

  private handler: EventHandler | null = null;
  private current: string | null = null;
  /** 每次 attach 加一：旧通道上迟到的帧靠它认出来丢掉 */
  private epoch = 0;

  async call<N extends CommandName>(name: N, args: Commands[N][0]): Promise<Commands[N][1]> {
    try {
      return (await this.send(name, args)) as Commands[N][1];
    } catch (raw) {
      throw toFailure(raw);
    }
  }

  /**
   * 大多数命令的参数是一个 JSON 对象。传文件内容的三个命令不走 JSON：字节原样作为请求体，
   * 其余参数放进请求头（文件名按 UTF-8 百分号编码，头里只能放 ASCII）；读文件的结果是 ArrayBuffer
   */
  private async send(name: CommandName, args: unknown): Promise<unknown> {
    switch (name) {
      case 'package_check_file': {
        const a = args as Commands['package_check_file'][0];
        return invoke(name, a.bytes, { headers: { 'x-aproxy-file-name': encodeURIComponent(a.name) } });
      }
      case 'extension_file_write': {
        const a = args as Commands['extension_file_write'][0];
        return invoke(name, a.bytes, {
          headers: { 'x-aproxy-extension': a.id, 'x-aproxy-file-name': encodeURIComponent(a.name) },
        });
      }
      case 'extension_file_read':
        return new Uint8Array(await invoke<ArrayBuffer>(name, args as Record<string, unknown>));
      default:
        // 参数的类型由命令表保证；invoke 要的是带索引签名的对象，接口类型不带，这里只是换个说法
        return invoke(name, args as Record<string, unknown>);
    }
  }

  session(): string | null {
    return this.current;
  }

  /**
   * Tauri 把自定义协议在 Windows 上映射成 http://<协议名>.localhost/，别的平台是 <协议名>://localhost/；
   * convertFileSrc 知道这个窗口用的是哪一种。它会把整个参数编码掉（包括 /），所以只交给它 id，
   * 包里的路径逐段编码后接在后面：/ 是分隔符，要原样留着。有激活代号时第一段是 `<id>~<代号>`
   * （convertFileSrc 把 ~ 原样留着或编成 %7E，Rust 一侧两种都认）
   */
  packageUrl(id: string, path: string, generation?: string): string | null {
    const first = generation ? `${id}~${generation}` : id;
    return `${convertFileSrc(first, 'aproxy-pkg')}/${path.split('/').map(encodeURIComponent).join('/')}`;
  }

  attach(handler: EventHandler): void {
    this.handler = handler;
    void this.open();
  }

  private async open(): Promise<void> {
    const epoch = ++this.epoch;
    this.current = null;

    /** attach 返回之前到的帧 */
    const early: Frame[] = [];
    let session: string | null = null;
    /** 下一帧应该是几号 */
    let expected = 1;
    /** 帧一个接一个处理 */
    let queue: Promise<void> = Promise.resolve();

    const fail = () => {
      if (epoch !== this.epoch) return;
      // 让这条通道上后面的帧都被丢掉，再重新接
      this.epoch++;
      this.current = null;
      setTimeout(() => void this.open(), REATTACH_DELAY_MS);
    };

    const process = async (frame: Frame) => {
      if (epoch !== this.epoch || frame.session !== session) return;
      if (frame.seq < expected) return;
      if (frame.seq > expected) return fail();
      expected++;
      try {
        await this.handler?.(frame.event);
      } catch (err) {
        // 重读失败由 store 自己显示；这一帧照样确认，否则通道就停在这里了
        console.warn('[link] event handler failed', err);
      }
      if (epoch !== this.epoch) return;
      try {
        await invoke('ack', { session, seq: frame.seq });
      } catch {
        fail();
      }
    };

    const channel = new Channel<Frame>();
    channel.onmessage = (frame) => {
      if (session === null) {
        early.push(frame);
        return;
      }
      queue = queue.then(() => process(frame));
    };

    try {
      session = await invoke<string>('attach', { channel });
    } catch {
      return fail();
    }
    if (epoch !== this.epoch) return;
    this.current = session;
    for (const frame of early) queue = queue.then(() => process(frame));
  }
}

export const tauriLink: Link = new TauriLink();

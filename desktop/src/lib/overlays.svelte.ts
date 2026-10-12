/*
 * 全局浮层：轻提示（toast）、二次确认对话框、命令面板的开关。
 * 放在模块级状态里，任何页面都能调用，根组件负责渲染。
 */

export interface Toast {
  id: number;
  text: string;
  tone: 'mint' | 'lilac' | 'coral';
}

export interface ConfirmRequest {
  title: string;
  /** 传函数时在对话框打开期间实时求值：「正在进行的 N 个请求」这类数字随时在变，
      用户按下确认时看到的必须是当下的数，而不是打开那一刻的 */
  body: string | (() => string);
  okLabel: string;
  /** 危险操作（停止、强制停止）用珊瑚色确认按钮，吉祥物露出担心的表情 */
  danger: boolean;
  onOk: () => void;
}

class Overlays {
  toasts = $state<Toast[]>([]);
  confirm = $state<ConfirmRequest | null>(null);
  palette = $state(false);
  private id = 0;

  toast(text: string, tone: Toast['tone'] = 'mint') {
    const id = ++this.id;
    this.toasts.push({ id, text, tone });
    setTimeout(() => {
      this.toasts = this.toasts.filter((x) => x.id !== id);
    }, 2800);
  }

  ask(req: ConfirmRequest) {
    this.confirm = req;
  }
}

export const overlays = new Overlays();

/** 复制到剪贴板；浏览器拒绝时（非安全上下文、权限）静默失败，提示照常给出 */
export async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // 原型里没有降级方案的必要；正式版经 Tauri 的剪贴板插件写入
  }
}

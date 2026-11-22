/*
 * 码 → 给人看的一句话：命令失败（Failure）、请求失败或重试的原因（HistoryCode）。
 *
 * Rust 只给稳定的码和几个固定的细节，从不给句子；句子都在界面的字典里（i18n.ts 的 fault.*、code.*），
 * 跟着界面语言走。认不出的码（比新版本的 Rust 少一个枚举值）落到通用的说法上，不显示码本身以外的原文。
 */
import type { HistoryCode } from './backend/contract';
import type { Failure } from './backend/link';
import { hasMessage, t } from './i18n';

/** 一次命令失败的说明 */
export function faultText(f: Failure): string {
  switch (f.code) {
    case 'desktop_unresponsive':
    case 'desktop_force_refused':
    case 'desktop_no_spec':
    case 'desktop_port':
    case 'install_restart_failed':
      return t(`fault.${f.code}`, { port: f.details.port });
    case 'desktop_start': {
      const head = t(f.details.reason === 'spawn' ? 'fault.desktop_start.spawn' : 'fault.desktop_start.notReady');
      return f.details.oldStopped ? `${head} ${t('fault.oldStopped')}` : head;
    }
    case 'desktop_stop':
      return t(`fault.desktop_stop.${f.details.reason}`, { port: f.details.port });
    case 'desktop_state_io':
    case 'desktop_state_reset':
      return t(`fault.${f.code}`, { file: t(`fault.file.${f.details.file}`) });
    case 'desktop_limit':
      return t(`fault.desktop_limit.${f.details.resource}`, { max: f.details.max });
    case 'desktop_registration':
      return t(`fault.desktop_registration.${f.details.reason}`);
    case 'install_foreground_instances':
      return t('fault.install_foreground_instances', { ports: f.details.ports.join(', ') });
    case 'install_precheck_failed':
      return t('fault.install_precheck_failed', { port: f.details.port });
    default: {
      const key = `fault.${f.code}`;
      return hasMessage(key) ? t(key) : t('fault.desktop_unavailable');
    }
  }
}

/** 请求失败或重试的原因；上游的错误状态码直接说出码来 */
export function codeText(code: HistoryCode, status: number | null = null): string {
  if (code === 'upstream_status' && status !== null) return t('code.upstream_status_n', { status });
  const key = `code.${code}`;
  return hasMessage(key) ? t(key) : t('code.unknown');
}

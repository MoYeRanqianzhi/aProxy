/*
 * 码 → 给人看的一句话：命令失败（Failure）、请求失败或重试的原因（HistoryCode）、配置诊断（Issue）。
 *
 * Rust 只给稳定的码和几个固定的细节，从不给句子；句子都在界面的字典里（i18n.ts 的 fault.*、code.*），
 * 跟着界面语言走。认不出的码（比新版本的 Rust 少一个枚举值）落到通用的说法上，不显示码本身以外的原文。
 */
import type { HistoryCode, Issue, PackageIssue } from './backend/contract';
import type { Failure } from './backend/link';
import { bytes, hasMessage, t } from './i18n';

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
    case 'desktop_package':
      return packageText(f.details);
    case 'desktop_format': {
      const key = `fault.desktop_format.${f.details.reason}`;
      return t(hasMessage(key) ? key : 'fault.desktop_format');
    }
    case 'desktop_format_fail':
      // 原因与代码都是 format 作者写的（宿主已打码），照原样放进句子里，不当成翻译的键
      return t('fault.desktop_format_fail', { reason: f.details.reason, code: f.details.code });
    case 'desktop_toml': {
      const i = f.details;
      return t(`fault.desktop_toml.${i.kind}`, { line: i.line ?? '?', column: i.column ?? '?', key: i.key ?? '?' });
    }
    default: {
      const key = `fault.${f.code}`;
      return hasMessage(key) ? t(key) : t('fault.desktop_unavailable');
    }
  }
}

/**
 * 包与扩展数据为什么被拒：按原因选句子，把包里的路径或 manifest 的字段填进去。超出上限时按是哪一项上限选句子，
 * 个数原样说、字节换成 KB/MB。比本版本新的 Rust 给出认不出的原因或上限时，落到不带细节的通用说法上
 */
function packageText(d: PackageIssue): string {
  if (d.reason === 'limit') {
    const max = d.max === null ? '?' : d.limit?.endsWith('-bytes') ? bytes(d.max) : String(d.max);
    const key = `fault.desktop_package.limit.${d.limit}`;
    return t(hasMessage(key) ? key : 'fault.desktop_package.limit', { max });
  }
  const key = `fault.desktop_package.${d.reason}`;
  return hasMessage(key) ? t(key, { path: d.path ?? '?', field: d.field ?? '?' }) : t('fault.desktop_package');
}

/** 请求失败或重试的原因；上游的错误状态码直接说出码来 */
export function codeText(code: HistoryCode, status: number | null = null): string {
  if (code === 'upstream_status' && status !== null) return t('code.upstream_status_n', { status });
  const key = `code.${code}`;
  return hasMessage(key) ? t(key) : t('code.unknown');
}

/** 配置诊断的说明；认不出的码落到「运行 aproxy doctor 查看」上 */
export function issueText(i: Issue): string {
  const key = `diag.${i.code}`;
  return hasMessage(key) ? t(key) : t('diag.unknown');
}

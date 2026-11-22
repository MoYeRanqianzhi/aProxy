/*
 * 演示用的配置文件：每个实例一份，只存在内存里，刷新页面就回到初始值。
 *
 * 行为照真实的来：读到的是脱敏后的投影（密钥只有「设没设」），改动以一组 Edit 传进来，
 * 校验与写回分两步，写回之后要重启实例才生效。
 */
import type {
  Cell,
  ConfigField,
  ConfigView,
  Edit,
  HeaderTable,
  Issue,
  SecondsKey,
  Source,
  TextKey,
  Trigger,
  Validation,
} from '../contract';
import type { Instance } from '../../types';

type Mode = Exclude<Trigger, 'unknown'>;

/** 文件里写了的键。没写的不在这里，读的时候给默认值 */
export interface Doc {
  text: Partial<Record<TextKey, string>>;
  secs: Partial<Record<SecondsKey, number>>;
  trigger: Mode | null;
  headers: Record<HeaderTable, [string, string][]>;
  /** 每写一次加一：读的时候拿到的文件句柄靠它认出「文件已经变了」 */
  version: number;
}

const TEXT_KEYS: TextKey[] = ['base_url', 'listen_addr', 'api_key', 'proxy', 'proxy_username', 'proxy_password', 'keepalive.heartbeat'];
const SECRET_KEYS: TextKey[] = ['api_key', 'proxy_password'];
const SECONDS_KEYS: SecondsKey[] = ['timeouts.connect_secs', 'timeouts.first_byte_secs', 'timeouts.read_idle_secs', 'keepalive.interval_secs'];

const TEXT_DEFAULT: Partial<Record<TextKey, string>> = {
  listen_addr: '127.0.0.1:12345',
  'keepalive.heartbeat': ': keepalive\\n\\n',
};

const SECONDS_DEFAULT: Record<SecondsKey, number> = {
  'timeouts.connect_secs': 30,
  'timeouts.first_byte_secs': 300,
  'timeouts.read_idle_secs': 300,
  'keepalive.interval_secs': 15,
};

/** 按实例的现状生成它的配置文件 */
export function docFor(inst: Instance): Doc {
  const doc: Doc = {
    text: { base_url: inst.baseUrl, listen_addr: inst.listen },
    secs: { 'keepalive.interval_secs': inst.keepalive.interval },
    trigger: null,
    headers: { extra_headers: [], override_headers: [] },
    version: 1,
  };
  if (inst.proxy) doc.text.proxy = inst.proxy;
  if (inst.transform) {
    doc.text.api_key = 'demo';
    doc.headers.extra_headers.push(['http-referer', 'https://github.com/MoYeRanQianZhi/aProxy']);
  }
  return doc;
}

export function fileToken(port: number, doc: Doc): string {
  return `file:${port}:${doc.version}`;
}

function text(s: string): Cell {
  return { kind: 'text', text: s };
}

function fieldsOf(doc: Doc, inst: Instance): ConfigField[] {
  const out: ConfigField[] = [];
  const add = (key: string, value: Cell, source: Source | null, editable = true) => out.push({ key, value, source, header: null, editable });

  for (const key of TEXT_KEYS) {
    const written = doc.text[key];
    if (SECRET_KEYS.includes(key)) {
      add(key, { kind: 'secret', set: !!written }, written ? 'toml' : null);
      continue;
    }
    const fallback = TEXT_DEFAULT[key];
    if (written !== undefined) add(key, text(written), 'toml');
    else if (fallback !== undefined) add(key, text(fallback), 'default');
    else add(key, { kind: 'none' }, null);
  }
  for (const key of SECONDS_KEYS) {
    const written = doc.secs[key];
    add(key, { kind: 'integer', decimal: String(written ?? SECONDS_DEFAULT[key]) }, written === undefined ? 'default' : 'toml');
  }
  add('keepalive.trigger', text(doc.trigger ?? 'any'), doc.trigger ? 'toml' : 'default');

  for (const table of ['extra_headers', 'override_headers'] as const) {
    for (const [name, value] of doc.headers[table]) {
      out.push({ key: `${table}.${name}`, value: text(value), source: 'toml', header: { table, name }, editable: true });
    }
  }

  // 表单改不了的部分只读地列出来：要改它们得去编辑文件
  add('delivery', text(inst.forwardOnly ? 'forward' : 'retry'), 'toml', false);
  add('cache.disk', { kind: 'bool', value: inst.diskCache }, 'toml', false);
  if (inst.boundedPaths.length) {
    add(
      'retry',
      {
        kind: 'list',
        items: inst.boundedPaths.map((p) => ({
          kind: 'map',
          entries: [
            { name: 'path', value: text(p) },
            { name: 'max_attempts', value: { kind: 'integer', decimal: '3' } },
          ],
        })),
      },
      'toml',
      false,
    );
  }
  if (inst.transform) {
    add(
      'format',
      {
        kind: 'list',
        items: [
          {
            kind: 'map',
            entries: [
              { name: 'command', value: text('aproxy-format') },
              { name: 'mode', value: text(inst.transform.mode) },
              { name: 'stages', value: { kind: 'list', items: [text('request'), text('retry')] } },
            ],
          },
        ],
      },
      'toml',
      false,
    );
  }
  return out;
}

export function viewOf(port: number, doc: Doc, inst: Instance): ConfigView {
  return {
    fileToken: fileToken(port, doc),
    file: inst.configPath,
    fields: fieldsOf(doc, inst),
    withheld: [],
    diagnostics: [],
    externalAcl: false,
  };
}

/** 把一组改动落到文件上，返回新的一份；原来的不动（校验只是试算） */
export function applyEdits(doc: Doc, changes: Edit[]): Doc {
  const next: Doc = structuredClone(doc);
  for (const c of changes) {
    switch (c.op) {
      case 'set':
        if (c.key === 'keepalive.trigger') next.trigger = c.value;
        else if (typeof c.value === 'number') next.secs[c.key as SecondsKey] = c.value;
        else next.text[c.key as TextKey] = c.value;
        break;
      case 'unset':
        if (c.key === 'keepalive.trigger') next.trigger = null;
        else {
          delete next.secs[c.key as SecondsKey];
          delete next.text[c.key as TextKey];
        }
        break;
      case 'set-header': {
        const rows = next.headers[c.key];
        const i = rows.findIndex(([n]) => n.toLowerCase() === c.name.toLowerCase());
        if (i >= 0) rows[i] = [c.name, c.value];
        else rows.push([c.name, c.value]);
        break;
      }
      case 'unset-header':
        next.headers[c.key] = next.headers[c.key].filter(([n]) => n.toLowerCase() !== c.name.toLowerCase());
        break;
    }
  }
  return next;
}

/** 监听地址里的端口；写得不对时为 null */
export function listenPort(addr: string | undefined): number | null {
  const m = /:(\d{1,5})$/.exec(addr ?? '');
  const port = m ? Number(m[1]) : NaN;
  return port >= 1 && port <= 65535 ? port : null;
}

/** 检查改动之后的文件。occupied 说的是「这个端口上有没有别的实例」 */
export function validate(doc: Doc, file: string, occupied: (port: number) => boolean): Validation {
  const diagnostics: Issue[] = [];
  const bad = (key: string, code: Issue['code']) => diagnostics.push({ code, severity: 'error', key, file, line: null, column: null });

  const base = doc.text.base_url;
  if (base === undefined || base === '') bad('base_url', 'config_missing_key');
  else {
    try {
      if (!/^https?:$/.test(new URL(base).protocol)) bad('base_url', 'config_invalid_value');
    } catch {
      bad('base_url', 'config_invalid_value');
    }
  }

  const listen = doc.text.listen_addr ?? TEXT_DEFAULT.listen_addr;
  const port = listenPort(listen);
  if (port === null) bad('listen_addr', 'config_invalid_value');
  else if (!/^(127\.0\.0\.1|localhost|\[::1\]):/.test(listen!)) {
    diagnostics.push({ code: 'listen_not_loopback', severity: 'warning', key: 'listen_addr', file, line: null, column: null });
  }

  for (const key of SECONDS_KEYS) {
    const v = doc.secs[key];
    if (v !== undefined && (!Number.isInteger(v) || v < 0)) bad(key, 'config_invalid_value');
  }

  const occupiedPort = port !== null && occupied(port) ? port : null;
  return {
    canSave: occupiedPort === null && !diagnostics.some((d) => d.severity === 'error'),
    diagnostics,
    occupiedPort,
    externalAcl: false,
  };
}

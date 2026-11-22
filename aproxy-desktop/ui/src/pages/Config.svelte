<!--
  配置：一份配置文件的表单。

  读到的是 Rust 给的投影（read_config）：每个键一项，带着值从哪来（文件、settings.json 的全局默认、
  环境变量、启动参数、内置默认）。密钥只说「设没设」，原值永远不会到界面里来。
  表单能改的只有几类键：上游地址、监听地址、代理与它的账号、API key、三个超时、保活、两张请求头表。
  其余的只读列出，要改得去编辑文件。Rust 只接受这几类改动（Edit），文件里的注释和排版原样保留。

  保存分两步：先把改动交给 Rust 试算（validate_config），把改动、问题和需要用户确认的事摆进一个对话框；
  用户确认后才写（write_config）。写回只改文件、不替用户重启：正在用这个文件的实例要重启才会用上新配置，
  保存后页面列出它们，并给每个一个重启按钮。

  目标有两种：
    正在运行的实例：按它这次运行实际用的那个文件。写的时候带上读时拿到的文件句柄，
                    文件在这期间被别处改过，Rust 会拒绝（desktop_changed），页面重读一遍。
    list_configs 给出的一项配置：还没在运行的别名、配置目录里的文件或默认配置。
-->
<script lang="ts">
  import { t, hasMessage } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import { pop, fadeOut } from '../lib/motion';
  import type { Cell, Choice, ConfigField, ConfigTarget, ConfigView, Edit, HeaderTable, SecondsKey, Source, TextKey, Trigger, Validation } from '../lib/backend/contract';
  import { toFailure, type Failure } from '../lib/backend/link';
  import { faultText, issueText } from '../lib/faults';
  import { aliasOf } from '../lib/view';
  import Icon, { type IconName } from '../components/Icon.svelte';
  import Dropdown from '../components/Dropdown.svelte';
  import Segmented from '../components/Segmented.svelte';

  // ---------------------------------------------------------------------------
  // 键的分类：决定一个字段用哪种输入框、改动编成哪种 Edit
  // ---------------------------------------------------------------------------

  /** 明文的文本键：输入框里显示当前值，可以直接改 */
  const PLAIN_KEYS: TextKey[] = ['base_url', 'listen_addr', 'proxy', 'keepalive.heartbeat'];
  /** 密钥：只知道设没设。只能换成新值或者删掉，看不到原值 */
  const SECRET_KEYS: TextKey[] = ['api_key', 'proxy_username', 'proxy_password'];
  const SECONDS_KEYS: SecondsKey[] = ['timeouts.connect_secs', 'timeouts.first_byte_secs', 'timeouts.read_idle_secs', 'keepalive.interval_secs'];
  const TABLES: HeaderTable[] = ['extra_headers', 'override_headers'];
  type Mode = Exclude<Trigger, 'unknown'>;

  const isPlain = (k: string): k is TextKey => (PLAIN_KEYS as string[]).includes(k);
  const isSecret = (k: string): k is TextKey => (SECRET_KEYS as string[]).includes(k);
  const isSeconds = (k: string): k is SecondsKey => (SECONDS_KEYS as string[]).includes(k);

  /** 表单的分组；最后一组「其他」收下所有前面没点名的键（只读） */
  const GROUPS: { id: 'upstream' | 'network' | 'timeouts' | 'keepalive' | 'other'; icon: IconName; keys: string[] | null }[] = [
    { id: 'upstream', icon: 'globe', keys: ['base_url', 'api_key', 'extra_headers', 'override_headers'] },
    { id: 'network', icon: 'server', keys: ['listen_addr', 'proxy', 'proxy_username', 'proxy_password'] },
    { id: 'timeouts', icon: 'clock', keys: ['timeouts.connect_secs', 'timeouts.first_byte_secs', 'timeouts.read_idle_secs'] },
    { id: 'keepalive', icon: 'heart', keys: ['keepalive.interval_secs', 'keepalive.trigger', 'keepalive.heartbeat'] },
    { id: 'other', icon: 'file', keys: null },
  ];
  const NAMED = new Set(GROUPS.flatMap((g) => g.keys ?? []));

  /** 显示值里的打码标记（与 Rust 认的一致）：带着它写回去会被拒绝，原来的密钥也会丢 */
  const marked = (s: string) => s.includes('***') || s.includes('<secret>') || s.includes('<secret ');
  /** 一个值在输入框里的样子：文本、整数原样，其他（没设、密钥、表）是空串 */
  function plain(c: Cell | undefined): string {
    if (!c) return '';
    if (c.kind === 'text') return c.text;
    if (c.kind === 'integer') return c.decimal;
    return '';
  }
  const HEARTBEAT = 'keepalive.heartbeat';
  // 心跳内容里常有换行，单行输入框装不下（浏览器会把换行吞掉）：输入框里换行写成 \n、反斜杠写成 \\，写回时还原
  const escapeText = (s: string) => s.replace(/\\/g, '\\\\').replace(/\r/g, '\\r').replace(/\n/g, '\\n');
  const unescapeText = (s: string) => s.replace(/\\([\\nr])/g, (_, c: string) => (c === 'n' ? '\n' : c === 'r' ? '\r' : '\\'));
  /** 一个值在输入框里的样子 */
  const shown = (k: string, c: Cell | undefined) => (k === HEARTBEAT ? escapeText(plain(c)) : plain(c));
  /** 草稿规整成要写进文件的值：心跳内容原样（空格也是内容），其余去掉首尾空白 */
  const valueOf = (k: string, draft: string | undefined) => (k === HEARTBEAT ? unescapeText(draft ?? '') : (draft ?? '').trim());

  /** 文件里写的是 body_stream，契约里是 body-stream */
  const modeOf = (c: Cell | undefined): Mode => {
    const m = plain(c).replace('_', '-');
    return m === 'accept' || m === 'body-stream' ? m : 'any';
  };

  // ---------------------------------------------------------------------------
  // 目标：哪份配置
  // ---------------------------------------------------------------------------

  let choices = $state.raw<Choice[]>([]);
  /** 当前读的目标；跟随运行的实例时不带文件句柄，写的时候再加上 */
  let target = $state.raw<ConfigTarget | null>(null);
  let config = $state.raw<ConfigView | null>(null);
  let loadFault = $state.raw<Failure | null>(null);
  /** 下拉框里的选择：run:<端口> 或 cfg:<序号> */
  let pick = $state('');
  let picked = '';

  const runningCards = $derived(store.cards.filter((c) => c.run));
  /** 没在运行的配置：正在运行的实例已经各占一项，同一个文件不重复列 */
  const idleChoices = $derived(choices.filter((c) => !runningCards.some((card) => card.metadata?.configPath === c.path)));
  const options = $derived([
    ...runningCards.map((c) => {
      const alias = aliasOf(c);
      return { value: `run:${c.port}`, label: alias ? `${c.port} ${alias}` : String(c.port), hint: t('cfg.running'), dot: 'mint' };
    }),
    ...idleChoices.map((c) => ({ value: `cfg:${choices.indexOf(c)}`, label: c.label, hint: c.path })),
  ]);
  const currentChoice = $derived(pick.startsWith('cfg:') ? choices[Number(pick.slice(4))] : undefined);

  // 页面按端口挂载（App 里的 {#key}）：换到另一个运行中的实例会重建本页，所以初始目标只定一次。
  // 等第一份快照到了才知道这个端口上有没有运行中的实例
  // 只在模板里读（「还没有配置」与「正在读取」的区分），所以要是响应式的
  let started = $state(false);
  $effect(() => {
    if (started || !store.connected) return;
    started = true;
    void begin();
  });

  async function begin() {
    try {
      choices = await store.listConfigs();
    } catch (raw) {
      loadFault = toFailure(raw);
    }
    const card = store.card(ui.instancePort);
    if (card?.run) choose(`run:${card.port}`);
    else {
      const i = choices.findIndex((c) => c.isDefault);
      if (choices.length) choose(`cfg:${i >= 0 ? i : 0}`);
    }
  }

  function choose(value: string) {
    pick = value;
    picked = value;
    if (value.startsWith('run:')) {
      const card = store.card(Number(value.slice(4)));
      if (!card?.run) return;
      // 换到另一个实例走路由：地址栏跟着变，返回键也认得
      if (card.port !== ui.instancePort) return ui.go('config', card.port);
      void load({ kind: 'run', run: card.run });
    } else {
      const c = choices[Number(value.slice(4))];
      if (c) void load(c.target);
    }
  }

  $effect(() => {
    if (pick && pick !== picked) choose(pick);
  });

  async function load(next: ConfigTarget) {
    target = next;
    try {
      const v = await store.readConfig(next);
      if (target !== next) return;
      config = v;
      loadFault = null;
      fill(v);
    } catch (raw) {
      if (target === next) loadFault = toFailure(raw);
    }
  }

  /** 写回用的目标：跟随运行时带上读到的文件句柄，读完之后文件被别处改过就不写 */
  function writeTarget(): ConfigTarget | null {
    if (!target) return null;
    return target.kind === 'run' && config?.fileToken ? { kind: 'run', run: target.run, expectedFile: config.fileToken } : target;
  }

  // ---------------------------------------------------------------------------
  // 草稿
  // ---------------------------------------------------------------------------

  /** 明文与秒数的草稿，键是配置键 */
  let texts = $state<Record<string, string>>({});
  /** 密钥的新值；空串是「不改」 */
  let secrets = $state<Record<string, string>>({});
  /** 要从文件里删掉的密钥 */
  let cleared = $state<Record<string, boolean>>({});
  let mode = $state<Mode>('any');

  interface HeaderRow {
    id: number;
    table: HeaderTable;
    /** 文件里已有的头：名字定死，值看不到，只能换值或删掉 */
    existing: boolean;
    name: string;
    /** 新值；已有的头留空是「不改」 */
    value: string;
    removed: boolean;
  }
  let headers = $state<HeaderRow[]>([]);
  let rowId = 0;

  const field = (key: string): ConfigField | undefined => config?.fields.find((f) => f.key === key && !f.header);

  function fill(v: ConfigView) {
    const next: Record<string, string> = {};
    for (const f of v.fields) if (!f.header && (isPlain(f.key) || isSeconds(f.key))) next[f.key] = shown(f.key, f.value);
    texts = next;
    secrets = {};
    cleared = {};
    mode = modeOf(v.fields.find((f) => f.key === 'keepalive.trigger')?.value);
    headers = v.fields
      .filter((f) => f.header)
      .map((f) => ({ id: ++rowId, table: f.header!.table, existing: true, name: f.header!.name, value: '', removed: false }));
  }

  function discard() {
    if (config) fill(config);
  }

  function addHeader(table: HeaderTable) {
    headers.push({ id: ++rowId, table, existing: false, name: '', value: '', removed: false });
  }

  /** 表头里名字带着密钥的项（Rust 不给名字、也不让改）：只读列出，让用户知道它在 */
  const lockedHeaders = (table: HeaderTable) => config?.fields.filter((f) => f.key.startsWith(`${table}.`) && !f.header) ?? [];

  // ---------------------------------------------------------------------------
  // 校验与改动
  // ---------------------------------------------------------------------------

  const HEADER_NAME = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;

  const errors = $derived.by(() => {
    const e: Record<string, string> = {};
    for (const [k, v] of Object.entries(texts)) {
      const s = valueOf(k, v);
      if (isSeconds(k) && s !== '' && (!/^\d+$/.test(s) || Number(s) > Number.MAX_SAFE_INTEGER)) e[k] = t('cfg.err.int');
      else if (isPlain(k) && s !== plain(field(k)?.value) && marked(s)) e[k] = t('cfg.err.marked');
    }
    for (const [k, v] of Object.entries(secrets)) if (marked(v)) e[k] = t('cfg.err.marked');
    for (const r of headers) {
      if (r.removed) continue;
      const id = `h${r.id}`;
      if (!r.existing) {
        if (!r.name && !r.value) continue;
        if (!HEADER_NAME.test(r.name)) e[id] = t('cfg.err.headerName');
        else if (headers.some((o) => o !== r && !o.removed && o.table === r.table && o.name.toLowerCase() === r.name.toLowerCase())) e[id] = t('cfg.err.headerDup');
        else if (!r.value) e[id] = t('cfg.err.headerValue');
      }
      if (!e[id] && (/[\r\n]/.test(r.value) || marked(r.value))) e[id] = t(marked(r.value) ? 'cfg.err.marked' : 'cfg.err.headerValue');
    }
    return e;
  });
  const hasErrors = $derived(Object.keys(errors).length > 0);

  /**
   * 草稿和读到的值算不算不同。值不是文件里写的（默认、全局默认）而草稿清空了：
   * 文件里本来就没有这一项，没有可删的，不算改动
   */
  function differs(f: ConfigField | undefined, draft: string): boolean {
    if (!f) return false;
    if (draft === '' && f.source !== 'toml') return false;
    return draft !== plain(f.value);
  }

  /** 草稿和读到的值之间的差，编成 Rust 认的改动；没改的（尤其是密钥）不传 */
  const edits = $derived.by((): Edit[] => {
    if (!config) return [];
    const out: Edit[] = [];
    for (const f of config.fields) {
      if (!f.editable || f.header) continue;
      const k = f.key;
      if (isSecret(k)) {
        if (cleared[k]) out.push({ op: 'unset', key: k });
        else if (secrets[k]) out.push({ op: 'set', key: k, value: secrets[k] });
      } else if (isSeconds(k)) {
        const s = valueOf(k, texts[k]);
        if (!differs(f, s) || errors[k]) continue;
        out.push(s === '' ? { op: 'unset', key: k } : { op: 'set', key: k, value: Number(s) });
      } else if (isPlain(k)) {
        const s = valueOf(k, texts[k]);
        if (!differs(f, s) || errors[k]) continue;
        out.push(s === '' ? { op: 'unset', key: k } : { op: 'set', key: k, value: s });
      } else if (k === 'keepalive.trigger' && mode !== modeOf(f.value)) {
        out.push({ op: 'set', key: 'keepalive.trigger', value: mode });
      }
    }
    for (const r of headers) {
      if (r.existing && r.removed) out.push({ op: 'unset-header', key: r.table, name: r.name });
      else if (!r.removed && r.value && r.name && !errors[`h${r.id}`]) out.push({ op: 'set-header', key: r.table, name: r.name, value: r.value });
    }
    return out;
  });
  /** 草稿里动过的地方（含还有错、编不成改动的），保存条按它出现 */
  const touched = $derived.by(() => {
    if (!config) return 0;
    let n = 0;
    for (const [k, v] of Object.entries(texts)) if (differs(field(k), valueOf(k, v))) n++;
    n += Object.values(secrets).filter(Boolean).length + Object.values(cleared).filter(Boolean).length;
    if (mode !== modeOf(field('keepalive.trigger')?.value)) n++;
    n += headers.filter((r) => (r.existing ? r.removed || r.value : r.name || r.value)).length;
    return n;
  });

  /** 文件读不懂（语法错）：Rust 给的字段全是空的，表单无从改起 */
  const unreadable = $derived(config?.diagnostics.some((d) => d.code === 'config_syntax') ?? false);

  // ---------------------------------------------------------------------------
  // 保存
  // ---------------------------------------------------------------------------

  let reviewing = $state(false);
  let checking = $state(false);
  let saving = $state(false);
  let check = $state.raw<Validation | null>(null);
  let saveFault = $state.raw<Failure | null>(null);
  /** 文件在主目录之外时，用户确认过「我知道，照样保存」 */
  let ack = $state(false);
  /** 刚保存过、要重启才用上新配置的实例 */
  let restartPorts = $state.raw<number[]>([]);

  async function review() {
    const to = writeTarget();
    if (!to) return;
    checking = true;
    try {
      check = await store.validateConfig(to, edits);
      ack = false;
      saveFault = null;
      reviewing = true;
    } catch (raw) {
      await failed(toFailure(raw));
    } finally {
      checking = false;
    }
  }

  async function save() {
    const to = writeTarget();
    if (!to || !target) return;
    saving = true;
    try {
      const r = await store.writeConfig(to, edits, ack);
      reviewing = false;
      overlays.toast(r.changed ? t('toast.saved', { path: config?.file ?? '' }) : t('cfg.unchanged'));
      restartPorts = r.instancesToRestart;
      await load(target);
    } catch (raw) {
      const f = toFailure(raw);
      // 写的时候 Rust 又查了一遍：没过就把它说的问题摆进对话框，不关
      if (f.code === 'desktop_config' && check) check = { ...check, canSave: false, diagnostics: f.details.diagnostics };
      else await failed(f);
    } finally {
      saving = false;
    }
  }

  async function failed(f: Failure) {
    if (f.code === 'desktop_changed' && target) {
      // 文件在读了之后被别处改过：草稿是按旧内容算的，不能硬写，重读一遍让用户再改
      reviewing = false;
      overlays.toast(t('cfg.changedElsewhere'), 'coral');
      await load(target);
    } else if (reviewing) saveFault = f;
    else overlays.toast(faultText(f), 'coral');
  }

  /** 正在重启的端口：重启要等新进程就绪，期间按钮不能再点 */
  let restarting = $state.raw<number[]>([]);

  async function restart(port: number) {
    const card = store.card(port);
    if (!card?.run) return;
    restarting = [...restarting, port];
    try {
      await store.restart(card.run);
      restartPorts = restartPorts.filter((p) => p !== port);
      overlays.toast(t('toast.restarted', { name: aliasOf(card) ?? port }));
    } catch (raw) {
      overlays.toast(faultText(toFailure(raw)), 'coral');
    } finally {
      restarting = restarting.filter((p) => p !== port);
    }
  }

  /** 对话框里一处改动的说法；密钥只说「换成新值」，值不出现在屏幕上 */
  function changeText(e: Edit): { key: string; text: string } {
    switch (e.op) {
      case 'set':
        return { key: e.key, text: isSecret(e.key) ? t('cfg.ch.secret') : t('cfg.ch.set', { value: (e.key as string) === HEARTBEAT ? escapeText(String(e.value)) : String(e.value) }) };
      case 'unset':
        return { key: e.key, text: t('cfg.ch.unset') };
      case 'set-header':
        return { key: `${e.key}.${e.name}`, text: t('cfg.ch.secret') };
      case 'unset-header':
        return { key: `${e.key}.${e.name}`, text: t('cfg.ch.unset') };
    }
  }

  // ---------------------------------------------------------------------------
  // 显示
  // ---------------------------------------------------------------------------

  /** 字段名：认得的键用字典里的叫法，认不得的（比界面新的 Rust 多出来的键）直接显示键本身 */
  function label(key: string): string {
    const k = `cfg.k.${key}`;
    return hasMessage(k) ? t(k) : key;
  }
  function help(key: string): string {
    const k = `cfg.kh.${key}`;
    return hasMessage(k) ? t(k) : '';
  }
  /**
   * 值不是从文件里来的：说一声从哪来。启动覆盖压过文件（改文件要等下次不带覆盖启动）；
   * 环境变量只有代理一项，而且只在文件没写代理时才用，所以在这里填了就以文件为准
   */
  const sourceTone: Record<Source, string> = { toml: '', settings: 'sky', default: '', env: 'butter', override: 'butter' };

  /** 「其他」组：前面几组没点名的键；请求头表的每一项归请求头表管 */
  const others = $derived(config?.fields.filter((f) => !f.header && !NAMED.has(f.key) && !TABLES.some((tb) => f.key.startsWith(`${tb}.`))) ?? []);

  /** 字段对应的读时问题（Rust 给了键的） */
  const issuesOf = (key: string) => config?.diagnostics.filter((d) => d.key === key) ?? [];

  let activeGroup = $state('upstream');
  let formEl: HTMLDivElement | undefined = $state();

  // 滚动时让左侧分组导航跟着高亮：哪个分组的标题进入上部三分之一，就算当前分组
  $effect(() => {
    if (!formEl) return;
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) if (e.isIntersecting) activeGroup = e.target.id.replace(/^g-/, '');
      },
      { rootMargin: '0px 0px -66% 0px' },
    );
    formEl.querySelectorAll('.group').forEach((g) => io.observe(g));
    return () => io.disconnect();
  });

  function jump(id: string) {
    activeGroup = id;
    formEl?.querySelector(`#g-${id}`)?.scrollIntoView({ behavior: ui.reducedMotion ? 'auto' : 'smooth', block: 'start' });
  }
</script>

<!-- 只读的值：文本与数字原样，表按「名字 = 值」逐行，列表逐项 -->
{#snippet cellView(c: Cell)}
  {#if c.kind === 'none'}
    <span class="muted">{t('cfg.empty')}</span>
  {:else if c.kind === 'secret'}
    <span class="chip" class:mint={c.set}><Icon name="key" size={11} stroke={2.4} />{c.set ? t('cfg.secretSet') : t('cfg.secretUnset')}</span>
  {:else if c.kind === 'text'}
    <code class="mono val">{c.text}</code>
  {:else if c.kind === 'integer'}
    <code class="mono val">{c.decimal}</code>
  {:else if c.kind === 'bool'}
    <span class="chip" class:mint={c.value}>{c.value ? t('cfg.yes') : t('cfg.no')}</span>
  {:else if c.kind === 'list'}
    {#if c.items.length === 0}
      <span class="muted">{t('cfg.emptyList')}</span>
    {:else}
      <ol class="cells">
        {#each c.items as item, i (i)}<li>{@render cellView(item)}</li>{/each}
      </ol>
    {/if}
  {:else if c.entries.length === 0}
    <span class="muted">{t('cfg.emptyList')}</span>
  {:else}
    <dl class="cells map">
      {#each c.entries as e, i (i)}
        <dt class="mono">{e.name}</dt>
        <dd>{@render cellView(e.value)}</dd>
      {/each}
    </dl>
  {/if}
{/snippet}

{#snippet lbl(f: ConfigField)}
  {@const changedHere = edits.some((e) => ('key' in e ? e.key : '') === f.key && (e.op === 'set' || e.op === 'unset'))}
  <div class="lbl">
    <div class="lbl-top">
      <!-- 输入框靠 aria-labelledby 指回这里取得可读的名字（字段名在左栏，不和输入框嵌套） -->
      <span class="name" id="lbl-{f.key}">{label(f.key)}</span>
      {#if f.source && f.source !== 'toml'}
        <span class="tag {sourceTone[f.source]}">{t(`cfg.src.${f.source}`)}</span>
      {/if}
      {#if changedHere}<span class="dirty-dot" title={t('cfg.dirty', { n: 1 })}></span>{/if}
    </div>
    <code class="mono tkey">{f.key}</code>
    {#if help(f.key)}<p class="help">{help(f.key)}</p>{/if}
  </div>
{/snippet}

{#snippet notes(f: ConfigField)}
  {#if errors[f.key]}
    <span class="msg error" in:pop={{ y: -4, scale: 1, duration: 300 }}><Icon name="alert" size={13} stroke={2.4} />{errors[f.key]}</span>
  {/if}
  {#if f.source === 'env' || f.source === 'override'}
    <span class="msg warn"><Icon name="info" size={13} stroke={2.4} />{t(`cfg.src.${f.source}.warn`)}</span>
  {/if}
  {#each issuesOf(f.key) as d, i (i)}
    <span class="msg {d.severity === 'error' ? 'error' : 'warn'}"><Icon name="alert" size={13} stroke={2.4} />{issueText(d)}</span>
  {/each}
{/snippet}

{#snippet editor(f: ConfigField)}
  {#if !f.editable}
    <div class="ctl">{@render cellView(f.value)}</div>
  {:else if isSecret(f.key)}
    {@const set = f.value.kind === 'secret' && f.value.set}
    <div class="ctl">
      <div class="secret-row">
        {#if cleared[f.key]}
          <span class="chip coral"><Icon name="minus" size={11} stroke={2.4} />{t('cfg.secretWillClear')}</span>
          <button class="btn sm ghost" onclick={() => (cleared[f.key] = false)}><Icon name="undo" size={13} />{t('cfg.secretKeep')}</button>
        {:else}
          <span class="chip" class:mint={set}><Icon name="key" size={11} stroke={2.4} />{set ? t('cfg.secretSet') : t('cfg.secretUnset')}</span>
          <input
            class="input mono"
            class:invalid={!!errors[f.key]}
            type="password"
            autocomplete="off"
            spellcheck="false"
            placeholder={set ? t('cfg.secretReplace') : t('cfg.secretNew')}
            aria-labelledby="lbl-{f.key}"
            bind:value={secrets[f.key]}
          />
          {#if set && f.source === 'toml'}
            <button class="btn sm ghost" onclick={() => ((cleared[f.key] = true), (secrets[f.key] = ''))}>{t('cfg.secretClear')}</button>
          {/if}
        {/if}
      </div>
      {@render notes(f)}
    </div>
  {:else if isSeconds(f.key)}
    <div class="ctl">
      <div class="num-wrap">
        <input class="input mono" class:invalid={!!errors[f.key]} inputmode="numeric" aria-labelledby="lbl-{f.key}" bind:value={texts[f.key]} aria-invalid={!!errors[f.key]} />
        <span class="unit">{t('cfg.seconds')}</span>
      </div>
      {#if (texts[f.key] ?? '').trim() === '' && f.source === 'toml'}<span class="msg warn">{t('cfg.unsetHint')}</span>{/if}
      {@render notes(f)}
    </div>
  {:else if f.key === 'keepalive.trigger'}
    <div class="ctl">
      <Segmented
        options={[
          { value: 'any', label: t('cfg.o.any') },
          { value: 'accept', label: t('cfg.o.accept') },
          { value: 'body-stream', label: t('cfg.o.body_stream') },
        ]}
        bind:value={mode}
        label={label(f.key)}
        size="sm"
      />
      {@render notes(f)}
    </div>
  {:else}
    <div class="ctl">
      <input
        class="input mono"
        class:invalid={!!errors[f.key]}
        spellcheck="false"
        autocomplete="off"
        placeholder={t('cfg.empty')}
        aria-labelledby="lbl-{f.key}"
        aria-invalid={!!errors[f.key]}
        bind:value={texts[f.key]}
      />
      {#if (texts[f.key] ?? '').trim() === '' && f.source === 'toml'}<span class="msg warn">{t('cfg.unsetHint')}</span>{/if}
      {@render notes(f)}
    </div>
  {/if}
{/snippet}

{#snippet headerTable(table: HeaderTable)}
  {@const f = field(table)}
  {@const rows = headers.filter((r) => r.table === table)}
  {@const locked = lockedHeaders(table)}
  <div class="field">
    <div class="lbl">
      <div class="lbl-top"><span class="name">{label(table)}</span></div>
      <code class="mono tkey">{table}</code>
      {#if help(table)}<p class="help">{help(table)}</p>{/if}
    </div>
    <div class="ctl">
      {#if !rows.length && !locked.length}<span class="muted">{t('cfg.headers.none')}</span>{/if}
      <ul class="headers">
        {#each rows as r (r.id)}
          <li class:removed={r.removed}>
            {#if r.existing}
              <code class="mono h-name">{r.name}</code>
              {#if r.removed}
                <span class="chip coral"><Icon name="minus" size={11} stroke={2.4} />{t('cfg.secretWillClear')}</span>
                <span class="grow"></span>
                <button class="btn sm ghost" onclick={() => (r.removed = false)}><Icon name="undo" size={13} />{t('cfg.secretKeep')}</button>
              {:else}
                <input class="input mono" type="password" autocomplete="off" placeholder={t('cfg.headers.replace')} aria-label={t('cfg.headers.valueOf', { name: r.name })} bind:value={r.value} />
                <button class="btn sm icon ghost" onclick={() => ((r.removed = true), (r.value = ''))} aria-label={t('cfg.remove')}><Icon name="x" size={13} stroke={2.6} /></button>
              {/if}
            {:else}
              <input class="input mono h-name" spellcheck="false" autocomplete="off" placeholder={t('cfg.headers.name')} aria-label={t('cfg.headers.name')} bind:value={r.name} />
              <input class="input mono" type="password" autocomplete="off" placeholder={t('cfg.headers.value')} aria-label={t('cfg.headers.value')} bind:value={r.value} />
              <button class="btn sm icon ghost" onclick={() => headers.splice(headers.indexOf(r), 1)} aria-label={t('cfg.remove')}><Icon name="x" size={13} stroke={2.6} /></button>
            {/if}
          </li>
          {#if errors[`h${r.id}`]}<li class="h-err"><span class="msg error"><Icon name="alert" size={13} stroke={2.4} />{errors[`h${r.id}`]}</span></li>{/if}
        {/each}
        {#each locked as l (l.key)}
          <li class="locked"><code class="mono h-name">{l.key.slice(table.length + 1)}</code><span class="muted">{t('cfg.headers.locked')}</span></li>
        {/each}
      </ul>
      {#if f?.source && f.source !== 'toml'}<span class="msg warn"><Icon name="info" size={13} stroke={2.4} />{t('cfg.headers.inherited', { from: t(`cfg.src.${f.source}`) })}</span>{/if}
      <button class="btn sm add" onclick={() => addHeader(table)}><Icon name="plus" size={13} stroke={2.6} />{t('cfg.headers.add')}</button>
    </div>
  </div>
{/snippet}

<header class="head">
  <div class="title-row">
    <h1>{t('nav.config')}</h1>
    {#if options.length}
      <Dropdown {options} bind:value={pick} label={t('cfg.target')} icon="file" />
    {/if}
    {#if config}
      <span class="file"><Icon name="file" size={14} /><code class="mono">{config.file}</code></span>
    {/if}
  </div>
  {#if currentChoice && !currentChoice.exists}
    <p class="hint"><Icon name="info" size={14} />{t('cfg.fileMissing')}</p>
  {/if}
</header>

{#if restartPorts.length}
  <div class="restart-banner" role="status" in:pop={{ y: -6, scale: 1 }}>
    <Icon name="restart" size={16} />
    <p>{t('cfg.restartNeeded')}</p>
    {#each restartPorts as port (port)}
      {@const card = store.card(port)}
      <button class="btn sm" disabled={!card?.run || restarting.includes(port)} onclick={() => restart(port)}><Icon name="restart" size={13} />{t('cfg.restartOne', { port })}</button>
    {/each}
    <button class="btn sm icon ghost" onclick={() => (restartPorts = [])} aria-label={t('confirm.cancel')}><Icon name="x" size={13} stroke={2.6} /></button>
  </div>
{/if}

{#if loadFault && !config}
  <div class="banner coral"><Icon name="alert" size={16} /><p>{faultText(loadFault)}</p></div>
{:else if !config}
  <p class="none">{store.connected && !options.length && started ? t('cfg.noTarget') : t('cfg.loading')}</p>
{:else}
  {#if config.diagnostics.length || config.withheld.length}
    <section class="problems" aria-label={t('cfg.problems')}>
      {#each config.diagnostics as d, i (i)}
        <p class="sev-{d.severity}">
          <Icon name={d.severity === 'info' ? 'info' : 'alert'} size={14} stroke={2.4} />
          <span>{issueText(d)}</span>
          {#if d.key}<code class="mono">{d.key}</code>{/if}
          {#if d.line !== null}<span class="muted num">{t('cfg.line', { n: d.line })}</span>{/if}
        </p>
      {/each}
      {#each config.withheld as w (w.key)}
        <p class="sev-warning">
          <Icon name="key" size={14} stroke={2.4} />
          <span>{t(`cfg.withheld.${w.reason}`, { key: w.key, from: w.defaultTarget, to: w.target })}</span>
        </p>
      {/each}
    </section>
  {/if}

  {#if !unreadable}
    <div class="layout">
      <nav class="groups" aria-label={t('nav.config')}>
        {#each GROUPS as g (g.id)}
          <button class:on={activeGroup === g.id} onclick={() => jump(g.id)}>
            <Icon name={g.icon} size={16} />{t(`cfg.group.${g.id}`)}
          </button>
        {/each}
      </nav>

      <div class="form" bind:this={formEl}>
        {#each GROUPS as g (g.id)}
          <!-- 点名的键按顺序排；Rust 没给的跳过。请求头表总是画出来：表是空的、文件里没写它时也要能添加 -->
          {@const fields = g.keys ? g.keys.map((k) => field(k) ?? k) : others}
          <section class="group" id="g-{g.id}">
            <h2><Icon name={g.icon} size={17} />{t(`cfg.group.${g.id}`)}</h2>
            {#if g.id === 'other'}<p class="group-desc">{t('cfg.other.desc')}</p>{/if}
            {#each fields as f (typeof f === 'string' ? f : f.key)}
              {#if typeof f === 'string' ? TABLES.includes(f as HeaderTable) : TABLES.includes(f.key as HeaderTable)}
                {@render headerTable((typeof f === 'string' ? f : f.key) as HeaderTable)}
              {:else if typeof f !== 'string'}
                <div class="field">
                  {@render lbl(f)}
                  {@render editor(f)}
                </div>
              {/if}
            {/each}
          </section>
        {/each}
      </div>
    </div>
  {/if}

  {#if touched > 0}
    <div class="savebar" in:pop={{ y: 30, scale: 0.96, duration: 560 }} out:fadeOut>
      <span class="dirty-dot big"></span>
      <span class="sb-text">{t('cfg.dirty', { n: touched })}</span>
      {#if hasErrors}<span class="msg error inline"><Icon name="alert" size={13} stroke={2.4} />{t('cfg.err.fix')}</span>{/if}
      <span class="grow"></span>
      <button class="btn sm ghost" onclick={discard}><Icon name="undo" size={14} />{t('cfg.discard')}</button>
      <button class="btn sm primary" disabled={hasErrors || checking || edits.length === 0} onclick={review}>
        <Icon name="eye" size={14} />{checking ? t('cfg.checking') : t('cfg.review')}
      </button>
    </div>
  {/if}

  {#if reviewing && check}
    <div class="scrim" in:fadeOut={{ duration: 160 }} out:fadeOut={{ duration: 120 }} onclick={() => (reviewing = false)} aria-hidden="true"></div>
    <div class="layer">
      <div class="review" role="dialog" aria-modal="true" aria-labelledby="review-title" in:pop={{ y: 18, scale: 0.94, duration: 560 }} out:fadeOut={{ duration: 120 }}>
        <h2 id="review-title">{t('cfg.diff.title')}</h2>
        <div class="file small"><Icon name="file" size={13} /><code class="mono">{config.file}</code></div>
        <ul class="changes">
          {#each edits as e, i (i)}
            {@const c = changeText(e)}
            <li class:del={e.op === 'unset' || e.op === 'unset-header'}><code class="mono">{c.key}</code><span>{c.text}</span></li>
          {/each}
        </ul>

        {#if check.diagnostics.length || check.occupiedPort !== null}
          <div class="issues" class:blocking={!check.canSave} role={check.canSave ? undefined : 'alert'}>
            {#if !check.canSave}<p><Icon name="alert" size={14} stroke={2.4} />{t('cfg.diff.issues')}</p>{/if}
            <ul>
              {#if check.occupiedPort !== null}<li>{t('cfg.occupied', { port: check.occupiedPort })}</li>{/if}
              {#each check.diagnostics as d, i (i)}
                <li class="sev-{d.severity}">{#if d.key}<code class="mono">{d.key}</code>{/if}{issueText(d)}</li>
              {/each}
            </ul>
          </div>
        {/if}

        {#if check.externalAcl && check.canSave}
          <label class="acl">
            <input type="checkbox" bind:checked={ack} />
            <span><strong>{t('cfg.acl')}</strong>{t('cfg.aclAck')}</span>
          </label>
        {/if}

        {#if check.canSave}
          <p class="restart-note warn"><Icon name="restart" size={14} stroke={2.4} />{t('cfg.diff.restartNote')}</p>
        {/if}
        {#if saveFault}<p class="msg error"><Icon name="alert" size={13} stroke={2.4} />{faultText(saveFault)}</p>{/if}

        <div class="actions">
          <button class="btn" onclick={() => (reviewing = false)}>{t('confirm.cancel')}</button>
          {#if check.canSave}
            <button class="btn primary" disabled={saving || (check.externalAcl && !ack)} onclick={save}>{t('cfg.save')}</button>
          {/if}
        </div>
      </div>
    </div>
  {/if}
{/if}

<style>
  .none {
    color: var(--text-3);
  }
  .head {
    margin-bottom: 18px;
  }
  .title-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
  }
  h1 {
    margin: 0 6px 0 0;
    font-size: var(--fs-2xl);
    font-weight: 620;
  }
  .file {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .file.small {
    margin-bottom: 10px;
  }
  .grow {
    flex: 1;
  }
  .layout {
    display: grid;
    grid-template-columns: 150px 1fr;
    gap: 22px;
    align-items: start;
  }
  .groups {
    position: sticky;
    top: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .groups button {
    display: flex;
    align-items: center;
    gap: 9px;
    height: 34px;
    padding: 0 12px;
    border-radius: var(--r-pill);
    border: var(--line-w-thin) solid transparent;
    background: transparent;
    color: var(--text-2);
    font-weight: 550;
    text-align: left;
  }
  .groups button:hover {
    background: var(--hover);
    color: var(--text);
  }
  /* 分组目录的当前项：白底药丸 + 贴身阴影从导航底色上浮起，不描边 */
  .groups button.on {
    background: var(--card-bg);
    color: var(--text);
    box-shadow: var(--shadow-1);
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 18px;
    padding-bottom: 80px;
  }
  .group {
    padding: 6px 20px 8px;
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    box-shadow: var(--shadow-1);
    scroll-margin-top: 8px;
  }
  .group h2 {
    display: flex;
    align-items: center;
    gap: 9px;
    margin: 12px 0 4px;
    font-size: var(--fs-lg);
    font-weight: 620;
  }
  .field {
    display: grid;
    grid-template-columns: minmax(200px, 0.85fr) 1.15fr;
    gap: 18px;
    padding: 14px 0;
    border-top: var(--line-w-thin) dashed var(--line-soft);
  }
  .group h2 + .field {
    border-top: 0;
  }
  .lbl-top {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .name {
    font-weight: 600;
  }
  .tkey {
    display: block;
    margin-top: 1px;
    color: var(--text-3);
    font-size: 11.5px;
  }
  .help {
    margin: 5px 0 0;
    color: var(--text-2);
    font-size: var(--fs-sm);
    max-width: 46ch;
  }
  .tag {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    height: 19px;
    padding: 0 7px;
    border-radius: var(--r-pill);
    font-size: 11px;
    font-weight: 600;
  }
  .dirty-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--lilac-deep);
  }
  .dirty-dot.big {
    width: 10px;
    height: 10px;
    box-shadow: 0 0 0 3px var(--lilac-soft);
  }
  .ctl {
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
    min-width: 0;
  }
  .ctl > .input {
    width: 100%;
  }
  .num-wrap {
    position: relative;
    width: 150px;
  }
  .num-wrap .input {
    padding-right: 40px;
  }
  .unit {
    position: absolute;
    right: 14px;
    top: 50%;
    transform: translateY(-50%);
    color: var(--text-3);
    font-size: var(--fs-sm);
    pointer-events: none;
  }
  .msg {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-sm);
    font-weight: 550;
  }
  .msg.error {
    color: var(--coral-deep);
  }
  .msg.warn {
    color: var(--butter-deep);
  }


  .savebar {
    position: absolute;
    left: 50%;
    bottom: 18px;
    z-index: 20;
    display: flex;
    align-items: center;
    gap: 10px;
    width: min(680px, calc(100% - 56px));
    padding: 9px 10px 9px 18px;
    border-radius: var(--r-pill);
    border: var(--line-w) solid var(--line);
    background: var(--overlay-bg);
    box-shadow: var(--shadow-3);
    translate: -50% 0;
  }
  .sb-text {
    font-weight: 600;
  }
  .msg.inline {
    font-size: var(--fs-xs);
  }

  .scrim {
    position: fixed;
    inset: 0;
    z-index: 80;
    background: var(--scrim);
  }
  .layer {
    position: fixed;
    inset: 0;
    z-index: 81;
    display: grid;
    place-items: center;
    padding: 24px;
    pointer-events: none;
  }
  .review {
    width: min(620px, 100%);
    padding: 22px 24px 20px;
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--overlay-bg);
    box-shadow: var(--shadow-3);
    pointer-events: auto;
  }
  .review h2 {
    margin: 0 0 6px;
    font-size: var(--fs-xl);
    font-weight: 620;
  }
  .restart-note {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0 0 18px;
    color: var(--mint-deep);
    font-weight: 550;
  }
  .restart-note.warn {
    color: var(--butter-deep);
  }
  .issues {
    margin: 0 0 18px;
    padding: 10px 14px;
    border-radius: var(--r-md);
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  .issues p {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0 0 6px;
    font-weight: 600;
  }
  .issues ul {
    margin: 0;
    padding-left: 21px;
    display: grid;
    gap: 4px;
    color: var(--text);
  }
  .issues code {
    margin-right: 8px;
    color: var(--coral-deep);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }

  /* 值的来源：全局默认是天蓝（别处来的、但照样生效），启动覆盖与环境变量是黄油色（要留意），内置默认是灰 */
  .tag {
    background: var(--card-2-bg);
    color: var(--text-2);
  }
  .tag.sky {
    background: var(--sky-soft);
    color: var(--sky-deep);
  }
  .tag.butter {
    background: var(--butter-soft);
    color: var(--butter-deep);
  }
  .muted {
    color: var(--text-3);
  }
  .hint {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 8px 0 0;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .group-desc {
    margin: 0 0 6px;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }

  /* 只读的值 */
  .val {
    word-break: break-all;
    font-size: var(--fs-sm);
  }
  .cells {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  /* 列表里的表（retry.rules、format 的每一项）各自成块，一眼分得清哪几行属于同一项 */
  ol.cells > li:has(> .map) {
    padding: 8px 10px;
    border-radius: var(--r-md);
    background: var(--card-2-bg);
  }
  .cells.map {
    grid-template-columns: max-content 1fr;
    column-gap: 12px;
    row-gap: 4px;
    align-items: baseline;
  }
  .cells.map dt {
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .cells.map dd {
    margin: 0;
    min-width: 0;
  }

  /* 密钥：状态片 + 换新值的输入框 + 删掉 */
  .secret-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .secret-row .input {
    flex: 1;
    min-width: 0;
  }

  /* 请求头表：已有的名字定死、值隐藏；新加的两格都能填 */
  .headers {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .headers li {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .headers li .input {
    flex: 1;
    min-width: 0;
  }
  .headers .h-name {
    flex: 0 0 34%;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: var(--fs-sm);
  }
  .headers li.removed .h-name {
    color: var(--text-3);
    text-decoration: line-through;
  }
  .headers li.locked {
    font-size: var(--fs-sm);
  }
  .btn.add {
    justify-self: start;
  }
  .grow {
    flex: 1;
  }

  /* 读到的问题、被扣下的凭据：放在表单上方，按严重程度着色 */
  .banner,
  .restart-banner {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    margin-bottom: 16px;
    padding: 10px 14px;
    border-radius: var(--r-md);
  }
  .banner {
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  .restart-banner {
    background: var(--mint-soft);
    color: var(--text);
  }
  .banner p,
  .restart-banner p {
    flex: 1;
    min-width: 220px;
    margin: 0;
  }
  .problems {
    display: grid;
    gap: 6px;
    margin-bottom: 18px;
    padding: 10px 14px;
    border-radius: var(--r-md);
    background: var(--card-2-bg);
  }
  .problems p {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 8px;
    margin: 0;
    font-size: var(--fs-sm);
  }
  .problems p :global(svg) {
    flex: none;
    align-self: center;
  }
  .problems code {
    color: var(--text-2);
  }
  .sev-error {
    color: var(--coral-deep);
  }
  .sev-warning {
    color: var(--butter-deep);
  }
  .sev-info {
    color: var(--sky-deep);
  }

  /* 对话框：改动清单、不挡保存的提醒（黄油色）、主目录之外的确认 */
  .changes {
    display: grid;
    gap: 6px;
    margin: 0 0 16px;
    padding: 10px 14px;
    border-radius: var(--r-md);
    background: var(--card-2-bg);
    list-style: none;
  }
  .changes li {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    font-size: var(--fs-sm);
  }
  .changes code {
    color: var(--mint-deep);
    font-weight: 600;
  }
  .changes li.del code {
    color: var(--coral-deep);
  }
  .issues:not(.blocking) {
    background: var(--butter-soft);
    color: var(--text);
  }
  .acl {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    margin: 0 0 16px;
    padding: 10px 14px;
    border-radius: var(--r-md);
    background: var(--butter-soft);
    font-size: var(--fs-sm);
    line-height: 1.5;
    cursor: pointer;
  }
  .acl input {
    margin-top: 3px;
  }
</style>

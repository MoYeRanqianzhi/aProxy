<!--
  配置：按分组的表单（上游、重试、保活、转换、安全），也可以切到原始 TOML。
  每个字段标出「实时生效」或「需重启」，输入时就地校验；有修改时底部浮出保存条，
  保存前先给出差异预览（像 TOML 的逐行增删），需重启的项单独提醒。
  哪些字段能实时生效是原型阶段的提案，接入时以实现为准。
-->
<script lang="ts">
  import { t } from '../lib/i18n';
  import { ui } from '../lib/state.svelte';
  import { store } from '../lib/store.svelte';
  import { overlays } from '../lib/overlays.svelte';
  import { backoffAfter } from '../lib/retry';
  import type { ConfigIssue, ConfigValues, InstanceConfig } from '../lib/types';
  import { pop, fadeOut } from '../lib/motion';
  import Icon, { type IconName } from '../components/Icon.svelte';
  import Segmented from '../components/Segmented.svelte';
  import Dropdown from '../components/Dropdown.svelte';
  import Switch from '../components/Switch.svelte';

  interface Cfg {
    base_url: string;
    listen_addr: string;
    proxy: string;
    connect_timeout_secs: string;
    read_timeout_secs: string;
    max_retry_backoff_secs: string;
    bounded_retry_paths: string[];
    forward_only: boolean;
    spool_limit_mb: string;
    max_body_mb: string;
    disk_cache: boolean;
    keepalive_interval_secs: string;
    keepalive_trigger: 'any' | 'accept' | 'body_stream';
    keepalive_heartbeat: string;
    rt_command: string;
    rt_args: string;
    rt_mode: 'persistent' | 'spawn';
    rt_extra: string;
    every_attempt: boolean;
    allowed_hosts: string[];
    allowed_origins: string[];
  }
  type Field = keyof Cfg;
  type NumField = 'connect_timeout_secs' | 'read_timeout_secs' | 'max_retry_backoff_secs' | 'spool_limit_mb' | 'max_body_mb' | 'keepalive_interval_secs';
  const NUM_FIELDS: Field[] = ['connect_timeout_secs', 'read_timeout_secs', 'max_retry_backoff_secs', 'spool_limit_mb', 'max_body_mb', 'keepalive_interval_secs'];
  /** 表单里摊平的请求转换字段，在配置文件里是同一张内联表 request_transform */
  const isTransformField = (f: Field) => f.startsWith('rt_') || f === 'every_attempt';

  /** 后端给的配置 → 表单：数字变成字符串（输入框里边打字边校验），请求转换摊平成几个字段 */
  function toForm(v: ConfigValues): Cfg {
    const rt = v.request_transform;
    return {
      base_url: v.base_url,
      listen_addr: v.listen_addr,
      proxy: v.proxy,
      connect_timeout_secs: String(v.connect_timeout_secs),
      read_timeout_secs: String(v.read_timeout_secs),
      max_retry_backoff_secs: String(v.max_retry_backoff_secs),
      bounded_retry_paths: [...v.bounded_retry_paths],
      forward_only: v.forward_only,
      spool_limit_mb: String(v.spool_limit_mb),
      max_body_mb: String(v.max_body_mb),
      disk_cache: v.disk_cache,
      keepalive_interval_secs: String(v.keepalive_interval_secs),
      keepalive_trigger: v.keepalive_trigger,
      keepalive_heartbeat: v.keepalive_heartbeat,
      rt_command: rt?.command ?? '',
      rt_args: rt?.args ?? '',
      rt_mode: rt?.mode ?? 'persistent',
      rt_extra: rt?.extra ?? '',
      every_attempt: rt?.every_attempt ?? false,
      allowed_hosts: [...v.allowed_hosts],
      allowed_origins: [...v.allowed_origins],
    };
  }

  /** 表单 → 只含改动的配置键，交给后端写回文件 */
  function changesOf(c: Cfg, fields: Field[]): Partial<ConfigValues> {
    const out: Record<string, unknown> = {};
    for (const f of fields) {
      if (isTransformField(f)) {
        out.request_transform = c.rt_command
          ? { command: c.rt_command, args: c.rt_args, mode: c.rt_mode, extra: c.rt_extra, every_attempt: c.every_attempt }
          : null;
      } else {
        out[f] = NUM_FIELDS.includes(f) ? Number(c[f]) : $state.snapshot(c[f]);
      }
    }
    return out as Partial<ConfigValues>;
  }

  const instances = $derived(store.instances);
  const inst = $derived(store.instance(ui.instancePort) ?? instances[0]);

  // 页面按实例端口挂载（App 里的 {#key}），换实例会重建本页，所以配置只读一次。
  // 直接从地址栏打开本页时，界面可能还没拿到实例列表，等连上之后再读
  let config = $state<InstanceConfig | null>(null);
  let original = $state<Cfg | null>(null);
  let cfg = $state<Cfg | null>(null);
  let requested = false;
  $effect(() => {
    if (requested || !store.connected) return;
    const i = store.instance(ui.instancePort) ?? store.instances[0];
    if (!i) return;
    requested = true;
    store.readConfig(i.port).then((c) => {
      config = c;
      original = toForm(c.values);
      cfg = toForm(c.values);
    });
  });
  /** 哪些键改了立即生效由实例说了算（后端随配置一起给出） */
  const live = $derived(new Set<string>(config?.live ?? []));
  const isLive = (f: Field) => live.has(isTransformField(f) ? 'request_transform' : f);
  let view = $state<'form' | 'toml'>('form');
  let reviewing = $state(false);
  /** 实例对这批改动的意见：格式之外界面判断不了的问题（端口被占、上游指回自己），有问题就不让保存 */
  let issues = $state<ConfigIssue[]>([]);
  let checking = $state(false);

  // 「检查并保存」先请实例检查一遍，再打开对比框：问题和改动摆在一起看
  async function review() {
    if (!cfg || !config) return;
    checking = true;
    try {
      issues = await store.validateConfig(config.port, changesOf(cfg, changed));
    } finally {
      checking = false;
    }
    reviewing = true;
  }
  let port = $state(ui.instancePort);
  let newPath = $state('');
  let newHost = $state('');
  let newOrigin = $state('');

  $effect(() => {
    if (port !== ui.instancePort && store.instance(port)) ui.go('config', port);
  });

  const intField = (v: string) => /^\d+$/.test(v.trim());

  const errors = $derived.by(() => {
    const e: Partial<Record<Field, string>> = {};
    if (!cfg) return e;
    if (!/^https?:\/\/\S+$/.test(cfg.base_url.trim())) e.base_url = t('cfg.err.url');
    if (cfg.proxy && !/^(https?|socks5h?):\/\/\S+$/.test(cfg.proxy.trim())) e.proxy = t('cfg.err.url');
    for (const f of ['connect_timeout_secs', 'read_timeout_secs', 'max_retry_backoff_secs', 'spool_limit_mb', 'max_body_mb', 'keepalive_interval_secs'] as const) {
      if (!intField(cfg[f])) e[f] = t('cfg.err.int');
    }
    for (const p of cfg.bounded_retry_paths) {
      try {
        new RegExp(p);
      } catch {
        e.bounded_retry_paths = t('cfg.err.regex');
      }
    }
    return e;
  });

  const changed = $derived.by((): Field[] => {
    if (!cfg || !original) return [];
    return (Object.keys(cfg) as Field[]).filter((k) => JSON.stringify(cfg![k]) !== JSON.stringify(original![k]));
  });
  const needRestart = $derived(changed.filter((f) => !isLive(f)));
  const hasErrors = $derived(Object.keys(errors).length > 0);

  /** 退避阶梯：第 n 次失败后等多久。一眼看懂「前三次立即、之后翻倍、到封顶为止」 */
  const ladder = $derived.by(() => {
    const cap = Number(cfg?.max_retry_backoff_secs);
    if (!Number.isFinite(cap)) return [];
    return Array.from({ length: 10 }, (_, i) => backoffAfter(i + 1, cap) / 1000);
  });

  function tomlValue(v: unknown): string {
    if (typeof v === 'boolean') return String(v);
    if (Array.isArray(v)) return `[${v.map((x) => `'${x}'`).join(', ')}]`;
    if (typeof v === 'string' && /^\d+$/.test(v)) return v;
    return `"${v}"`;
  }

  /** 把表单映射回 TOML 的键（请求转换收拢成一张内联表） */
  function tomlLines(c: Cfg): { key: string; text: string }[] {
    const lines: { key: string; text: string }[] = [];
    const simple: Field[] = [
      'base_url',
      'listen_addr',
      'proxy',
      'connect_timeout_secs',
      'read_timeout_secs',
      'max_retry_backoff_secs',
      'bounded_retry_paths',
      'forward_only',
      'spool_limit_mb',
      'max_body_mb',
      'disk_cache',
      'keepalive_interval_secs',
      'keepalive_trigger',
      'keepalive_heartbeat',
      'allowed_hosts',
      'allowed_origins',
    ];
    for (const k of simple) {
      const v = c[k];
      if (v === '' || (Array.isArray(v) && v.length === 0)) continue;
      lines.push({ key: k, text: `${k} = ${tomlValue(v)}` });
    }
    if (c.rt_command) {
      lines.push({
        key: 'request_transform',
        text: `request_transform = { command = "${c.rt_command}", args = ["${c.rt_args}"], mode = "${c.rt_mode}", extra = "${c.rt_extra}"${c.every_attempt ? ', every_attempt = true' : ''} }`,
      });
    }
    return lines;
  }

  const diff = $derived.by(() => {
    if (!cfg || !original) return [];
    const before = tomlLines(original);
    const after = tomlLines(cfg);
    const out: { kind: '-' | '+' | ' '; text: string }[] = [];
    const keys = [...new Set([...before.map((l) => l.key), ...after.map((l) => l.key)])];
    for (const k of keys) {
      const b = before.find((l) => l.key === k);
      const a = after.find((l) => l.key === k);
      if (b && a && b.text === a.text) continue;
      if (b) out.push({ kind: '-', text: b.text });
      if (a) out.push({ kind: '+', text: a.text });
    }
    return out;
  });

  function discard() {
    if (original) cfg = $state.snapshot(original);
  }

  function setNum(f: NumField, v: string) {
    if (cfg) cfg[f] = v;
  }

  async function save(restart: boolean) {
    if (!cfg || !config) return;
    const { port: target, path } = config;
    const name = store.instance(target)?.alias ?? target;
    await store.writeConfig(target, changesOf(cfg, changed));
    original = $state.snapshot(cfg);
    reviewing = false;
    overlays.toast(t('toast.saved', { path }));
    if (restart) {
      await store.restart(target);
      overlays.toast(t('toast.restarted', { name }));
    }
  }

  function addTo(list: string[], value: string): boolean {
    const v = value.trim();
    if (!v || list.includes(v)) return false;
    list.push(v);
    return true;
  }

  const groups: { id: string; icon: IconName; label: () => string }[] = [
    { id: 'upstream', icon: 'globe', label: () => t('cfg.group.upstream') },
    { id: 'retry', icon: 'repeat', label: () => t('cfg.group.retry') },
    { id: 'keepalive', icon: 'heart', label: () => t('cfg.group.keepalive') },
    { id: 'transform', icon: 'shuffle', label: () => t('cfg.group.transform') },
    { id: 'security', icon: 'lock', label: () => t('cfg.group.security') },
  ];
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

  const instanceOptions = $derived(instances.map((i) => ({ value: i.port, label: i.alias ? `${i.port} ${i.alias}` : `${i.port}` })));

  /** TOML 语法着色：逐行切成键、等号、值（字符串 / 数字 / 布尔 / 数组） */
  function tokens(line: string): { cls: string; text: string }[] {
    const m = line.match(/^(\w+)(\s*=\s*)(.*)$/);
    if (!m) return [{ cls: 'plain', text: line }];
    const v = m[3];
    const cls = /^"/.test(v) ? 'str' : /^\d/.test(v) ? 'num' : /^(true|false)/.test(v) ? 'bool' : /^[[{]/.test(v) ? 'arr' : 'plain';
    return [
      { cls: 'key', text: m[1] },
      { cls: 'eq', text: m[2] },
      { cls, text: v },
    ];
  }
</script>

{#snippet badge(f: Field)}
  {#if isLive(f)}
    <span class="tag live"><Icon name="bolt" size={11} stroke={2.4} />{t('cfg.live')}</span>
  {:else}
    <span class="tag restart"><Icon name="restart" size={11} stroke={2.4} />{t('cfg.restart')}</span>
  {/if}
{/snippet}

{#snippet label(f: Field, key: Parameters<typeof t>[0], help?: Parameters<typeof t>[0])}
  <div class="lbl">
    <div class="lbl-top">
      <!-- 输入框靠 aria-labelledby 指回这里取得可读的名字（字段名在左栏，不和输入框嵌套） -->
      <span class="name" id="lbl-{f}">{t(key)}</span>
      {@render badge(f)}
      {#if changed.includes(f)}<span class="dirty-dot" title={t('cfg.dirty', { n: 1 })}></span>{/if}
    </div>
    <code class="mono tkey">{f.startsWith('rt_') ? 'request_transform' : f}</code>
    {#if help}<p class="help">{t(help)}</p>{/if}
  </div>
{/snippet}

{#snippet err(f: Field)}
  {#if errors[f]}
    <span class="msg error" in:pop={{ y: -4, scale: 1, duration: 300 }}><Icon name="alert" size={13} stroke={2.4} />{errors[f]}</span>
  {/if}
{/snippet}

{#snippet num(f: NumField, unit: string)}
  <div class="num-wrap">
    <input
      class="input mono"
      class:invalid={!!errors[f]}
      inputmode="numeric"
      aria-labelledby="lbl-{f}"
      value={cfg?.[f] ?? ''}
      oninput={(e) => setNum(f, e.currentTarget.value)}
      aria-invalid={!!errors[f]}
    />
    <span class="unit">{unit}</span>
  </div>
{/snippet}

{#snippet chips(list: string[], draft: string, setDraft: (v: string) => void, placeholder: string)}
  <div class="chips">
    {#each list as item, i (item)}
      <span class="item-chip mono" in:pop={{ y: 0, scale: 0.8, duration: 420 }}>
        {item}
        <button onclick={() => list.splice(i, 1)} aria-label={t('cfg.remove')}><Icon name="x" size={11} stroke={2.8} /></button>
      </span>
    {/each}
    <span class="adder">
      <input
        class="mono"
        value={draft}
        {placeholder}
        oninput={(e) => setDraft(e.currentTarget.value)}
        onkeydown={(e) => {
          if (e.key === 'Enter' && addTo(list, draft)) setDraft('');
        }}
        aria-label={t('cfg.add')}
      />
      <button onclick={() => addTo(list, draft) && setDraft('')}><Icon name="plus" size={13} stroke={2.6} />{t('cfg.add')}</button>
    </span>
  </div>
{/snippet}

{#if !inst || !cfg || !config}
  <p class="none">{t('logs.noTraffic')}</p>
{:else}
  <header class="head">
    <div class="title-row">
      <h1>{t('nav.config')}</h1>
      <Dropdown options={instanceOptions} bind:value={port} label={t('logs.col.instance')} icon="server" />
      <span class="file"><Icon name="file" size={14} /><code class="mono">{config.path}</code></span>
      <button class="btn ghost sm" onclick={() => overlays.toast(t('toast.protoOnly'), 'lilac')}><Icon name="external" size={14} />{t('cfg.openInEditor')}</button>
      <span class="grow"></span>
      <Segmented
        options={[
          { value: 'form', label: t('cfg.form'), icon: 'sliders' },
          { value: 'toml', label: t('cfg.toml'), icon: 'braces' },
        ]}
        bind:value={view}
        label={t('cfg.view')}
        size="sm"
      />
    </div>
  </header>

  {#if view === 'toml'}
    <div class="toml" in:pop={{ y: 6, scale: 1 }}>
      <div class="toml-file"><Icon name="file" size={13} />{config.path}</div>
      <ol>
        {#each tomlLines(cfg) as line, i (line.key)}
          <li><span class="ln">{i + 1}</span><code class="mono">{#each tokens(line.text) as tk, j (j)}<span class="tk {tk.cls}">{tk.text}</span>{/each}</code></li>
        {/each}
      </ol>
    </div>
  {:else}
    <div class="layout">
      <nav class="groups" aria-label={t('nav.config')}>
        {#each groups as g (g.id)}
          <button class:on={activeGroup === g.id} onclick={() => jump(g.id)}>
            <Icon name={g.icon} size={16} />{g.label()}
          </button>
        {/each}
      </nav>

      <div class="form" bind:this={formEl}>
        <section class="group" id="g-upstream">
          <h2><Icon name="globe" size={17} />{t('cfg.group.upstream')}</h2>
          <div class="field">
            {@render label('base_url', 'cfg.f.base_url', 'cfg.h.base_url')}
            <div class="ctl">
              <input class="input mono" class:invalid={!!errors.base_url} bind:value={cfg.base_url} aria-invalid={!!errors.base_url} aria-labelledby="lbl-base_url" />
              {@render err('base_url')}
            </div>
          </div>
          <div class="field">
            {@render label('listen_addr', 'cfg.f.listen_addr', 'cfg.h.listen_addr')}
            <div class="ctl"><input class="input mono" bind:value={cfg.listen_addr} aria-labelledby="lbl-listen_addr" /></div>
          </div>
          <div class="field">
            {@render label('proxy', 'cfg.f.proxy', 'cfg.h.proxy')}
            <div class="ctl">
              <input class="input mono" class:invalid={!!errors.proxy} bind:value={cfg.proxy} placeholder="http://127.0.0.1:7890" aria-labelledby="lbl-proxy" />
              {@render err('proxy')}
            </div>
          </div>
          <div class="field">
            <div class="lbl">
              <div class="lbl-top"><span class="name">{t('cfg.f.api_key')}</span><span class="tag live"><Icon name="bolt" size={11} stroke={2.4} />{t('cfg.live')}</span></div>
              <code class="mono tkey">api_key</code>
              <p class="help">{t('cfg.h.api_key')}</p>
            </div>
            <div class="ctl row">
              <!-- 密钥只写不读：后端只给打码后的样子 -->
              {#if config.apiKeyMasked}
                <span class="chip pink"><Icon name="lock" size={12} stroke={2.4} />{t('cfg.secretSet')}<code class="mono">{config.apiKeyMasked}</code></span>
              {:else}
                <span class="chip">{t('cfg.secretUnset')}</span>
              {/if}
              <button class="btn sm" onclick={() => overlays.toast(t('toast.protoOnly'), 'lilac')}><Icon name="key" size={13} />{t('cfg.replace')}</button>
            </div>
          </div>
          <div class="field two">
            <div>
              {@render label('connect_timeout_secs', 'cfg.f.connect_timeout_secs')}
              {@render num('connect_timeout_secs', t('cfg.seconds'))}
              {@render err('connect_timeout_secs')}
            </div>
            <div>
              {@render label('read_timeout_secs', 'cfg.f.read_timeout_secs')}
              {@render num('read_timeout_secs', t('cfg.seconds'))}
              {@render err('read_timeout_secs')}
              <p class="help">{t('cfg.h.read_timeout_secs')}</p>
            </div>
          </div>
        </section>

        <section class="group" id="g-retry">
          <h2><Icon name="repeat" size={17} />{t('cfg.group.retry')}</h2>
          <div class="field">
            {@render label('max_retry_backoff_secs', 'cfg.f.max_retry_backoff_secs', 'cfg.h.max_retry_backoff_secs')}
            <div class="ctl">
              {@render num('max_retry_backoff_secs', t('cfg.seconds'))}
              {@render err('max_retry_backoff_secs')}
              {#if cfg.max_retry_backoff_secs.trim() === '0'}
                <span class="msg warn" in:pop={{ y: -4, scale: 1, duration: 300 }}><Icon name="alert" size={13} stroke={2.4} />{t('cfg.warn.backoff0')}</span>
              {/if}
              {#if ladder.length && !errors.max_retry_backoff_secs}
                <ol class="ladder" aria-hidden="true">
                  {#each ladder as s, i (i)}
                    <li class:zero={s === 0} class:cap={s > 0 && s === Number(cfg.max_retry_backoff_secs)}>{s}s</li>
                  {/each}
                  <li class="more">…</li>
                </ol>
              {/if}
            </div>
          </div>
          <div class="field">
            {@render label('bounded_retry_paths', 'cfg.f.bounded_retry_paths', 'cfg.h.bounded_retry_paths')}
            <div class="ctl">
              {@render chips(cfg.bounded_retry_paths, newPath, (v) => (newPath = v), '/v1/files.*')}
              {@render err('bounded_retry_paths')}
            </div>
          </div>
          <div class="field" class:hazard-field={cfg.forward_only}>
            {@render label('forward_only', 'cfg.f.forward_only', 'cfg.h.forward_only')}
            <div class="ctl">
              <Switch bind:checked={cfg.forward_only} label={t('cfg.f.forward_only')} />
              {#if cfg.forward_only}
                <span class="msg warn strong" in:pop={{ y: -4, scale: 1, duration: 300 }}><span class="tape"></span>{t('cfg.warn.forwardOnly')}</span>
              {/if}
            </div>
          </div>
          <div class="field two">
            <div>
              {@render label('spool_limit_mb', 'cfg.f.spool_limit_mb')}
              {@render num('spool_limit_mb', 'MB')}
              {@render err('spool_limit_mb')}
            </div>
            <div>
              {@render label('max_body_mb', 'cfg.f.max_body_mb')}
              {@render num('max_body_mb', 'MB')}
              {@render err('max_body_mb')}
            </div>
          </div>
          <div class="field">
            {@render label('disk_cache', 'cfg.f.disk_cache', 'cfg.h.disk_cache')}
            <div class="ctl"><Switch bind:checked={cfg.disk_cache} label={t('cfg.f.disk_cache')} /></div>
          </div>
        </section>

        <section class="group" id="g-keepalive">
          <h2><Icon name="heart" size={17} />{t('cfg.group.keepalive')}</h2>
          <div class="field">
            {@render label('keepalive_interval_secs', 'cfg.f.keepalive_interval_secs', 'cfg.h.keepalive_interval_secs')}
            <div class="ctl">
              {@render num('keepalive_interval_secs', t('cfg.seconds'))}
              {@render err('keepalive_interval_secs')}
            </div>
          </div>
          <div class="field">
            {@render label('keepalive_trigger', 'cfg.f.keepalive_trigger')}
            <div class="ctl">
              <Segmented
                options={[
                  { value: 'any', label: t('cfg.o.any') },
                  { value: 'accept', label: t('cfg.o.accept') },
                  { value: 'body_stream', label: t('cfg.o.body_stream') },
                ]}
                bind:value={cfg.keepalive_trigger}
                label={t('cfg.f.keepalive_trigger')}
                size="sm"
              />
            </div>
          </div>
          <div class="field">
            {@render label('keepalive_heartbeat', 'cfg.f.keepalive_heartbeat', 'cfg.h.keepalive_heartbeat')}
            <div class="ctl"><input class="input mono" bind:value={cfg.keepalive_heartbeat} aria-labelledby="lbl-keepalive_heartbeat" /></div>
          </div>
        </section>

        <section class="group" id="g-transform">
          <h2><Icon name="shuffle" size={17} />{t('cfg.group.transform')}</h2>
          <div class="field">
            {@render label('rt_command', 'cfg.f.request_transform', 'cfg.h.request_transform')}
            <div class="ctl stack">
              <input class="input mono" bind:value={cfg.rt_command} placeholder="~/.aproxy/bin/aproxy-format" aria-label="command" />
              <div class="pair">
                <input class="input mono" bind:value={cfg.rt_args} placeholder="run" aria-label="args" />
                <Segmented
                  options={[
                    { value: 'persistent', label: t('cfg.o.persistent') },
                    { value: 'spawn', label: t('cfg.o.spawn') },
                  ]}
                  bind:value={cfg.rt_mode}
                  label={t('cfg.f.mode')}
                  size="sm"
                />
              </div>
              <input class="input mono" bind:value={cfg.rt_extra} placeholder={t('cfg.f.extra')} aria-label={t('cfg.f.extra')} />
            </div>
          </div>
          <div class="field">
            {@render label('every_attempt', 'cfg.f.every_attempt', 'cfg.h.every_attempt')}
            <div class="ctl"><Switch bind:checked={cfg.every_attempt} label={t('cfg.f.every_attempt')} /></div>
          </div>
        </section>

        <section class="group" id="g-security">
          <h2><Icon name="lock" size={17} />{t('cfg.group.security')}</h2>
          <div class="field">
            {@render label('allowed_hosts', 'cfg.f.allowed_hosts', 'cfg.h.allowed_hosts')}
            <div class="ctl">{@render chips(cfg.allowed_hosts, newHost, (v) => (newHost = v), 'myproxy.local')}</div>
          </div>
          <div class="field">
            {@render label('allowed_origins', 'cfg.f.allowed_origins', 'cfg.h.allowed_origins')}
            <div class="ctl">{@render chips(cfg.allowed_origins, newOrigin, (v) => (newOrigin = v), 'http://localhost:5173')}</div>
          </div>
        </section>
      </div>
    </div>
  {/if}

  {#if changed.length > 0}
    <div class="savebar" in:pop={{ y: 30, scale: 0.96, duration: 560 }} out:fadeOut>
      <span class="dirty-dot big"></span>
      <span class="sb-text">{t('cfg.dirty', { n: changed.length })}</span>
      {#if hasErrors}<span class="msg error inline"><Icon name="alert" size={13} stroke={2.4} />{t('cfg.err.fix')}</span>{/if}
      <span class="grow"></span>
      <button class="btn sm ghost" onclick={discard}><Icon name="undo" size={14} />{t('cfg.discard')}</button>
      <button class="btn sm primary" disabled={hasErrors || checking} onclick={review}>
        <Icon name="eye" size={14} />{checking ? t('cfg.checking') : t('cfg.review')}
      </button>
    </div>
  {/if}

  {#if reviewing}
    <div class="scrim" in:fadeOut={{ duration: 160 }} out:fadeOut={{ duration: 120 }} onclick={() => (reviewing = false)} aria-hidden="true"></div>
    <div class="layer">
      <div class="review" role="dialog" aria-modal="true" aria-labelledby="review-title" in:pop={{ y: 18, scale: 0.94, duration: 560 }} out:fadeOut={{ duration: 120 }}>
        <h2 id="review-title">{t('cfg.diff.title')}</h2>
        <div class="file small"><Icon name="file" size={13} /><code class="mono">{config.path}</code></div>
        <ol class="diff">
          {#each diff as d, i (i)}
            <li class="d-{d.kind === '-' ? 'del' : 'add'}"><span class="sign">{d.kind}</span><code class="mono">{d.text}</code></li>
          {/each}
        </ol>
        {#if issues.length}
          <div class="issues" role="alert">
            <p><Icon name="alert" size={14} stroke={2.4} />{t('cfg.diff.issues')}</p>
            <ul>
              {#each issues as issue (issue.key)}
                <li><code class="mono">{issue.key}</code>{issue.message[ui.lang] ?? issue.message.en}</li>
              {/each}
            </ul>
          </div>
        {:else}
          <p class="restart-note" class:warn={needRestart.length > 0}>
            <Icon name={needRestart.length > 0 ? 'restart' : 'bolt'} size={14} stroke={2.4} />
            {needRestart.length > 0 ? t('cfg.diff.restartNote', { n: needRestart.length }) : t('cfg.diff.allLive')}
          </p>
        {/if}
        <div class="actions">
          <button class="btn" onclick={() => (reviewing = false)}>{t('confirm.cancel')}</button>
          {#if !issues.length}
            <button class="btn {needRestart.length > 0 ? '' : 'primary'}" onclick={() => save(false)}>{t('cfg.save')}</button>
            {#if needRestart.length > 0}
              <button class="btn primary" onclick={() => save(true)}><Icon name="restart" size={14} />{t('cfg.saveRestart')}</button>
            {/if}
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
  .field.two {
    grid-template-columns: 1fr 1fr;
  }
  .field.two > div {
    display: grid;
    gap: 8px;
    align-content: start;
  }
  .field.hazard-field {
    margin: 0 -12px;
    padding: 14px 12px;
    border-radius: var(--r-md);
    background: var(--butter-soft);
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
  .tag.live {
    background: var(--mint-soft);
    color: var(--mint-deep);
  }
  .tag.restart {
    background: var(--butter-soft);
    color: var(--butter-deep);
    border: 1px solid color-mix(in oklab, var(--butter-deep) 30%, transparent);
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
  .ctl.row {
    flex-direction: row;
    align-items: center;
    flex-wrap: wrap;
  }
  .ctl.stack {
    align-items: stretch;
  }
  .pair {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .pair .input {
    flex: 1;
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
  .msg.warn.strong {
    color: var(--text);
  }
  .msg .tape {
    flex: none;
    width: 26px;
    height: 16px;
    border-radius: var(--r-pill);
    border: var(--line-w) solid var(--butter-deep);
    background: repeating-linear-gradient(-45deg, var(--on-fill) 0 4px, var(--butter) 4px 8px);
  }
  .ladder {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin: 2px 0 0;
    padding: 0;
    list-style: none;
  }
  .ladder li {
    padding: 2px 7px;
    border-radius: var(--r-pill);
    background: var(--sky-soft);
    color: var(--sky-deep);
    font-size: 11px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .ladder li.zero {
    background: var(--card-2-bg);
    color: var(--text-3);
  }
  .ladder li.cap {
    background: var(--sky);
    color: var(--on-fill);
  }
  .ladder li.more {
    background: transparent;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }
  .item-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 28px;
    padding: 0 4px 0 11px;
    border-radius: var(--r-pill);
    border: var(--line-w-thin) solid color-mix(in oklab, var(--lilac-soft) 60%, var(--lilac));
    background: var(--lilac-soft);
    font-size: 12px;
  }
  .item-chip button {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: 0;
    background: transparent;
    color: var(--text-2);
  }
  .item-chip button:hover {
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  .adder {
    display: inline-flex;
    align-items: center;
    height: 30px;
    padding: 0 3px 0 12px;
    border-radius: var(--r-pill);
    /* 「添加一项」的输入位：虚线表示还空着，但它能输入，所以虚线用强边色 */
    border: var(--line-w-thin) dashed var(--line-strong);
    background: var(--card-bg);
  }
  .adder input {
    width: 190px;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--text);
    font-size: 12px;
  }
  .adder input::placeholder {
    color: var(--text-3);
  }
  .adder button {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    height: 22px;
    padding: 0 9px;
    border-radius: var(--r-pill);
    border: 0;
    background: var(--lilac);
    color: var(--on-fill);
    font-size: 11.5px;
    font-weight: 600;
  }

  .toml {
    border-radius: var(--r-lg);
    border: var(--line-w) solid var(--line);
    background: var(--card-bg);
    overflow: hidden;
  }
  .toml-file {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 9px 16px;
    border-bottom: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-2-bg);
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .toml ol {
    margin: 0;
    padding: 10px 0 14px;
    list-style: none;
  }
  .toml li {
    display: flex;
    gap: 14px;
    padding: 1px 16px;
    font-size: 13px;
    line-height: 1.7;
  }
  .ln {
    width: 22px;
    flex: none;
    text-align: right;
    color: var(--text-3);
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .tk.key {
    color: var(--lilac-deep);
  }
  .tk.eq {
    color: var(--text-3);
  }
  .tk.str {
    color: var(--mint-deep);
  }
  .tk.num {
    color: var(--sky-deep);
  }
  .tk.bool {
    color: var(--coral-deep);
  }
  .tk.arr {
    color: var(--text);
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
  .diff {
    margin: 0 0 14px;
    padding: 8px 0;
    list-style: none;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-2-bg);
    max-height: 300px;
    overflow: auto;
  }
  .diff li {
    display: flex;
    gap: 10px;
    padding: 2px 14px;
    font-size: 12.5px;
    line-height: 1.6;
  }
  .diff code {
    white-space: pre-wrap;
    word-break: break-all;
  }
  .d-del {
    background: var(--coral-soft);
    color: var(--coral-deep);
  }
  .d-add {
    background: var(--mint-soft);
    color: var(--mint-deep);
  }
  .sign {
    width: 10px;
    flex: none;
    font-family: var(--font-mono);
    font-weight: 700;
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
</style>

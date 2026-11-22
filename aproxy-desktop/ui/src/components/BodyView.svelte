<!--
  一段请求体或响应体：按块向 Rust 取（body_chunk），拼起来按内容显示成 JSON 树、SSE 事件列表或纯文本。

  体可能很大，也是最敏感的内容，所以：
    - 只在这一块真的显示出来时才取，取第一块（CHUNK）就先显示，更多的要用户点「继续加载」；
      取到 AUTO_LIMIT 之后不再自动往下取，免得一个几十 MB 的响应把窗口拖慢。
    - 交来的字节里已知的密钥已经被等长的 * 盖住；这里只解码、显示，不缓存、不写到任何地方。
    - 选择（selection）换了——详情被重新打开或者关掉——旧选择上的块一律丢弃，Rust 那边也不再交给旧选择。
  more 与 complete 都为假表示「目前没有更多」（比如还在接收中的响应），只有用户再点才重读。
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { BodyInfo, BodyPart, Ref, Selection } from '../lib/backend/contract';
  import { toFailure, type Failure } from '../lib/backend/link';
  import { faultText } from '../lib/faults';
  import { bytes, t } from '../lib/i18n';
  import { copyText, overlays } from '../lib/overlays.svelte';
  import { store } from '../lib/store.svelte';
  import { pop } from '../lib/motion';
  import type { Json } from '../lib/types';
  import Icon from './Icon.svelte';
  import JsonView from './JsonView.svelte';

  let { selection, target, part, info }: { selection: Selection; target: Ref; part: BodyPart; info: BodyInfo | null } = $props();

  /** 一次取多少字节 */
  const CHUNK = 256 * 1024;
  /** 自动取到这么多就停下，等用户点 */
  const AUTO_LIMIT = 1024 * 1024;
  /** 超过这么大的不尝试解析成 JSON 树：树的节点太多会卡 */
  const JSON_LIMIT = 1024 * 1024;

  let text = $state('');
  let loaded = $state(0);
  let total = $state<number | null>(null);
  let more = $state(false);
  let complete = $state(false);
  let loading = $state(false);
  let failure = $state.raw<Failure | null>(null);
  let openEvent = $state<number | null>(null);

  // 多字节字符可能被切在两块之间：流式解码，残缺的尾巴留给下一块
  let decoder = new TextDecoder('utf-8');

  function decode(bin: string, last: boolean): string {
    const buf = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
    return decoder.decode(buf, { stream: !last });
  }

  /** 这个组件实例还在显示同一个选择：换了选择的组件会被重建，旧的迟到结果靠它丢掉 */
  let alive = true;
  $effect(() => () => (alive = false));

  async function next(auto: boolean) {
    if (loading) return;
    loading = true;
    failure = null;
    try {
      do {
        const chunk = await store.bodyChunk(selection, target, part, loaded, CHUNK);
        if (!alive) return;
        total = chunk.totalBytes;
        more = chunk.more;
        complete = chunk.complete;
        const raw = atob(chunk.dataB64);
        text += decode(raw, !chunk.more);
        loaded = chunk.offset + raw.length;
      } while (auto && more && loaded < AUTO_LIMIT);
    } catch (raw) {
      if (alive) failure = toFailure(raw);
    } finally {
      if (alive) loading = false;
    }
  }

  /** 「目前没有更多」之后用户要求再读：从头再来，响应可能已经接收完了 */
  function reload() {
    decoder = new TextDecoder('utf-8');
    text = '';
    loaded = 0;
    total = null;
    more = complete = false;
    void next(true);
  }

  // 只看 info：next 里读写的加载状态不能成为这个 effect 的依赖，否则每取一块就又触发一次
  $effect(() => {
    if (info?.state === 'retained' && info.bytes > 0) untrack(() => void next(true));
  });

  /** SSE：以空行分隔的事件，每个事件有 event: 与 data: 行。冒号开头的是注释（保活心跳），不算事件 */
  function sseOf(s: string): { event: string; data: string }[] | null {
    if (!/^(event|data):/m.test(s.slice(0, 200))) return null;
    const out: { event: string; data: string }[] = [];
    for (const block of s.split(/\r?\n\r?\n/)) {
      let event = 'message';
      const data: string[] = [];
      for (const line of block.split(/\r?\n/)) {
        if (line.startsWith('event:')) event = line.slice(6).trim();
        else if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''));
      }
      if (data.length) out.push({ event, data: data.join('\n') });
    }
    return out.length ? out : null;
  }

  function jsonOf(s: string): { value: Json } | null {
    const head = s.trimStart()[0];
    if (head !== '{' && head !== '[') return null;
    try {
      return { value: JSON.parse(s) };
    } catch {
      return null;
    }
  }

  const done = $derived(total !== null && loaded >= total && !more);
  const json = $derived(done && loaded <= JSON_LIMIT ? jsonOf(text) : null);
  const events = $derived(json ? null : sseOf(text));
  /** 流式响应里拼起来的文本：各家的增量事件都把文字放在 text 或 delta 字段里 */
  const assembled = $derived(
    events
      ?.filter((ev) => ev.event.endsWith('delta') || ev.event === 'message')
      .map((ev) => {
        const m = ev.data.match(/"(?:text|delta|content)":("(?:[^"\\]|\\.)*")/);
        try {
          return m ? (JSON.parse(m[1]) as string) : '';
        } catch {
          return '';
        }
      })
      .join('') ?? '',
  );

  function parsed(data: string): Json {
    try {
      return JSON.parse(data);
    } catch {
      return data;
    }
  }

  async function copy() {
    await copyText(text);
    overlays.toast(t(done ? 'toast.copiedBody' : 'toast.copiedPartial'));
  }
</script>

{#if info === null}
  <p class="note">{t('body.none')}</p>
{:else if info.state === 'same-as-previous'}
  <p class="note">{t('body.same')}</p>
{:else if info.state === 'evicted'}
  <p class="note">{t('body.evicted')}</p>
{:else if info.state === 'not-retained'}
  <p class="note">{t(info.cause === 'budget-disabled' ? 'body.off' : info.cause === 'exceeds-budget' ? 'body.tooBig' : info.cause === 'decode-failed' ? 'body.undecodable' : 'body.notKept')}</p>
{:else if info.state !== 'retained'}
  <p class="note">{t('body.unknown')}</p>
{:else if info.bytes === 0}
  <p class="note">{t('d.noBody')}</p>
{:else}
  <div class="bar">
    <span class="muted">{total !== null && loaded < total ? t('body.partial', { got: bytes(loaded), all: bytes(total) }) : bytes(total ?? info.bytes)}</span>
    {#if info.encoding && info.encoding !== 'identity'}<span class="chip">{info.encoding}</span>{/if}
    <span class="grow"></span>
    {#if text}<button class="mini" onclick={copy}><Icon name="copy" size={13} />{t('d.copy')}</button>{/if}
  </div>
  {#if failure}
    <p class="note err">{faultText(failure)}</p>
  {/if}
  {#if json}
    <JsonView value={json.value} openDepth={3} />
  {:else if events}
    {#if assembled}
      <div class="assembled">
        <span class="label">{t('d.textPreview')}</span>
        <p>{assembled}</p>
      </div>
    {/if}
    <div class="events-head">
      <span>{t('d.events')}</span><span class="chip">{t('d.eventsN', { n: events.length })}</span>
    </div>
    <ol class="events">
      {#each events as ev, i (i)}
        <li class="ev" class:open={openEvent === i}>
          <button class="ev-row" onclick={() => (openEvent = openEvent === i ? null : i)} aria-expanded={openEvent === i}>
            <span class="ev-name">{ev.event}</span>
            <span class="ev-data mono">{ev.data}</span>
          </button>
          {#if openEvent === i}
            <div class="ev-json" in:pop={{ y: -4, scale: 0.99, duration: 360 }}>
              <JsonView value={parsed(ev.data)} openDepth={4} />
            </div>
          {/if}
        </li>
      {/each}
    </ol>
  {:else if text}
    <pre class="pre mono">{text}</pre>
  {:else if loading}
    <p class="note">{t('d.loading')}</p>
  {/if}
  {#if !loading && more}
    <button class="btn sm" onclick={() => next(true)}>{t('body.more')}</button>
  {:else if !loading && !more && !complete && total !== null}
    <!-- 目前没有更多：响应可能还在接收，用户点了才再读 -->
    <p class="note">{t('body.notYet')}<button class="btn ghost sm" onclick={reload}>{t('body.reload')}</button></p>
  {:else if loading && text}
    <p class="note">{t('d.loading')}</p>
  {/if}
{/if}

<style>
  .note {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .note.err {
    color: var(--coral-deep);
  }
  .muted {
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .grow {
    flex: 1;
  }
  .mini {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 26px;
    padding: 0 9px;
    border: 0;
    border-radius: var(--r-pill);
    background: transparent;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .mini:hover {
    background: var(--hover);
  }
  .pre {
    margin: 0;
    padding: 12px 14px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-2-bg);
    font-size: 12.5px;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .assembled {
    padding: 10px 14px;
    border-radius: var(--r-md);
    background: var(--lilac-soft);
  }
  .assembled .label {
    color: var(--lilac-deep);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .assembled p {
    margin: 4px 0 0;
    white-space: pre-wrap;
  }
  .events-head {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-sm);
    font-weight: 600;
    color: var(--text-2);
  }
  .events {
    list-style: none;
    margin: 0;
    padding: 0;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line-soft);
    overflow: hidden;
  }
  .ev + .ev {
    border-top: var(--line-w-thin) solid var(--line-soft);
  }
  .ev-row {
    display: grid;
    grid-template-columns: 170px 1fr;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 32px;
    padding: 0 12px;
    border: 0;
    background: transparent;
    text-align: left;
    font-size: 12px;
  }
  .ev-row:hover {
    background: var(--hover);
  }
  .ev-name {
    font-weight: 600;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ev-data {
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ev-json {
    padding: 0 10px 10px;
  }
</style>

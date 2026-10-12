<!--
  JSON 查看器的一个节点（递归）。对象与数组可折叠，折叠时显示「{…} 5 项」摘要；
  长字符串截断，可展开；打码的密钥值（含 ***）用粉色底和小锁标出。
  「全部展开 / 全部折叠」由 JsonView 通过 command 广播，单个节点的手动开合在两次广播之间有效。
-->
<script lang="ts">
  import type { Json } from '../lib/types';
  import { t } from '../lib/i18n';
  import Icon from './Icon.svelte';
  import Self from './JsonNode.svelte';

  let {
    name,
    value,
    depth,
    openDepth,
    command,
    last = true,
  }: {
    name: string | number | null;
    value: Json;
    depth: number;
    openDepth: number;
    command: { gen: number; open: boolean };
    last?: boolean;
  } = $props();

  const MAX_STR = 140;

  // 初始开合只取一次：之后由用户点击或「全部展开/折叠」决定，不随 props 回弹
  const initiallyOpen = (() => depth < openDepth)();
  let open = $state(initiallyOpen);
  let fullText = $state(false);

  $effect(() => {
    if (command.gen > 0) open = command.open;
  });

  const isArr = $derived(Array.isArray(value));
  const isObj = $derived(value !== null && typeof value === 'object');
  const entries = $derived(
    isObj ? (isArr ? (value as Json[]).map((v, i) => [i, v] as const) : Object.entries(value as Record<string, Json>)) : [],
  );
</script>

<div class="node" style="--d: {depth}">
  {#if isObj}
    <button class="row toggle" onclick={() => (open = !open)} aria-expanded={open}>
      <span class="chev" class:open><Icon name="chevronRight" size={12} stroke={2.6} /></span>
      {#if name !== null}<span class="key" class:idx={typeof name === 'number'}>{typeof name === 'number' ? name : `"${name}"`}</span><span class="colon">:</span>{/if}
      {#if open}
        <span class="brace">{isArr ? '[' : '{'}</span>
      {:else}
        <span class="brace">{isArr ? '[' : '{'}</span><span class="summary">{t(isArr ? 'json.items' : 'json.keys', { n: entries.length })}</span><span class="brace">{isArr ? ']' : '}'}</span>{#if !last}<span class="comma">,</span>{/if}
      {/if}
    </button>
    {#if open}
      <div class="children">
        {#each entries as [k, v], i (k)}
          <Self name={k} value={v} depth={depth + 1} {openDepth} {command} last={i === entries.length - 1} />
        {/each}
      </div>
      <div class="row close"><span class="brace">{isArr ? ']' : '}'}</span>{#if !last}<span class="comma">,</span>{/if}</div>
    {/if}
  {:else}
    <div class="row leaf">
      {#if name !== null}<span class="key" class:idx={typeof name === 'number'}>{typeof name === 'number' ? name : `"${name}"`}</span><span class="colon">:</span>{/if}
      {#if typeof value === 'string'}
        {@const secret = value.includes('***')}
        <span class="val str" class:secret>
          {#if secret}<Icon name="lock" size={11} stroke={2.4} />{/if}"{fullText || value.length <= MAX_STR ? value : value.slice(0, MAX_STR)}{#if !fullText && value.length > MAX_STR}…{/if}"
        </span>
        {#if !fullText && value.length > MAX_STR}
          <button class="more" onclick={() => (fullText = true)}>{t('json.more', { n: value.length - MAX_STR })}</button>
        {/if}
      {:else if typeof value === 'number'}
        <span class="val num">{value}</span>
      {:else if typeof value === 'boolean'}
        <span class="val bool">{value}</span>
      {:else}
        <span class="val null">null</span>
      {/if}{#if !last}<span class="comma">,</span>{/if}
    </div>
  {/if}
</div>

<style>
  .node {
    font-family: var(--font-mono);
    font-size: 12.5px;
    line-height: 1.6;
  }
  .row {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 0;
    padding-left: 18px;
    min-width: 0;
  }
  .toggle {
    position: relative;
    width: 100%;
    border: 0;
    background: transparent;
    text-align: left;
    font: inherit;
    color: inherit;
    border-radius: 6px;
  }
  .toggle:hover {
    background: var(--hover);
  }
  .chev {
    position: absolute;
    left: 2px;
    top: 5px;
    display: inline-flex;
    color: var(--text-3);
    transition: transform var(--dur-mid) var(--ease-spring);
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .children {
    margin-left: 8px;
    padding-left: 8px;
    border-left: 1.5px dotted var(--line-soft);
  }
  .key {
    color: var(--text);
  }
  .key.idx {
    color: var(--text-3);
  }
  .colon,
  .comma,
  .brace {
    color: var(--text-3);
  }
  /* 冒号紧贴键名、后面留一格，和平常写 JSON 的样子一致 */
  .colon {
    margin-right: 0.6em;
  }
  .summary {
    margin: 0 4px;
    padding: 0 6px;
    border-radius: var(--r-pill);
    background: var(--card-2-bg);
    color: var(--text-3);
    font-family: var(--font-ui);
    font-size: var(--fs-xs);
  }
  .val {
    word-break: break-word;
    white-space: pre-wrap;
    min-width: 0;
  }
  .str {
    color: var(--mint-deep);
  }
  .str.secret {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 0 5px;
    border-radius: 5px;
    background: var(--pink-soft);
    color: var(--pink-deep);
  }
  .num {
    color: var(--sky-deep);
  }
  .bool {
    color: var(--lilac-deep);
  }
  .null {
    color: var(--text-3);
    font-style: italic;
  }
  .more {
    margin-left: 6px;
    border: 0;
    padding: 0 6px;
    border-radius: var(--r-pill);
    background: var(--lilac-soft);
    color: var(--lilac-deep);
    font-family: var(--font-ui);
    font-size: var(--fs-xs);
  }
</style>

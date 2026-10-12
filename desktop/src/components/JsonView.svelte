<!--
  可折叠的 JSON 查看器。默认展开前两层（请求体的 messages、tools 等一眼可见，又不至于一屏铺满）。
-->
<script lang="ts">
  import type { Json } from '../lib/types';
  import { t } from '../lib/i18n';
  import JsonNode from './JsonNode.svelte';

  let { value, openDepth = 2 }: { value: Json; openDepth?: number } = $props();

  const command = $state({ gen: 0, open: true });

  function all(open: boolean) {
    command.open = open;
    command.gen += 1;
  }
</script>

<div class="json">
  <div class="tools">
    <button onclick={() => all(true)}>{t('json.expandAll')}</button>
    <button onclick={() => all(false)}>{t('json.collapseAll')}</button>
  </div>
  <JsonNode name={null} {value} depth={0} {openDepth} {command} />
</div>

<style>
  .json {
    position: relative;
    padding: 10px 12px 12px 4px;
    border-radius: var(--r-md);
    border: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-2-bg);
    overflow-x: auto;
  }
  .tools {
    position: absolute;
    top: 8px;
    right: 8px;
    z-index: 1;
    display: flex;
    gap: 4px;
  }
  .tools button {
    height: 22px;
    padding: 0 8px;
    border-radius: var(--r-pill);
    border: var(--line-w-thin) solid var(--line-soft);
    background: var(--card-bg);
    color: var(--text-2);
    font-size: var(--fs-xs);
  }
  .tools button:hover {
    border-color: var(--line);
    color: var(--text);
  }
</style>

<!--
  扩展挂进某个位置的面板（api.ui.mount）。没有扩展挂东西时什么也不渲染，页面的排版不受影响。
-->
<script lang="ts">
  import { host } from '../lib/extensions/host.svelte';
  import type { MountPoint } from '../lib/extensions/api.svelte';
  import { packages, packageName } from '../lib/packages.svelte';
  import ExtRender from './ExtRender.svelte';

  let { point, ctx = {} }: { point: MountPoint; ctx?: Record<string, unknown> } = $props();

  const list = $derived(host.mounts.filter((m) => m.point === point));

  function nameOf(id: string): string {
    const e = packages.get(id);
    return e ? packageName(e) : id;
  }
</script>

{#if list.length}
  <div class="mounts">
    {#each list as m (m.id)}
      <ExtRender render={m.render} {ctx} name={nameOf(m.extension)} />
    {/each}
  </div>
{/if}

<style>
  .mounts {
    display: grid;
    gap: 10px;
    margin-bottom: 16px;
  }
</style>

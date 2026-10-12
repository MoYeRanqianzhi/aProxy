<!--
  翻牌数字：每位数字一块小牌，数值变化时只有变了的那几位翻过去。
  用在「救回的请求数」与重试次数上——计数在变，就是 aProxy 在干活的证据。
-->
<script lang="ts">
  import type { TransitionConfig } from 'svelte/transition';
  import { ui } from '../lib/state.svelte';
  import { springOut } from '../lib/motion';

  let { value, tiles = true, label }: { value: number; tiles?: boolean; label?: string } = $props();

  const chars = $derived(new Intl.NumberFormat(ui.lang).format(value).split(''));

  // 新数字从下方翻上来，旧数字向上翻走；减少动效时只做淡入淡出
  function flipIn(_n: Element): TransitionConfig {
    if (ui.reducedMotion) return { duration: 120, css: (t) => `opacity:${t}` };
    return {
      duration: 520,
      css: (t) => {
        const s = springOut(t);
        return `transform: perspective(240px) translateY(${(1 - s) * 55}%) rotateX(${(1 - s) * -80}deg); opacity:${Math.min(1, t * 2)}`;
      },
    };
  }
  function flipOut(_n: Element): TransitionConfig {
    if (ui.reducedMotion) return { duration: 100, css: (t) => `opacity:${t}` };
    return {
      duration: 260,
      css: (t, u) => `transform: perspective(240px) translateY(${-u * 55}%) rotateX(${u * 80}deg); opacity:${t}`,
    };
  }
</script>

<span class="flip" class:tiles>
  <span class="sr-only">{label ?? chars.join('')}</span>
  {#each chars as c, i (chars.length - i)}
    {#if /\d/.test(c)}
      <span class="cell" aria-hidden="true">
        {#key c}
          <span class="digit" in:flipIn out:flipOut>{c}</span>
        {/key}
      </span>
    {:else}
      <span class="sep" aria-hidden="true">{c}</span>
    {/if}
  {/each}
</span>

<style>
  .flip {
    display: inline-flex;
    align-items: stretch;
    gap: 3px;
    font-variant-numeric: tabular-nums;
    line-height: 1;
  }
  .cell {
    position: relative;
    display: inline-grid;
    place-items: center;
    width: 0.66em;
    height: 1.1em;
    overflow: hidden;
  }
  .tiles .cell {
    width: 0.78em;
    height: 1.18em;
    border-radius: 0.2em;
    background: var(--card);
    border: var(--line-w) solid var(--line);
    box-shadow: var(--shadow-1);
  }
  /* 牌面中间那道缝：翻牌计数器的标志 */
  .tiles .cell::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    height: 1px;
    background: var(--line);
  }
  .digit {
    grid-area: 1 / 1;
    display: block;
    backface-visibility: hidden;
  }
  .sep {
    align-self: flex-end;
    padding: 0 0.02em;
  }
</style>

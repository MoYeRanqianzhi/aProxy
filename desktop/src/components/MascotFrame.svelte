<!--
  自定义吉祥物的宿主：别人写的吉祥物整个跑在一个 sandbox iframe 里（allow-scripts，没有 allow-same-origin）。
  iframe 的源是不透明的，碰不到界面的 DOM、存储与 IPC；它画错、抛异常或者卡死，坏的只是这一块——隔离是为了稳定。

  界面这边只做两件事：经 Backend 取到吉祥物的源码（store.readMascot）原样交给沙箱，再把每次的请求（MascotState
  或图标参数）发过去。画面完全在沙箱里，界面拿不到也不需要。沙箱一侧的协议见 public/mascot-sandbox.js。
  消息只认来自这个 iframe 的；发出去的目标写 '*'，因为沙箱的源是不透明的「null」，没法点名，内容也不敏感。
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { store } from '../lib/store.svelte';
  import type { MascotRequest } from './MascotView.svelte';

  let {
    id,
    request,
    replay = 0,
    freezeAt,
  }: { id: string; request: MascotRequest; replay?: number; freezeAt?: number } = $props();

  const SRC = `${import.meta.env.BASE_URL}mascot-sandbox.html`;

  /** 组件销毁时 bind:this 会把它清回 null；读源码是异步的，回来时组件可能已经不在了（翻页、换吉祥物） */
  let frame: HTMLIFrameElement | null = null;
  /** 沙箱里的定义加载好了，之后的请求才发得过去 */
  let loaded = $state(false);

  onMount(() => {
    const onMessage = (ev: MessageEvent) => {
      if (ev.source !== frame?.contentWindow) return;
      const msg: unknown = ev.data;
      if (typeof msg !== 'object' || msg === null) return;
      const { type, message } = msg as { type?: unknown; message?: unknown };
      if (type === 'ready') {
        store.readMascot(id).then(
          (code) => frame?.contentWindow?.postMessage({ type: 'load', code }, '*'),
          (err) => console.warn(`[mascot:${id}] could not read its code`, err),
        );
      } else if (type === 'loaded') {
        loaded = true;
      } else if (type === 'error') {
        console.warn(`[mascot:${id}]`, String(message));
      }
    };
    addEventListener('message', onMessage);
    return () => removeEventListener('message', onMessage);
  });

  // 状态一变就整份发过去；标记没变时沙箱不会动 DOM，动画不会从头播
  $effect(() => {
    const msg = { type: 'render', ...request, replay, freezeAt };
    if (loaded) frame?.contentWindow?.postMessage(msg, '*');
  });
</script>

<iframe bind:this={frame} class="sandbox" src={SRC} sandbox="allow-scripts" title="mascot" aria-hidden="true" tabindex="-1"></iframe>

<style>
  /* color-scheme 与沙箱页保持一致（都是默认的 normal）：两边不一致时 Chromium 会给 iframe 垫一层不透明底色，
     深色主题下吉祥物就会顶着一块白方块 */
  .sandbox {
    display: block;
    width: 100%;
    height: 100%;
    border: 0;
    background: transparent;
    color-scheme: normal;
    pointer-events: none;
  }
</style>

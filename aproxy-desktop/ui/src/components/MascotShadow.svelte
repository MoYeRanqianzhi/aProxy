<!--
  内置吉祥物的宿主：直接调用随界面编译的定义，把返回的 SVG 放进一个独占的 shadow root。
  shadow root 让吉祥物的样式和界面互不干扰，try/catch 让一个画错的吉祥物只是「不显示」。

  只在标记文本真的变了时才替换 DOM：动画不会因为无关的刷新从头播，
  而「状态变化 = 重新挂载」这条约定让吉祥物可以把一次性动画（被撞、庆祝）放在挂载时播。
-->
<script lang="ts">
  import type { MascotDefinition } from '../lib/mascot/types';
  import type { MascotRequest } from './MascotView.svelte';

  let {
    def,
    request,
    replay = 0,
    freezeAt,
  }: { def: MascotDefinition; request: MascotRequest; replay?: number; freezeAt?: number } = $props();

  // shadow root 里的基础样式：让吉祥物的 <svg> 填满宿主给的方框
  const BASE = ':host{display:block;width:100%;height:100%}.stage{display:block;width:100%;height:100%;line-height:0}.stage>svg{width:100%;height:100%}';

  let host: HTMLSpanElement;
  let stage: HTMLSpanElement | null = null;
  // 用可构造样式表而不是 style 元素（组件里不能出现这个标签的字面量，Svelte 的预处理会把它当成样式块）：
  // 界面的 CSP 不允许内联样式，可构造样式表不受这条限制
  let sheet: CSSStyleSheet | null = null;
  let shownMarkup = '';
  let shownCss: string | null = null;
  let shownReplay = 0;

  $effect(() => {
    if (!stage) {
      const root = host.attachShadow({ mode: 'open' });
      sheet = new CSSStyleSheet();
      root.adoptedStyleSheets = [sheet];
      stage = document.createElement('span');
      stage.className = 'stage';
      root.append(stage);
    }

    let markup = '';
    let css = '';
    try {
      css = def.css ?? '';
      markup = 'icon' in request ? def.icon(request.icon) : def.render(request.state);
      if (typeof markup !== 'string') throw new Error('render() must return a string');
    } catch (err) {
      console.error(`[mascot:${def.id}] render failed`, err);
      markup = '';
    }

    if (css !== shownCss && sheet) {
      sheet.replaceSync(BASE + css);
      shownCss = css;
    }
    if (markup !== shownMarkup || replay !== shownReplay) {
      stage.innerHTML = markup;
      shownMarkup = markup;
      shownReplay = replay;
    }

    if (freezeAt !== undefined) {
      // getAnimations() 会先同步算一次样式，所以刚插进去的 CSS 动画此刻已经存在
      for (const a of host.shadowRoot?.getAnimations() ?? []) {
        a.pause();
        a.currentTime = freezeAt;
      }
    }
  });
</script>

<span bind:this={host} class="shadow-host"></span>

<style>
  .shadow-host {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>

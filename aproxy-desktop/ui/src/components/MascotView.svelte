<!--
  吉祥物的宿主：把界面状态整理成 MascotState 交给当前吉祥物，再把画面放进一块它独占的地方。

  为什么要隔离：吉祥物可以替换，用户还能装别人写的形象。隔离是为了稳定——一个画错、抛异常或卡死的吉祥物
  只是「不显示」，而不是把标题栏一起拖垮。两种来源两种隔离：
    - 内置的（随界面编译、带定义）由 MascotShadow 在 shadow root 里直接渲染，样式互不干扰，try/catch 接住异常；
    - 用户装的自定义吉祥物由 MascotFrame 放进 sandbox iframe，代码和画面都在不透明源里，界面从不执行它的代码。
  吉祥物拿到的只有 MascotState（状态、尺寸档位、是否减少动效、解析好的颜色），不依赖界面的 DOM 或全局状态；
  两种宿主拿到的请求在这里一次备齐，完全一样。

  props：
    mood / retries / size   交给吉祥物的状态
    px                      容器边长（默认按档位：icon 22、small 48、hero 128）
    icon                    设了就改为调用 icon({ px: icon })，用来预览软件图标与托盘图标
    id                      指定某个吉祥物（预览页用）；不设就用设置里选中的那个
    still                   强制静态（信息密集的页面里用）；用户开启减少动效时本来就是静态
    palette                 覆盖部分颜色（预览托盘图标时把 surface 换成任务栏底色）
    replay                  数值变化时强制重新挂载一次，重播一次性动画（预览页用）
    freezeAt                把所有动画停在第 N 毫秒（预览页的分帧图用）
    label                   有值时作为图片读出；否则对读屏隐藏（旁边总有文字说明状态）
-->
<script lang="ts" module>
  import type { MascotIconOptions, MascotState } from '../lib/mascot/types';

  /** 交给吉祥物的一次请求：按状态渲染，或者出一张静态图标 */
  export type MascotRequest = { state: MascotState } | { icon: MascotIconOptions };
</script>

<script lang="ts">
  import { ui } from '../lib/state.svelte';
  import { mascots } from '../lib/mascot/registry.svelte';
  import { mascotPalette } from '../lib/mascot/palette.svelte';
  import type { MascotMood, MascotPalette, MascotSize } from '../lib/mascot/types';
  import MascotShadow from './MascotShadow.svelte';
  import MascotFrame from './MascotFrame.svelte';

  let {
    mood,
    retries = 0,
    size = 'hero',
    px,
    icon,
    id,
    still = false,
    palette,
    replay = 0,
    freezeAt,
    label,
  }: {
    mood: MascotMood;
    retries?: number;
    size?: MascotSize;
    px?: number;
    icon?: number;
    id?: string;
    still?: boolean;
    palette?: Partial<MascotPalette>;
    replay?: number;
    freezeAt?: number;
    label?: string;
  } = $props();

  const DEFAULT_PX: Record<MascotSize, number> = { icon: 22, small: 48, hero: 128 };

  const entry = $derived(id ? mascots.get(id) : mascots.current);
  const box = $derived(px ?? icon ?? DEFAULT_PX[size]);

  // 展开成普通对象：调色板是响应式代理，而沙箱那条路要经 postMessage 结构化克隆，代理克隆不了
  const pal = $derived<MascotPalette>({ ...mascotPalette.current, ...palette });
  const request = $derived<MascotRequest>(
    icon
      ? { icon: { px: icon, mood, theme: ui.variant, palette: pal } }
      : { state: { mood, retries, size, reducedMotion: still || ui.reducedMotion, theme: ui.variant, palette: pal } },
  );
</script>

<span
  class="mascot-host"
  style:width="{box}px"
  style:height="{box}px"
  role={label ? 'img' : undefined}
  aria-label={label || undefined}
  aria-hidden={label ? undefined : 'true'}
>
  {#if entry?.def}
    <MascotShadow def={entry.def} {request} {replay} {freezeAt} />
  {:else if entry}
    <!-- 换了一个自定义吉祥物就换一个沙箱：旧代码留下的任何东西都不带过去 -->
    {#key entry.id}
      <MascotFrame id={entry.id} {request} {replay} {freezeAt} />
    {/key}
  {/if}
</span>

<style>
  .mascot-host {
    flex: none;
    display: inline-block;
    vertical-align: middle;
  }
</style>

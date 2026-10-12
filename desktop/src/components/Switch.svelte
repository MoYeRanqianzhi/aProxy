<!--
  开关：按下时圆钮被挤扁（横向拉长），松开后弹簧弹到另一头。
  打开为薄荷色。禁用时降低不透明度并改成「不可点」的光标，原因由调用方在旁边写明。
-->
<script lang="ts">
  let {
    checked = $bindable(false),
    disabled = false,
    label,
    onchange,
  }: { checked?: boolean; disabled?: boolean; label: string; onchange?: (v: boolean) => void } = $props();

  function toggle() {
    if (disabled) return;
    checked = !checked;
    onchange?.(checked);
  }
</script>

<button
  type="button"
  class="switch"
  class:on={checked}
  role="switch"
  aria-checked={checked}
  aria-label={label}
  {disabled}
  onclick={toggle}
>
  <span class="knob"></span>
</button>

<style>
  .switch {
    --w: 46px;
    --h: 26px;
    --k: 18px;
    /* 圆钮四周留白：由轨道内高与圆钮直径算出，描边粗细变了圆钮也仍然居中 */
    --gap: calc((var(--h) - 2 * var(--line-w) - var(--k)) / 2);
    /* 打开时圆钮要走的距离 = 轨道内宽 - 圆钮 - 两侧留白 */
    --travel: calc(var(--w) - 2 * var(--line-w) - var(--k) - 2 * var(--gap));
    position: relative;
    flex: none;
    width: var(--w);
    height: var(--h);
    padding: 0;
    border-radius: var(--r-pill);
    /* 关着时轨道与卡片底色接近，开/关这个状态要靠轨道边界认出来，所以用 ≥3:1 的强边，
       再加一道很浅的内凹阴影，让轨道读作「槽」而不是又一个按钮 */
    border: var(--line-w) solid var(--line-strong);
    background: var(--card-2-bg);
    box-shadow: inset 0 1px 3px oklch(0.3 0.05 290 / 0.12);
    transition:
      background-color var(--dur-mid) var(--ease-soft),
      border-color var(--dur-mid) var(--ease-soft);
  }
  .switch.on {
    background: var(--mint);
    border-color: color-mix(in oklab, var(--mint) 70%, var(--mint-deep));
  }
  .knob {
    position: absolute;
    top: var(--gap);
    left: var(--gap);
    width: var(--k);
    height: var(--k);
    border-radius: var(--r-pill);
    background: var(--card);
    /* 圆钮不描边，靠一层贴身投影从轨道上「浮」出来 */
    box-shadow:
      0 1px 2px oklch(0.3 0.05 290 / 0.22),
      0 2px 6px -1px oklch(0.3 0.05 290 / 0.2);
    transition:
      transform var(--dur-slow) var(--ease-spring),
      width var(--dur-mid) var(--ease-spring);
  }
  .on .knob {
    transform: translateX(var(--travel));
  }
  /* 挤压：按住时圆钮变宽，向行进方向「蓄力」 */
  .switch:active:not(:disabled) .knob {
    width: calc(var(--k) + 7px);
  }
  /* 打开状态下按住：圆钮向左拉宽，右边缘保持不动，所以起点要往回退拉宽的那 7px */
  .on:active:not(:disabled) .knob {
    transform: translateX(calc(var(--travel) - 7px));
  }
  .switch:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
</style>

<!--
  实例页的 format 卡片：这次运行的 [[format]] 条目，各自的程序与版本、请求这一路的熔断器、有没有控制接口，
  以及「不换实例、只重启 format 进程」的按钮。

  条目不进快照（卡片每次重读都去问实例太贵），这里单独取：运行换了、通道重新接上、做完一次重启都重取，
  停在这一页时每 15 秒再取一次，熔断器打开或恢复能跟上。没有条目的实例整块不显示。

  重启要确认：在途的请求不受影响，但新进程握手失败时，必需的条目会让之后的请求失败，这一点要让人点之前就知道。
-->
<script lang="ts">
  import type { FormatView, RestartOutcome, Run } from '../lib/backend/contract';
  import { toFailure } from '../lib/backend/link';
  import { faultText } from '../lib/faults';
  import { ago, hasMessage, t } from '../lib/i18n';
  import { overlays } from '../lib/overlays.svelte';
  import { runKey, store } from '../lib/store.svelte';
  import Icon from './Icon.svelte';

  let { run, controllable }: { run: Run | null; controllable: boolean } = $props();

  let rows = $state.raw<FormatView[]>([]);
  /** 正在重启的条目名；null 键表示「全部」。重启进行中按钮不可再点 */
  let restarting = $state<string | null | undefined>(undefined);
  /** 每做完一次重启加一，让下面的 effect 重取 */
  let bump = $state(0);

  const id = $derived(run ? runKey(run) : null);

  $effect(() => {
    void id;
    void store.epoch;
    void bump;
    const current = run;
    if (!current) {
      rows = [];
      return;
    }
    let live = true;
    const read = () =>
      store.formatList(current).then(
        (list) => {
          if (live) rows = list;
        },
        // 取不到（实例正在停、旧版本实例）时保留上一次的结果，不在这里报错：实例卡片本身会显示状态
        () => {},
      );
    void read();
    const timer = setInterval(read, 15_000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  });

  /** 握手失败的原因是 format 故障的类别；认得的翻成一句话，认不出的原样显示 */
  function reasonText(kind: string): string {
    const key = `fmt.reason.${kind}`;
    return hasMessage(key) ? t(key) : kind;
  }

  function report(outcomes: RestartOutcome[]) {
    const failed = outcomes.filter((o) => o.outcome === 'handshake-failed');
    for (const o of failed) {
      if (o.outcome === 'handshake-failed') overlays.toast(t('fmt.toast.failed', { name: o.name, reason: reasonText(o.reason) }), 'coral');
    }
    const ok = outcomes.length - failed.length;
    if (ok === 1 && outcomes.length === 1) overlays.toast(t('fmt.toast.restarted', { name: outcomes[0].name }));
    else if (ok > 0) overlays.toast(t('fmt.toast.restartedN', { n: ok }));
  }

  function askRestart(name: string | null) {
    const current = run;
    if (!current) return;
    overlays.ask({
      title: name === null ? t('fmt.confirm.all') : t('fmt.confirm.one', { name }),
      body: t('fmt.confirm.body'),
      okLabel: t('fmt.confirm.ok'),
      danger: false,
      onOk: async () => {
        restarting = name;
        try {
          report(await store.formatRestart(current, name === null ? null : [name]));
        } catch (raw) {
          overlays.toast(faultText(toFailure(raw)), 'coral');
        } finally {
          restarting = undefined;
          bump++;
        }
      },
    });
  }

  const breakerTone = { closed: 'mint', open: 'coral', probing: 'butter' } as const;
</script>

{#if rows.length}
  <section class="panel formats">
    <div class="head">
      <h2 class="section-title">{t('fmt.title')}<span class="count">{rows.length}</span></h2>
      {#if rows.length > 1}
        <button class="btn sm" disabled={!controllable || restarting !== undefined} onclick={() => askRestart(null)}>
          <Icon name="restart" size={14} />{t('fmt.restartAll')}
        </button>
      {/if}
    </div>
    <ul>
      {#each rows as f (f.name)}
        <li>
          <div class="r1">
            <code class="name mono">{f.name}</code>
            <span class="chip {breakerTone[f.breaker]}"><span class="dot {breakerTone[f.breaker]}"></span>{t(`fmt.breaker.${f.breaker}`)}</span>
            {#if f.required}<span class="chip outlined">{t('fmt.required')}</span>{/if}
            {#if f.control}<span class="chip lilac">{t('fmt.control')}</span>{/if}
            <button
              class="btn sm ghost restart"
              disabled={!controllable || restarting !== undefined}
              aria-label="{t('fmt.restart')} {f.name}"
              onclick={() => askRestart(f.name)}
            >
              <Icon name="restart" size={13} class={restarting === f.name || restarting === null ? 'spin' : ''} />{t('fmt.restart')}
            </button>
          </div>
          <div class="r2">
            {#if f.program}
              <span class="program">{f.program}{#if f.version}<span class="ver">{f.version}</span>{/if}</span>
            {:else}
              <span class="muted">{t('fmt.notNegotiated')}</span>
            {/if}
            <span class="sep" aria-hidden="true">·</span>
            <span>{t(`fmt.process.${f.process}`)}</span>
            <span class="sep" aria-hidden="true">·</span>
            <code class="mono stages">{f.stages.join(' / ')}</code>
          </div>
          {#if f.restartedAt !== null}
            <div class="r3">{t('fmt.restartedAt', { ago: ago(store.now - f.restartedAt) })}</div>
          {/if}
        </li>
      {/each}
    </ul>
    <p class="note">{t('fmt.note')}</p>
  </section>
{/if}

<style>
  .formats {
    padding: 16px 18px 14px;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  ul {
    display: grid;
    gap: 2px;
    margin: 10px 0 0;
    padding: 0;
    list-style: none;
  }
  li {
    display: grid;
    gap: 4px;
    padding: 10px 0;
    border-top: var(--line-w-thin) solid var(--line);
  }
  li:first-child {
    border-top: none;
  }
  .r1 {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .name {
    font-weight: 600;
    color: var(--text);
    margin-right: 2px;
  }
  .restart {
    margin-left: auto;
  }
  .r2 {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px;
    font-size: var(--fs-sm);
    color: var(--text-2);
  }
  .program {
    color: var(--text);
  }
  .ver {
    margin-left: 0.35em;
    color: var(--text-2);
  }
  .sep {
    color: var(--text-3);
  }
  .stages {
    font-size: var(--fs-xs);
  }
  .r3 {
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
  .muted {
    color: var(--text-3);
  }
  .note {
    margin: 8px 0 0;
    font-size: var(--fs-xs);
    line-height: 1.5;
    color: var(--text-3);
  }
  :global(.formats .spin) {
    animation: fmt-spin 0.9s linear infinite;
  }
  @keyframes fmt-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    :global(.formats .spin) {
      animation: none;
    }
  }
</style>

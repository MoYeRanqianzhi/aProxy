/*
 * 吉祥物注册表：界面只通过它拿「当前选中的那一个」，不直接引用任何具体形象。
 *
 * 两种来源：
 *   - 内置的几种随界面一起编译，带着定义（def），由 MascotView 在 shadow root 里直接渲染；
 *   - 用户装的自定义吉祥物来自扩展列表（kind 为 mascot 的条目），这里只记名字和装在哪——
 *     它们是别人写的代码，只在沙箱 iframe 里运行（见 MascotFrame），界面这边从不持有、也不执行它的函数。
 * 设置页的吉祥物列表、标题栏、总览都只看条目，不必关心它是哪来的。
 * 选择本身（含「不显示」）是用户偏好，存在 ui.mascotId。
 */
import { ui } from '../state.svelte';
import type { MascotDefinition } from './types';
import { tumbler } from './tumbler';
import { courier } from './courier';
import { orb } from './orb';

export type MascotSource = 'builtin' | 'custom';

export interface MascotEntry {
  id: string;
  name: MascotDefinition['name'];
  description?: MascotDefinition['description'];
  source: MascotSource;
  /** 只有内置的有：界面直接调用它渲染。自定义的没有，它的代码只在沙箱里跑 */
  def?: MascotDefinition;
  /** 自定义吉祥物装在哪里（显示给用户看，方便找到文件删除） */
  origin?: string;
}

/** 「不显示」在设置里是一个和吉祥物并列的选项，用一个不可能撞上的 id 表示 */
export const NO_MASCOT = 'none';

const BUILTINS: MascotEntry[] = [tumbler, courier, orb].map((def) => ({
  id: def.id,
  name: def.name,
  description: def.description,
  source: 'builtin',
  def,
}));

class MascotRegistry {
  // raw：条目是不可变的普通对象（内置的里面还有函数），不需要深层代理；变化时整体换数组
  entries = $state.raw<MascotEntry[]>(BUILTINS);

  get(id: string): MascotEntry | undefined {
    return this.entries.find((e) => e.id === id);
  }

  /** 当前选中的吉祥物；选了「不显示」或选中的已被卸载时为 undefined，调用方据此把位置让出来 */
  get current(): MascotEntry | undefined {
    return ui.mascotId === NO_MASCOT ? undefined : this.get(ui.mascotId);
  }

  /**
   * 换上当前装好的自定义吉祥物。清单来自扩展，是系统边界：id 撞上内置的或「不显示」的直接跳过，
   * 免得一个自定义吉祥物顶掉内置形象。只拼内置常量与传进来的清单、不读 entries，
   * 所以可以放在 $effect 里调用而不会自己触发自己。
   */
  setCustom(list: { id: string; name: MascotEntry['name']; description?: MascotEntry['description']; origin: string }[]) {
    const taken = new Set([NO_MASCOT, ...BUILTINS.map((e) => e.id)]);
    const custom: MascotEntry[] = [];
    for (const m of list) {
      if (!m.id || taken.has(m.id)) continue;
      taken.add(m.id);
      custom.push({ id: m.id, name: m.name, description: m.description, source: 'custom', origin: m.origin });
    }
    this.entries = [...BUILTINS, ...custom];
  }
}

export const mascots = new MascotRegistry();

/** 按界面语言取显示名，缺了就退回英文 */
export function mascotName(entry: MascotEntry): string {
  return entry.name[ui.lang] ?? entry.name.en;
}

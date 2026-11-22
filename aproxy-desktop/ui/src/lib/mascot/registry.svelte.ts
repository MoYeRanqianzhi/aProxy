/*
 * 吉祥物注册表：界面只通过它拿「当前选中的那一个」，不直接引用任何具体形象。
 *
 * 两种来源，都带着定义（def），由 MascotView 在 shadow root 里渲染：
 *   - 内置的几种随界面一起编译；
 *   - 启用的扩展经 api.ui.registerMascot 加进来的，扩展停用时随之撤掉。
 * 设置页的吉祥物列表、标题栏、总览都只看条目，不必关心它是哪来的。
 * 选择本身（含「不显示」）是用户偏好，存在 ui.mascotId；选中的那个不在了，就回落到「不显示」以外的默认形象。
 */
import { ui } from '../state.svelte';
import type { MascotDefinition } from './types';
import { tumbler } from './tumbler';
import { courier } from './courier';
import { orb } from './orb';

export type MascotSource = 'builtin' | 'extension';

export interface MascotEntry {
  id: string;
  name: MascotDefinition['name'];
  description?: MascotDefinition['description'];
  source: MascotSource;
  def: MascotDefinition;
  /** 扩展加的：是哪个扩展 */
  extension?: string;
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
   * 扩展加一个吉祥物，返回撤掉它的函数。定义来自扩展：id 撞上内置的、「不显示」或别的扩展已经加的，
   * 就不加（返回的函数什么也不做），免得一个扩展顶掉别人的形象；名字缺了 en 的用 id 顶上
   */
  addExtension(extension: string, def: MascotDefinition): () => void {
    const id = String(def.id);
    if (!id || id === NO_MASCOT || this.entries.some((e) => e.id === id)) return () => {};
    const name = def.name && typeof def.name.en === 'string' ? def.name : { en: id };
    const entry: MascotEntry = { id, name, description: def.description, source: 'extension', def, extension };
    this.entries = [...this.entries, entry];
    return () => {
      this.entries = this.entries.filter((e) => e !== entry);
    };
  }
}

export const mascots = new MascotRegistry();

/** 按界面语言取显示名，缺了就退回英文 */
export function mascotName(entry: MascotEntry): string {
  return entry.name[ui.lang] ?? entry.name.en;
}

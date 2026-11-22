/*
 * 界面用的那一条传输层：桌面窗口里经 Tauri 发给 Rust，浏览器里（开发、截图）用演示数据。
 * 两者对 store 完全一样（见 link.ts），除了这里，界面的其余代码不知道自己接的是哪一个。
 *
 * 演示实现也打进了生产构建：选哪个要到运行时才知道（isTauri），打包时删不掉。它不碰任何系统资源，
 * 只在页面不在 aProxy 窗口里时才会被用到，标题栏据 link.demo 常驻标明「演示数据」。
 */
import { isTauri } from '@tauri-apps/api/core';
import type { Link } from './link';
import { tauriLink } from './tauri';
import { demoLink } from './demo';

export const link: Link = isTauri() ? tauriLink : demoLink;

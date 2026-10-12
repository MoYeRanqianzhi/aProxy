/*
 * 界面使用的那一个 Backend。现在桌面窗口与浏览器里用的都是 mock（预览版的演示数据，界面据它的 demo 标明）；
 * 真实数据接进来时在这里按运行环境换成 Tauri IPC 实现，将来的 Web UI 换成 HTTP/WebSocket 实现——
 * 除了这一行，界面的其余代码都不用改。
 */
import type { Backend } from './types';
import { mockBackend } from './mock';

export const backend: Backend = mockBackend;

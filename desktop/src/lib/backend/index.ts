/*
 * 界面使用的那一个 Backend。原型里只有 mock；接入 Tauri 时在这里按运行环境换成 IPC 实现，
 * 将来的 Web UI 换成 HTTP/WebSocket 实现——除了这一行，界面的其余代码都不用改。
 */
import type { Backend } from './types';
import { mockBackend } from './mock';

export const backend: Backend = mockBackend;

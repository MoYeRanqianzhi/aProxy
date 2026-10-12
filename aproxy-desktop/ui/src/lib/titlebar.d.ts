// Type declarations for titlebar.js. Copy both files side by side; TypeScript projects then import
// './titlebar.js' under `strict` without `allowJs`.
import type { Window } from '@tauri-apps/api/window';

/** True inside a Windows WebView2 host; the Windows helpers are no-ops elsewhere. */
export declare const isWindows: boolean;

/** Keeps --titlebar-top / --titlebar-height / --titlebar-right in sync on `root`. Returns a disposer. */
export declare function watchTitlebarInsets(root?: HTMLElement): () => void;

/** Paints the native caption behind the buttons; `background` must be "#rrggbb", or null for the system color. */
export declare function setTitlebarColor(background: string | null): Promise<void>;

/** Drag + double-click maximize for a page-drawn bar (Linux, decorations: false). Returns a disposer. */
export declare function enableDragRegion(bar: HTMLElement, appWindow?: Window): () => void;

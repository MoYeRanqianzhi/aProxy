// Frontend helpers for the custom title bar. The Windows half pairs with windows_titlebar.rs;
// enableDragRegion() is for platforms where the page draws the whole bar (Linux, decorations: false).
// Framework-agnostic ES module; works as-is in TS projects (`allowJs`) or rename to .ts.
//
// CSS variables written on <html>:
//   --titlebar-top     rows hidden above the screen while maximized (0 when restored)
//   --titlebar-height  visible caption-button height = recommended title bar row height
//   --titlebar-right   width reserved for the native caption buttons
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

export const isWindows = navigator.userAgent.includes('Windows');

/**
 * Keeps the CSS variables in sync. Geometry changes on maximize/restore, DPI change and resize, and
 * every one of those fires a window `resize` event in the page. Returns a disposer.
 */
export function watchTitlebarInsets(root = document.documentElement) {
  if (!isWindows) return () => {};
  let seq = 0;
  const apply = async () => {
    const mine = ++seq;
    const insets = await invoke('titlebar_insets');
    if (mine !== seq) return; // a newer resize already answered; drop stale geometry
    root.style.setProperty('--titlebar-top', `${insets.top}px`);
    root.style.setProperty('--titlebar-height', `${insets.height}px`);
    root.style.setProperty('--titlebar-right', `${insets.right}px`);
  };
  apply();
  addEventListener('resize', apply);
  return () => removeEventListener('resize', apply);
}

/**
 * Paints the native caption behind the buttons with the title bar color. DWM picks contrasting
 * glyph and hover colors automatically; do not call `setTheme()` for that (it also flips the page's
 * `prefers-color-scheme` on Windows).
 * @param {string | null} background "#rrggbb" of the title bar, or null for the system color
 */
export async function setTitlebarColor(background) {
  if (!isWindows) return;
  await invoke('set_caption_color', { color: background });
}

/**
 * Makes `bar` drag the window and toggle maximize on double-click, for bars without native drag
 * support (Linux with `decorations: false`). Only presses on the bar itself count, not its children.
 *
 * The OS move starts only after the pointer travels a few px. `data-tauri-drag-region` starts it on
 * the first press instead; on X11 the window manager's move grab then swallows the second press, so
 * double-click never maximizes. Needs `core:window:allow-start-dragging` and
 * `core:window:allow-toggle-maximize`. Do not use it on Windows; `app-region: drag` is native there.
 * Returns a disposer.
 */
export function enableDragRegion(bar, appWindow = getCurrentWindow()) {
  let start = null;
  const down = (e) => { if (e.button === 0 && e.target === bar) start = [e.screenX, e.screenY]; };
  const move = (e) => {
    if (start && (e.buttons & 1) && Math.hypot(e.screenX - start[0], e.screenY - start[1]) > 3) {
      start = null;
      appWindow.startDragging();
    }
  };
  const up = () => { start = null; };
  const dbl = (e) => { if (e.target === bar) appWindow.toggleMaximize(); };
  bar.addEventListener('mousedown', down);
  addEventListener('mousemove', move);
  addEventListener('mouseup', up);
  bar.addEventListener('dblclick', dbl);
  return () => {
    bar.removeEventListener('mousedown', down);
    removeEventListener('mousemove', move);
    removeEventListener('mouseup', up);
    bar.removeEventListener('dblclick', dbl);
  };
}

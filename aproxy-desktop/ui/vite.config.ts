import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// 产物将来由 Tauri 以自定义协议加载并嵌进 aproxy 二进制，所以：
// - base 用相对路径，不假设站点根；
// - 不引入任何运行时 CDN，字体等资源全部随包输出到 dist/。
export default defineConfig({
  base: './',
  plugins: [svelte()],
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: false,
  },
  build: {
    // WebView2（Chromium）、WebKitGTK、WKWebView 都支持的语法基线
    target: 'es2022',
    // 字体文件小于该阈值会被内联成 base64，反而让 CSS 变大且无法单独缓存；
    // 嵌入式应用不在乎请求数，统一输出为独立文件
    assetsInlineLimit: 0,
  },
});

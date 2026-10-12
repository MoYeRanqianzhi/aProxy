import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// 产物由 Tauri 以自定义协议加载并嵌进 aproxy 二进制，所以：
// - base 用相对路径，不假设站点根；
// - 不引入任何运行时 CDN，字体等资源全部随包输出；
// - 输出到 crate 根的 dist/（aproxy-desktop/dist）：generate_context! 从那里嵌入，crate 发布时
//   也只带它、不带 ui/ 的源码。它在 Vite 的根目录（ui/）之外，所以要显式允许清空。
// 开发服务器（npm run dev）不受这些影响，照样从源码提供页面。
export default defineConfig({
  base: './',
  plugins: [svelte()],
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: false,
  },
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    // 脚本按 Tauri 官方的 Vite 指南降级：Windows 的 WebView2 取 chrome105，WebKitGTK 与 WKWebView
    // 取 safari13。同一份产物要在三个平台上用，所以两个都写，取二者都能跑的语法。
    target: ['chrome105', 'safari13'],
    // 样式另定：界面的颜色全是 oklch() 与 color-mix()，要 Chromium 111、Safari 16.4（WebKitGTK 2.40）
    // 起才认，这才是界面真正的下限。照脚本的 safari13 降级时，构建会给每个颜色令牌补一份
    // display-p3 的覆盖，广色域屏幕上颜色随之变化；按这里的目标则原样输出，与审过的样子一致。
    cssTarget: ['chrome111', 'safari16.4'],
    // 字体文件小于该阈值会被内联成 base64，反而让 CSS 变大且无法单独缓存；
    // 嵌入式应用不在乎请求数，统一输出为独立文件
    assetsInlineLimit: 0,
  },
});

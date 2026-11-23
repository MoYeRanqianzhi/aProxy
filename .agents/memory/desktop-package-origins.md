---
name: desktop-package-origins
description: Recall when changing package URLs, style CSP or presenting repository verification in the desktop window.
metadata:
  type: project
  scope: Tauri 2.12 desktop scheme and GitHub release-asset facts for local packages
  status: active
  last_verified: 2026-11-23
---

包里的资源要放在有层级的 URL 上。Tauri 2.12.3 的 `scripts/core.js` 对 `convertFileSrc` 的整个参数做
`encodeURIComponent`：只把包的 id 交给它，包里的路径逐段编码后接在后面；编码过的斜杠不再是路径分隔符。按本应用默认的
协议选项，Windows 上的源是 `http://aproxy-pkg.localhost`，macOS 与 Linux 上是 `aproxy-pkg://localhost`。地址里的查询串
用来区分 webview 里同一个模块先后几次激活，不用来区分安装器里的文件。

**Why:** 浏览器按父资源解析 CSS 里的 URL 与模块的相对 import。style-src 里只要有 nonce 或哈希，浏览器就忽略
`'unsafe-inline'`。Tauri 的 manager 分别判断 script 与 style 两处 CSP 改写开关：只关掉 style 的改写，脚本的保护不变，
页面注入的样式也能生效。资源地址写得对、Rust 回了响应，都证明不了每个引擎真的加载了字体、样式表或模块。

**How to apply:** 改宿主时保持地址的层级，并在每个真实引擎里看计算后的样式、模块是否真的激活、相对路径的请求是否发出。
核对来源要拿**原始 ZIP** 的 sha256 去对 release asset 的摘要，不按文件名：改过名的下载照样能对上。摘要没对上时，star、
许可证这些仓库事实描述的只是一个谁都能写进 manifest 的仓库，不是用户选中的这些字节。另见 [[desktop-extension-trust]]
与[页面契约](../plan/w5e-extensions.md)里页面自己负责的决定。

**Evidence:** Tauri 2.12.3 的 `scripts/core.js`（`convertFileSrc`）与 `src/manager/mod.rs`（按指令分别处理 nonce 与哈希）；
`tests/desktop_packages.rs` 覆盖有层级的请求，以及本地模拟的 GitHub（含改名的下载、摘要不符、没有摘要）。对
`repos/MoYeRanqianzhi/aProxy/releases/latest` 的只读 GET 显示，上传的 release asset 带 `sha256:` 摘要。各平台原生引擎
上的验证另做（Codex 在 t10b 中查证，2026-11-23）。

**Recheck when:** 升级 Tauri 或 Wry、改 HTTPS 协议选项、改 CSP 的注入方式，或 GitHub 改了 release asset 摘要的契约。

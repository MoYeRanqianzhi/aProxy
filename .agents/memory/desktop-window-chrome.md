---
name: desktop-window-chrome
description: 改桌面窗口的标题栏、窗口按钮、页面布局宽度或滚动条之前回想：三个平台都用自定义标题栏，页面不许出现横向滚动条，渲染是否正确只认截图。
metadata:
  type: feedback
  scope: aproxy-desktop 的窗口外壳（aproxy-desktop/src/lib.rs、titlebar.rs）与页面布局（aproxy-desktop/ui/）
  status: active
  last_verified: 2026-11-25
---

三条规矩，都是用户在 2026-11-24/25 看截图时定下的：

1. **三个平台都用自定义标题栏，不用系统原生的。** Windows 保留 DWM 画的真按钮、页面画其余部分
   （`titlebar.rs`）；macOS 用 Overlay，交通灯居中在页面的 44px 标题栏里（`TRAFFIC_LIGHTS`）；Linux 去掉窗口
   管理器的装饰，页面画最小化、最大化/还原、关闭三个按钮（`2020a6b3`，`Titlebar.svelte`）。用户原话：macOS
   「正确设计为自定义标题栏而非系统原生标题栏」；Linux 用了原生标题栏加页面工具栏两层时，「linux没做自定义标题栏，
   完全错误」。
2. **页面不许出现横向滚动条。** 窗口变窄时要挤压布局（列宽 `minmax(0, …)`、窄了改上下排、长文本省略或折行），
   不能靠滚动兜底。用户原话：「放了一个横向的滚动条 导致不美观，需要换一种方式，比如挤压UI」。滚动条本身要跟界面
   同一套样子（丁香紫胶囊，`app.css`），不能是系统灰色的那条。
3. **渲染对不对只认截图。** 标题栏、按钮、布局改完，要在对应平台的真窗口里截图核对（浏览器预览只能当初筛）。

**Why:** 用户把窗口外观当作 0.2.0「精美 UI」的一部分验收，原生标题栏与横向滚动条都被直接判为错误；浏览器里看着对、
真窗口里不对的情况这一轮出现过（WebKitGTK 不认 `app-region`、WKWebView 的拖动、Server 版 Windows 上按钮区不变色）。

**How to apply:** 改布局后在浏览器里按最小窗口 900×600 与 922×607、1000、1100、1200 各宽度，中英文两种语言，
逐页（含日志详情抽屉的每个标签页）检查 `scrollWidth > clientWidth`；改标题栏或窗口按钮后，三个平台各截一张浅色、
一张深色。Linux 的拖动、双击、按钮、改大小用 `probe/screenshots` 分支的 `.github/screenshots/linux-titlebar.py`
实测。

**Evidence:** 用户 2026-11-24、2026-11-25 的消息（见上面原话）；`c736c636`、`493d5892`（横向溢出与滚动条），
浏览器审计 720 个抽屉视图无溢出，macOS 与 Windows runner 截图确认横向滚动条消失。

**Recheck when:** 用户改变对某个平台标题栏的要求，或 Tauri/WebView 的升级让某个平台的做法失效。
相关：[[ui-design-style]] [[desktop-screenshots]]

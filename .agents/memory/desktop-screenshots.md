---
name: desktop-screenshots
description: 做 README 截图、截图工作流或图集之前回想：用演示数据、每个细节页都截、完整图集放专门分支、Windows 要 Windows 11、截图不许缩放。
metadata:
  type: project
  scope: 截图工作流（目前在 probe/screenshots 分支）、README 的截图、APROXY_DESKTOP_DEMO
  status: active
  last_verified: 2026-11-26
---

用户在 2026-11-24/25 定下的截图做法：

- **用演示数据。** 窗口外壳是真的，数据用浏览器预览里那套演示数据：设 `APROXY_DESKTOP_DEMO=<en|zh-CN>,<light|dark>[,scene=<名字>]`，
  场景表在 `aproxy-desktop/ui/src/lib/backend/demo/scenes.ts`。这个开关只编进 debug 构建（`7e891836`），发布的二进制里没有。
- **每个细节页都截，展示各个功能。** README 只挑典型页面，其余作为额外图片供查看细节。
- **完整图集放到一个专门的分支上**（`screenshots`，首页由 `.github/screenshots/gallery.py` 生成，2026-11-26 首次推送 run 45539839130 的 153 张图）。每次生成都把这个分支重置成只有一个提交再强推，克隆只带最新一套图，
  `.git` 不会越积越大；README 用的几张典型图经 PR 进 main。一套图约一百张、二三十 MB（每张 140–430 KB）。
  README 里三个平台的切换只能用 `<details>`（GitHub markdown 没有标签页组件，也不能用脚本）。
- **Windows 用 Windows 11。** 用户：「演示也使用Win11，win10已经停止官方支持了」。`windows-latest` 是
  Windows Server 2025（镜像 windows-2025-vs2026），窗口是 Windows 10 的直角外观；公开仓库可用 `windows-11-arm`。
- **不许缩放。** 截图链路里任何一步都不缩放，给用户看原图（1:1）。托管 runner 都是 1 倍显示：Windows 默认
  1024×768（窗口被压到 924×680），macOS 虚拟显示器最高 1920×1080、没有 HiDPI 模式，Linux 的 Xvfb 尺寸自定。
  要让窗口按默认 1200×780 打开，先把屏幕调大（macOS 用 `displayplacer`，Windows 用 `ChangeDisplaySettings`）。

- **Windows 11 runner 的桌面不干净。** 登录后会弹首次登录的隐私向导、开始菜单，还有一个等着装 WSL 的终端，
  而且那个终端最小化后过一阵会自己再弹出来。`scene.py` 截图前：开始菜单在前台时按 Esc、`MinimizeAll`、还原并临时
  置顶 aProxy 窗口（只在开始菜单在前台时按 Esc，否则会关掉场景摆好的命令面板）。

**Why:** 截图同时是 README 的展示和三平台渲染正确性的证据（见 [[desktop-window-chrome]]）；把图集放进主线历史会让
每个克隆都背上所有历史版本的图。用户看到被缩小、被放大过的预览时直接指出了「画质低」。

**How to apply:** 截图工作流定稿前在 `probe/screenshots` 分支上迭代，改 main 上的 `.github/` 之前先告诉用户；专门分支
用孤立提交（无父提交）强推；预览给用户时用 1:1 的单张原图或按设备像素比还原的页面，不要拼图缩小。

**Evidence:** 用户 2026-11-24、2026-11-25 的消息；run 45438804960 的 Windows 任务日志里 `Image: windows-2025-vs2026`；
macOS runner 上 `displayplacer list` 的模式表；截图尺寸实测（Windows 924×680、Linux 1200×780、macOS 截图原为 1024 宽）。

**Recheck when:** GitHub 的托管 runner 镜像变化（Windows 11 x64 托管 runner、macOS 的 HiDPI 模式），或用户改变图集的放法。
相关：[[desktop-window-chrome]]

#!/usr/bin/env bash
# 无界面版本不得多出任何依赖：本机目标平台上 aproxy 不带默认特性的常规依赖树里，不能出现
# 桌面界面那一族（aproxy-desktop、tauri 及其子包、wry、tao、webview2-com、webkit2gtk）。
#
# 只查本机平台：cargo tree 不带 --target 时按 host 过滤，只需要本机构建本来就要下载的包。
# 三个平台的 headless 作业各跑一遍，合起来覆盖全部平台；不用 --target all——那会让一台
# 机器去下载别的平台专用的依赖源码，例如 Linux 去取 Windows 的 windows-sys。
#
# 用法：headless-tree.sh（在仓库根目录执行）
set -euo pipefail

tree=$(cargo tree -p aproxy --no-default-features -e normal --prefix none --locked)
if grep -E '^(aproxy-desktop|tauri|tauri-[a-z-]+|wry|tao|webview2-com|webkit2gtk) ' <<<"$tree"; then
  echo "::error::headless 的依赖树里出现了桌面界面的依赖（见上面列出的包）"
  exit 1
fi
echo "headless 依赖树（$(rustc -vV | sed -n 's/^host: //p')）里没有桌面界面的依赖"
